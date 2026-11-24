//! Embedded local CPU Text-to-Speech via ONNX Runtime (`ort`) for Kokoro-82M.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use base64::Engine as _;
use ndarray::{Array1, Array2};
use ort::session::{Session, builder::GraphOptimizationLevel};
use ort::value::Tensor;

use super::catalog::{VoiceModelKind, find_model_spec, model_dir};
use super::g2p::{load_kokoro_vocab, phonemes_to_kokoro_ids, text_to_phonemes};
use super::phonon::LoadedPhonon;
use super::shared_ort_env;

pub const KOKORO_SAMPLE_RATE: u32 = 24_000;
const VOICE_EMBED_DIM: usize = 256;

/// Response returned to the webview for one synthesized text segment.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceSynthesizeResult {
    pub wav_base64: String,
    pub sample_rate: u32,
    pub duration_ms: u64,
}

struct LoadedKokoro {
    model_id: String,
    session: Session,
    vocab: HashMap<char, i64>,
    voices: HashMap<String, Vec<f32>>,
}

enum LoadedTts {
    Kokoro(LoadedKokoro),
    Phonon(LoadedPhonon),
}

impl LoadedTts {
    fn model_id(&self) -> &str {
        match self {
            LoadedTts::Kokoro(k) => &k.model_id,
            LoadedTts::Phonon(p) => &p.model_id,
        }
    }
}

/// Lazy-loaded in-process TTS engine (Kokoro-82M & Phonon 40M).
pub struct VoiceTtsEngine {
    loaded: Mutex<Option<LoadedTts>>,
}

impl Default for VoiceTtsEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl VoiceTtsEngine {
    pub fn new() -> Self {
        Self {
            loaded: Mutex::new(None),
        }
    }

    /// Drops the cached session if it belongs to `model_id` (e.g. before delete).
    pub fn evict(&self, model_id: &str) {
        if let Ok(mut guard) = self.loaded.lock()
            && guard.as_ref().is_some_and(|l| l.model_id() == model_id)
        {
            *guard = None;
        }
    }

    /// Synthesizes one text segment into a 24 kHz mono 16-bit WAV payload.
    pub fn synthesize(
        &self,
        data_dir: &Path,
        model_id: &str,
        voice_id: &str,
        text: &str,
        speed: f32,
        language: Option<&str>,
    ) -> Result<VoiceSynthesizeResult, String> {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Err("cannot synthesize empty text".to_string());
        }

        let spec = find_model_spec(data_dir, model_id)
            .ok_or_else(|| format!("unknown TTS model: {model_id}"))?;
        if spec.kind != VoiceModelKind::Tts {
            return Err(format!("model '{model_id}' is not a TTS model"));
        }
        if !spec.runtime_ready {
            return Err(format!(
                "The '{}' runtime ({}) is not wired yet — select Kokoro 82M in Settings → Voice.",
                spec.name, spec.engine
            ));
        }

        let base = model_dir(data_dir, VoiceModelKind::Tts, &spec.id);

        if spec.engine == "phonon-gguf" {
            let inferred_lang = language
                .filter(|l| !l.trim().is_empty())
                .or_else(|| {
                    spec.voices
                        .iter()
                        .find(|v| v.id == voice_id)
                        .map(|v| v.language.as_str())
                })
                .unwrap_or("es");
            let mut guard = self
                .loaded
                .lock()
                .map_err(|_| "TTS engine lock poisoned".to_string())?;
            let needs_load = match guard.as_ref() {
                Some(LoadedTts::Phonon(p)) => !p.matches(&spec.id, voice_id, inferred_lang),
                _ => true,
            };
            if needs_load {
                let phonon = LoadedPhonon::load(&spec.id, &base, voice_id, inferred_lang)?;
                *guard = Some(LoadedTts::Phonon(phonon));
            }
            let Some(LoadedTts::Phonon(phonon)) = guard.as_ref() else {
                return Err("TTS engine state mismatch".to_string());
            };
            return phonon.synthesize(trimmed);
        }

        let onnx_path = resolve_kokoro_onnx(&base)?;
        let tokenizer_path = base.join("tokenizer.json");
        if !tokenizer_path.is_file() {
            return Err(format!(
                "Kokoro tokenizer.json not found in {} — re-download the model in Settings → Voice",
                base.display()
            ));
        }

