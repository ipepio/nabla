//! Embedded local CPU voice capabilities (TTS & STT) for Nabla.
//!
//! - `catalog`: Curated & custom model catalog under `<data_dir>/models/{tts,stt}/<model-id>/`
//! - `download`: Streaming HTTPS downloader with progress events & cancellation
//! - `g2p`: Pure-Rust Spanish/English IPA phonemizer for Kokoro-82M
//! - `tts`: In-process Kokoro-82M ONNX synthesis (`ort`)
//! - `stt`: In-process Moonshine ONNX transcription (`ort`)

pub mod catalog;
pub mod download;
pub mod g2p;
pub mod parakeet;
pub mod phonon;
pub mod recorder;
pub mod stt;
pub mod tts;
pub mod whisper;

use std::sync::OnceLock;
use ort::environment::Environment;

static ORT_ENV: OnceLock<Environment> = OnceLock::new();

/// Process-wide singleton ONNX Runtime environment shared by STT and TTS
/// (ORT allows only one Environment per process).
pub fn shared_ort_env() -> Result<Environment, String> {
    if let Some(env) = ORT_ENV.get() {
        return Ok(env.clone());
    }
    let env = ort::init()
        .build()
        .map_err(|e| format!("failed to initialize ONNX Runtime: {e}"))?;
    let _ = ORT_ENV.set(env.clone());
    Ok(ORT_ENV.get().cloned().unwrap_or(env))
}

pub use catalog::{
    CustomVoiceModelDraft, HostHardwareInfo, VoiceModelEntry, delete_model, detect_host_hardware,
    find_model_spec, import_custom_model, list_catalog,
};
pub use download::{DownloadSink, VoiceDownloadManager, VoiceDownloadProgress};
pub use recorder::VoiceRecorderState;
pub use stt::{VoiceSttEngine, VoiceTranscribeResult};
pub use tts::{VoiceSynthesizeResult, VoiceTtsEngine};
