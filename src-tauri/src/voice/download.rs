//! Streaming HTTPS downloader for local voice models & voices with progress
//! reporting, atomic `.part` commits, and per-model cancellation.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use std::io::Write;

use super::catalog::{VoiceFileSpec, VoiceModelSpec, model_dir};

/// Event emitted on the `voice-download-progress` Tauri channel.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceDownloadProgress {
    pub model_id: String,
    pub voice_id: Option<String>,
    pub file_name: String,
    pub downloaded_bytes: u64,
    pub total_bytes: u64,
    /// `"downloading"` | `"completed"` | `"cancelled"` | `"error"`.
    pub status: String,
    pub error: Option<String>,
}

pub type DownloadSink = Arc<dyn Fn(&VoiceDownloadProgress) + Send + Sync>;

/// Tracks in-flight model downloads so each can be cancelled cleanly and the
/// catalog listing knows which models are currently downloading.
#[derive(Default)]
pub struct VoiceDownloadManager {
    active: Mutex<HashMap<String, Arc<AtomicBool>>>,
}

impl VoiceDownloadManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn active_ids(&self) -> HashSet<String> {
        self.active
            .lock()
            .map(|guard| guard.keys().cloned().collect())
            .unwrap_or_default()
    }

    pub fn cancel(&self, model_id: &str) -> bool {
        let Ok(guard) = self.active.lock() else {
            return false;
        };
        if let Some(flag) = guard.get(model_id) {
            flag.store(true, Ordering::SeqCst);
            return true;
        }
        false
    }

    /// Registers a download slot; returns `Err` if that model is already downloading.
    fn acquire(&self, model_id: &str) -> Result<Arc<AtomicBool>, String> {
        let mut guard = self
            .active
            .lock()
            .map_err(|_| "download registry poisoned".to_string())?;
        if guard.contains_key(model_id) {
            return Err(format!("model '{model_id}' is already downloading"));
        }
        let flag = Arc::new(AtomicBool::new(false));
        guard.insert(model_id.to_string(), Arc::clone(&flag));
        Ok(flag)
    }

    fn release(&self, model_id: &str) {
        if let Ok(mut guard) = self.active.lock() {
            guard.remove(model_id);
        }
    }

    /// Starts downloading the model's core files (if not yet present) plus any
    /// requested `voice_ids` on a background Tokio task.
    pub fn start_download(
        self: &Arc<Self>,
        data_dir: PathBuf,
        spec: VoiceModelSpec,
        voice_ids: Vec<String>,
        sink: DownloadSink,
    ) -> Result<(), String> {
        if spec.files.is_empty() && voice_ids.is_empty() {
            return Err(format!(
                "No downloadable files configured for '{}'.",
                spec.name
            ));
        }
        let cancel_flag = self.acquire(&spec.id)?;
        let manager = Arc::clone(self);
        let model_id = spec.id.clone();

        tauri::async_runtime::spawn(async move {
            let result =
                run_download(&data_dir, &spec, &voice_ids, &cancel_flag, &sink).await;
            manager.release(&model_id);

            match result {
                Ok(()) => {
                    sink(&VoiceDownloadProgress {
                        model_id: model_id.clone(),
                        voice_id: None,
                        file_name: String::new(),
                        downloaded_bytes: 0,
                        total_bytes: 0,
                        status: "completed".to_string(),
                        error: None,
                    });
                }
                Err(DownloadFailure::Cancelled) => {
                    sink(&VoiceDownloadProgress {
                        model_id: model_id.clone(),
                        voice_id: None,
                        file_name: String::new(),
                        downloaded_bytes: 0,
                        total_bytes: 0,
                        status: "cancelled".to_string(),
                        error: None,
                    });
                }
                Err(DownloadFailure::Error(msg)) => {
                    sink(&VoiceDownloadProgress {
                        model_id: model_id.clone(),
                        voice_id: None,
                        file_name: String::new(),
                        downloaded_bytes: 0,
                        total_bytes: 0,
                        status: "error".to_string(),
                        error: Some(msg),
                    });
                }
            }
        });

        Ok(())
    }
}

enum DownloadFailure {
    Cancelled,
    Error(String),
}