        let voice_path = resolve_voice_path(&base, voice_id)?;
        let inferred_lang = language
            .filter(|l| !l.trim().is_empty())
            .or_else(|| {
                spec.voices
                    .iter()
                    .find(|v| v.id == voice_id)
                    .map(|v| v.language.as_str())
            })
            .unwrap_or("es");

        let env = shared_ort_env()?;
        let mut guard = self
            .loaded
            .lock()
            .map_err(|_| "TTS engine lock poisoned".to_string())?;

        let needs_load = guard
            .as_ref()
            .map(|l| l.model_id() != spec.id)
            .unwrap_or(true);
        if needs_load {
            let threads = std::thread::available_parallelism()
                .map(|n| n.get().clamp(1, 4))
                .unwrap_or(2);
            let session = Session::builder(&env)
                .map_err(|e| format!("ORT session builder error: {e}"))?
                .with_optimization_level(GraphOptimizationLevel::Level3)
                .map_err(|e| format!("ORT optimization error: {e}"))?
                .with_intra_threads(threads)
                .map_err(|e| format!("ORT threads error: {e}"))?
                .commit_from_file(&onnx_path)
                .map_err(|e| format!("failed to load Kokoro ONNX model: {e}"))?;
            let vocab = load_kokoro_vocab(&tokenizer_path)?;
            *guard = Some(LoadedTts::Kokoro(LoadedKokoro {
                model_id: spec.id.clone(),
                session,
                vocab,
                voices: HashMap::new(),
            }));
        }

        let Some(LoadedTts::Kokoro(loaded)) = guard.as_mut() else {
            return Err("TTS engine state mismatch".to_string());
        };

        if !loaded.voices.contains_key(voice_id) {
            let floats = read_voice_bin(&voice_path)?;
            loaded.voices.insert(voice_id.to_string(), floats);
        }

        let phonemes = text_to_phonemes(trimmed, inferred_lang);
        let token_ids = phonemes_to_kokoro_ids(&phonemes, &loaded.vocab);
        let style_row = token_ids.len().saturating_sub(2).min(509);
        let voice_data = loaded.voices.get(voice_id).expect("cached voice");
        let style_slice = slice_style_vector(voice_data, style_row)?;

        let seq_len = token_ids.len();
        let input_ids_arr = Array2::from_shape_vec((1, seq_len), token_ids)
            .map_err(|e| format!("input_ids shape error: {e}"))?;
        let style_arr = Array2::from_shape_vec((1, VOICE_EMBED_DIM), style_slice)
            .map_err(|e| format!("style shape error: {e}"))?;
        let clamped_speed = speed.clamp(0.5, 2.0);
        let speed_arr = Array1::from_vec(vec![clamped_speed]);

        let input_ids_tensor = Tensor::from_array(input_ids_arr)
            .map_err(|e| format!("input_ids tensor error: {e}"))?;
        let style_tensor =
            Tensor::from_array(style_arr).map_err(|e| format!("style tensor error: {e}"))?;
        let speed_tensor =
            Tensor::from_array(speed_arr).map_err(|e| format!("speed tensor error: {e}"))?;

        let outputs = loaded
            .session
            .run(ort::inputs![
                "input_ids" => input_ids_tensor,
                "style" => style_tensor,
                "speed" => speed_tensor,
            ])
            .map_err(|e| format!("Kokoro inference failed: {e}"))?;

        let (_, first_out) = outputs
            .iter()
            .next()
            .ok_or_else(|| "Kokoro produced no output tensor".to_string())?;
        let audio = first_out
            .try_extract_array::<f32>()
            .map_err(|e| format!("failed to extract audio waveform: {e}"))?;
        let samples: Vec<f32> = audio.iter().copied().collect();
        let duration_ms = ((samples.len() as u64) * 1000) / (KOKORO_SAMPLE_RATE as u64);
        let wav = encode_wav_pcm16(&samples, KOKORO_SAMPLE_RATE);

        Ok(VoiceSynthesizeResult {
            wav_base64: base64::engine::general_purpose::STANDARD.encode(wav),
            sample_rate: KOKORO_SAMPLE_RATE,
            duration_ms,
        })
    }
}

fn resolve_kokoro_onnx(base: &Path) -> Result<PathBuf, String> {
    let candidates = [
        base.join("onnx/model_quantized.onnx"),
        base.join("onnx/model.onnx"),
        base.join("model.onnx"),
    ];
    candidates
        .into_iter()
        .find(|p| p.is_file())
        .ok_or_else(|| "Kokoro ONNX model not installed. Download it in Settings → Voice.".to_string())
}

fn resolve_voice_path(base: &Path, voice_id: &str) -> Result<PathBuf, String> {
    let clean = voice_id.trim();
    if clean.is_empty() {
        return Err("No TTS voice selected. Choose a downloaded voice in Settings → Voice.".to_string());
    }
    let path = base.join("voices").join(format!("{clean}.bin"));
    if path.is_file() {
        return Ok(path);
    }
    Err(format!(
        "Voice '{clean}' is not downloaded. Download it in Settings → Voice."
    ))
}

fn read_voice_bin(path: &Path) -> Result<Vec<f32>, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read voice file: {e}"))?;
    if bytes.len() < VOICE_EMBED_DIM * 4 || bytes.len() % 4 != 0 {
        return Err(format!("corrupt voice embedding file ({} bytes)", bytes.len()));
    }
    let mut floats = Vec::with_capacity(bytes.len() / 4);
    for chunk in bytes.as_chunks::<4>().0 {
        floats.push(f32::from_le_bytes(*chunk));
    }
    Ok(floats)
}

fn slice_style_vector(voice_data: &[f32], row_index: usize) -> Result<Vec<f32>, String> {
    let total_rows = voice_data.len() / VOICE_EMBED_DIM;
    if total_rows == 0 {
        return Err("voice embedding contains no style rows".to_string());
    }
    let row = row_index.min(total_rows - 1);
    let start = row * VOICE_EMBED_DIM;
    Ok(voice_data[start..start + VOICE_EMBED_DIM].to_vec())
}

/// Encodes mono `f32` PCM (`-1.0..=1.0`) as a standard 16-bit signed PCM WAV.
pub fn encode_wav_pcm16(samples: &[f32], sample_rate: u32) -> Vec<u8> {
    let data_bytes = (samples.len() * 2) as u32;
    let mut buf = Vec::with_capacity(44 + data_bytes as usize);
    buf.extend_from_slice(b"RIFF");
    buf.extend_from_slice(&(36 + data_bytes).to_le_bytes());
    buf.extend_from_slice(b"WAVE");
    buf.extend_from_slice(b"fmt ");
    buf.extend_from_slice(&16u32.to_le_bytes()); // PCM fmt chunk size
    buf.extend_from_slice(&1u16.to_le_bytes()); // PCM format
    buf.extend_from_slice(&1u16.to_le_bytes()); // mono
    buf.extend_from_slice(&sample_rate.to_le_bytes());
    buf.extend_from_slice(&(sample_rate * 2).to_le_bytes()); // byte rate
    buf.extend_from_slice(&2u16.to_le_bytes()); // block align
    buf.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    buf.extend_from_slice(b"data");
    buf.extend_from_slice(&data_bytes.to_le_bytes());
    for &s in samples {
        let clamped = s.clamp(-1.0, 1.0);
        let q = (clamped * 32767.0).round() as i16;
        buf.extend_from_slice(&q.to_le_bytes());
    }
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_encoder_produces_valid_riff_header_and_clamps_samples() {
        let wav = encode_wav_pcm16(&[0.0, 1.5, -1.5], 24_000);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(wav.len(), 44 + 6);
        let s1 = i16::from_le_bytes([wav[46], wav[47]]);
        let s2 = i16::from_le_bytes([wav[48], wav[49]]);
        assert_eq!(s1, 32767);
        assert_eq!(s2, -32767);
    }

    #[test]
    #[ignore]
    fn live_kokoro_synthesizes_real_wav_from_installed_model() {
        let home = std::env::var("HOME").expect("HOME");
        let data_dir = PathBuf::from(home).join("Library/Application Support/com.ipepio.nabla");
        let engine = VoiceTtsEngine::new();
        let res = engine
            .synthesize(&data_dir, "kokoro-82m", "ef_dora", "Hola mundo desde Nabla.", 1.0, Some("es"))
            .expect("kokoro synthesize");
        assert!(res.duration_ms > 200, "expected audible duration, got {}ms", res.duration_ms);
        assert!(!res.wav_base64.is_empty());
    }
}