async fn run_download(
    data_dir: &Path,
    spec: &VoiceModelSpec,
    voice_ids: &[String],
    cancel: &AtomicBool,
    sink: &DownloadSink,
) -> Result<(), DownloadFailure> {
    let base = model_dir(data_dir, spec.kind, &spec.id);
    std::fs::create_dir_all(&base)
        .map_err(|e| DownloadFailure::Error(format!("cannot create model dir: {e}")))?;

    let mut queue: Vec<(Option<String>, VoiceFileSpec)> = Vec::new();
    for f in &spec.files {
        if !base.join(&f.rel_path).is_file() {
            queue.push((None, f.clone()));
        }
    }
    for vid in voice_ids {
        if let Some(voice) = spec.voices.iter().find(|v| &v.id == vid) {
            let dest = base.join(&voice.rel_path);
            if !dest.is_file() {
                queue.push((
                    Some(voice.id.clone()),
                    VoiceFileSpec {
                        rel_path: voice.rel_path.clone(),
                        url: voice.url.clone(),
                        approx_bytes: voice.approx_bytes,
                    },
                ));
            }
        }
    }

    if queue.is_empty() {
        return Ok(());
    }

    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(20))
        .user_agent("Nabla/0.2.0")
        .build()
        .map_err(|e| DownloadFailure::Error(format!("http client error: {e}")))?;

    let approx_total: u64 = queue.iter().map(|(_, f)| f.approx_bytes).sum();
    let mut completed_bytes: u64 = 0;

    for (voice_id, file) in queue {
        if cancel.load(Ordering::SeqCst) {
            return Err(DownloadFailure::Cancelled);
        }

        let dest = base.join(&file.rel_path);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| DownloadFailure::Error(format!("cannot create subdir: {e}")))?;
        }
        let part = dest.with_extension("part");

        let resp = client
            .get(&file.url)
            .send()
            .await
            .map_err(|e| DownloadFailure::Error(format!("download failed for {}: {e}", file.rel_path)))?;

        if !resp.status().is_success() {
            return Err(DownloadFailure::Error(format!(
                "HTTP {} downloading {}",
                resp.status().as_u16(),
                file.rel_path
            )));
        }

        let content_len = resp.content_length().unwrap_or(file.approx_bytes);
        let effective_total = approx_total.max(completed_bytes + content_len);

        let mut out = std::fs::File::create(&part)
            .map_err(|e| DownloadFailure::Error(format!("cannot create {}: {e}", part.display())))?;

        let mut file_downloaded: u64 = 0;
        let mut last_emit = Instant::now() - Duration::from_secs(1);
        let mut stream = resp.bytes_stream();

        while let Some(chunk_res) = stream.next().await {
            if cancel.load(Ordering::SeqCst) {
                drop(out);
                let _ = std::fs::remove_file(&part);
                return Err(DownloadFailure::Cancelled);
            }
            let chunk = chunk_res
                .map_err(|e| DownloadFailure::Error(format!("stream error for {}: {e}", file.rel_path)))?;
            out.write_all(&chunk)
                .map_err(|e| DownloadFailure::Error(format!("write error for {}: {e}", file.rel_path)))?;
            file_downloaded += chunk.len() as u64;

            if last_emit.elapsed() >= Duration::from_millis(150) {
                last_emit = Instant::now();
                sink(&VoiceDownloadProgress {
                    model_id: spec.id.clone(),
                    voice_id: voice_id.clone(),
                    file_name: file.rel_path.clone(),
                    downloaded_bytes: completed_bytes + file_downloaded,
                    total_bytes: effective_total,
                    status: "downloading".to_string(),
                    error: None,
                });
            }
        }

        out.flush()
            .map_err(|e| DownloadFailure::Error(format!("flush error: {e}")))?;
        drop(out);

        std::fs::rename(&part, &dest)
            .map_err(|e| DownloadFailure::Error(format!("cannot commit {}: {e}", file.rel_path)))?;

        completed_bytes += file_downloaded;
        sink(&VoiceDownloadProgress {
            model_id: spec.id.clone(),
            voice_id,
            file_name: file.rel_path.clone(),
            downloaded_bytes: completed_bytes,
            total_bytes: effective_total.max(completed_bytes),
            status: "downloading".to_string(),
            error: None,
        });
    }

    Ok(())
}
