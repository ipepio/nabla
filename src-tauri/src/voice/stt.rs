//! Embedded local CPU Speech-to-Text via ONNX Runtime (`ort`) for Moonshine.
//!
//! Supports both Moonshine layouts in our curated catalog:
//! - `moonshine-seq2seq` (`onnx-community/moonshine-tiny-ONNX`):
//!   `encoder_model_quantized.onnx` + `decoder_model_merged_quantized.onnx`
//! - `moonshine-streaming` (`Masterx/moonshine-streaming-{tiny,small}-es-ONNX`):
//!   5-graph incremental runtime (`frontend`, `encoder_int8`, `adapter_int8`,
//!   `cross_kv_int8`, `decoder_kv_int8` + `streaming_config.json`)

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use base64::Engine as _;
use ndarray::{Array1, Array2, Array3};
use ort::environment::Environment;
use ort::session::{Session, builder::GraphOptimizationLevel};
use ort::value::Tensor;

use super::catalog::{VoiceModelKind, find_model_spec, model_dir};
use super::parakeet::LoadedParakeet;
use super::shared_ort_env;
use super::whisper::LoadedWhisper;

pub const STT_SAMPLE_RATE: u32 = 16_000;
const MAX_DECODE_TOKENS: usize = 256;

/// Response returned to the webview after local PCM transcription.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceTranscribeResult {
    pub text: String,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct StreamingConfig {
    #[serde(default = "default_bos")]
    bos_id: i64,
    #[serde(default = "default_eos")]
    eos_id: i64,
    #[serde(default = "default_depth")]
    depth: usize,
    #[serde(default = "default_nheads")]
    nheads: usize,
    #[serde(default = "default_head_dim")]
    head_dim: usize,
    #[serde(default = "default_frame_len")]
    frame_len: usize,
}

fn default_bos() -> i64 {
    1
}
fn default_eos() -> i64 {
    2
}
fn default_depth() -> usize {
    6
}
fn default_nheads() -> usize {
    8
}
fn default_head_dim() -> usize {
    40
}
fn default_frame_len() -> usize {
    80
}

enum LoadedSttBackend {
    Seq2Seq {
        encoder: Session,
        decoder: Session,
        bos_id: i64,
        eos_id: i64,
        id_to_token: HashMap<i64, String>,
    },
    Streaming {
        frontend: Session,
        encoder: Session,
        adapter: Session,
        cross_kv: Session,
        decoder_kv: Session,
        config: StreamingConfig,
        id_to_token: HashMap<i64, String>,
    },
    Whisper(Box<LoadedWhisper>),
    Parakeet(Box<LoadedParakeet>),
}

struct LoadedStt {
    model_id: String,
    backend: LoadedSttBackend,
}

/// Lazy-loaded in-process Moonshine STT engine.
pub struct VoiceSttEngine {
    loaded: Mutex<Option<LoadedStt>>,
}

impl Default for VoiceSttEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl VoiceSttEngine {
    pub fn new() -> Self {
        Self {
            loaded: Mutex::new(None),
        }
    }

    pub fn evict(&self, model_id: &str) {
        if let Ok(mut guard) = self.loaded.lock()
            && guard.as_ref().is_some_and(|l| l.model_id == model_id)
        {
            *guard = None;
        }
    }

    /// Transcribes base64-encoded 16-bit signed LE mono PCM (16 kHz) locally on CPU.
    pub fn transcribe_pcm(
        &self,
        data_dir: &Path,
        model_id: &str,
        pcm_base64: &str,
        sample_rate: u32,
    ) -> Result<VoiceTranscribeResult, String> {
        let raw_bytes = base64::engine::general_purpose::STANDARD
            .decode(pcm_base64.trim())
            .map_err(|e| format!("invalid PCM base64: {e}"))?;
        if raw_bytes.len() < 320 || raw_bytes.len() % 2 != 0 {
            return Err("audio clip is too short to transcribe".to_string());
        }
        let mut samples = Vec::with_capacity(raw_bytes.len() / 2);
        for chunk in raw_bytes.as_chunks::<2>().0 {
            let s = i16::from_le_bytes(*chunk);
            samples.push((s as f32) / 32768.0);
        }
        self.transcribe_samples(data_dir, model_id, &samples, sample_rate)
    }

    /// Transcribes mono `f32` samples directly from native microphone capture.
    pub fn transcribe_samples(
        &self,
        data_dir: &Path,
        model_id: &str,
        samples: &[f32],
        sample_rate: u32,
    ) -> Result<VoiceTranscribeResult, String> {
        if samples.len() < 160 {
            return Err("audio clip is too short to transcribe".to_string());
        }
        let audio = if sample_rate != STT_SAMPLE_RATE && sample_rate > 0 {
            resample_linear(samples, sample_rate, STT_SAMPLE_RATE)
        } else {
            samples.to_vec()
        };
        let duration_ms = ((audio.len() as u64) * 1000) / (STT_SAMPLE_RATE as u64);

        let spec = find_model_spec(data_dir, model_id)
            .ok_or_else(|| format!("unknown STT model: {model_id}"))?;
        if spec.kind != VoiceModelKind::Stt {
            return Err(format!("model '{model_id}' is not an STT model"));
        }
        if !spec.runtime_ready {
            return Err(format!(
                "The '{}' runtime ({}) is not wired yet — select Moonshine Tiny/Small in Settings → Voice.",
                spec.name, spec.engine
            ));
        }
        let base = model_dir(data_dir, VoiceModelKind::Stt, &spec.id);

        let env = shared_ort_env()?;
        let mut guard = self
            .loaded
            .lock()
            .map_err(|_| "STT engine lock poisoned".to_string())?;

        let needs_load = guard
            .as_ref()
            .map(|l| l.model_id != spec.id)
            .unwrap_or(true);
        if needs_load {
            let threads = std::thread::available_parallelism()
                .map(|n| n.get().clamp(1, 4))
                .unwrap_or(2);
            let backend = match spec.engine.as_str() {
                "whisper" => {
                    LoadedSttBackend::Whisper(Box::new(LoadedWhisper::load(&env, &base, threads)?))
                }
                "parakeet" => LoadedSttBackend::Parakeet(Box::new(LoadedParakeet::load(
                    &env, &base, threads,
                )?)),
                "moonshine-streaming" => {
                    let tokenizer_path = base.join("tokenizer.json");
                    if !tokenizer_path.is_file() {
                        return Err(
                            "STT model is not installed. Download it first in Settings → Voice."
                                .to_string(),
                        );
                    }
                    let id_to_token = load_id_to_token(&tokenizer_path)?;
                    load_streaming_backend(&env, &base, threads, id_to_token)?
                }
                _ => {
                    let tokenizer_path = base.join("tokenizer.json");
                    if !tokenizer_path.is_file() {
                        return Err(
                            "STT model is not installed. Download it first in Settings → Voice."
                                .to_string(),
                        );
                    }
                    let id_to_token = load_id_to_token(&tokenizer_path)?;
                    load_seq2seq_backend(&env, &base, threads, id_to_token)?
                }
            };
            *guard = Some(LoadedStt {
                model_id: spec.id.clone(),
                backend,
            });
        }

        let loaded = guard.as_mut().expect("loaded stt");
        let text = match &mut loaded.backend {
            LoadedSttBackend::Seq2Seq {
                encoder,
                decoder,
                bos_id,
                eos_id,
                id_to_token,
            } => {
                let ids = run_seq2seq(encoder, decoder, &audio, *bos_id, *eos_id)?;
                decode_token_ids(&ids, id_to_token)
            }
            LoadedSttBackend::Streaming {
                frontend,
                encoder,
                adapter,
                cross_kv,
                decoder_kv,
                config,
                id_to_token,
            } => {
                let ids =
                    run_streaming(frontend, encoder, adapter, cross_kv, decoder_kv, config, &audio)?;
                decode_token_ids(&ids, id_to_token)
            }
            LoadedSttBackend::Whisper(w) => w.transcribe(&audio)?,
            LoadedSttBackend::Parakeet(p) => p.transcribe(&audio)?,
        };

        Ok(VoiceTranscribeResult { text, duration_ms })
    }
}

fn build_session(env: &Environment, path: &Path, threads: usize) -> Result<Session, String> {
    if !path.is_file() {
        return Err(format!("missing ONNX file: {}", path.display()));
    }
    Session::builder(env)
        .map_err(|e| format!("ORT session builder error: {e}"))?
        .with_optimization_level(GraphOptimizationLevel::Level3)
        .map_err(|e| format!("ORT optimization error: {e}"))?
        .with_intra_threads(threads)
        .map_err(|e| format!("ORT threads error: {e}"))?
        .commit_from_file(path)
        .map_err(|e| format!("failed to load {}: {e}", path.display()))
}

fn load_seq2seq_backend(
    env: &Environment,
    base: &Path,
    threads: usize,
    id_to_token: HashMap<i64, String>,
) -> Result<LoadedSttBackend, String> {
    let encoder_path = [
        base.join("onnx/encoder_model_quantized.onnx"),
        base.join("onnx/encoder_model.onnx"),
        base.join("encoder_model.onnx"),
    ]
    .into_iter()
    .find(|p| p.is_file())
    .ok_or_else(|| "Moonshine encoder ONNX not found".to_string())?;

    let decoder_path = [
        base.join("onnx/decoder_model_merged_quantized.onnx"),
        base.join("onnx/decoder_model_merged.onnx"),
        base.join("decoder_model_merged.onnx"),
    ]
    .into_iter()
    .find(|p| p.is_file())
    .ok_or_else(|| "Moonshine decoder ONNX not found".to_string())?;

    Ok(LoadedSttBackend::Seq2Seq {
        encoder: build_session(env, &encoder_path, threads)?,
        decoder: build_session(env, &decoder_path, threads)?,
        bos_id: 1,
        eos_id: 2,
        id_to_token,
    })
}

fn load_streaming_backend(
    env: &Environment,
    base: &Path,
    threads: usize,
    id_to_token: HashMap<i64, String>,
) -> Result<LoadedSttBackend, String> {
    let cfg_body = std::fs::read_to_string(base.join("streaming_config.json"))
        .map_err(|e| format!("missing streaming_config.json: {e}"))?;
    let config: StreamingConfig = serde_json::from_str(&cfg_body)
        .map_err(|e| format!("invalid streaming_config.json: {e}"))?;

    Ok(LoadedSttBackend::Streaming {
        frontend: build_session(env, &base.join("frontend.onnx"), threads)?,
        encoder: build_session(env, &base.join("encoder_int8.onnx"), threads)?,
        adapter: build_session(env, &base.join("adapter_int8.onnx"), threads)?,
        cross_kv: build_session(env, &base.join("cross_kv_int8.onnx"), threads)?,
        decoder_kv: build_session(env, &base.join("decoder_kv_int8.onnx"), threads)?,
        config,
        id_to_token,
    })
}

fn run_seq2seq(
    encoder: &mut Session,
    decoder: &mut Session,
    audio: &[f32],
    bos_id: i64,
    eos_id: i64,
) -> Result<Vec<i64>, String> {
    let input_values = Array2::from_shape_vec((1, audio.len()), audio.to_vec())
        .map_err(|e| format!("audio tensor shape error: {e}"))?;
    let enc_in =
        Tensor::from_array(input_values).map_err(|e| format!("encoder tensor error: {e}"))?;
    let enc_out = encoder
        .run(ort::inputs!["input_values" => enc_in])
        .map_err(|e| format!("Moonshine encoder failed: {e}"))?;
    let (_, hidden_val) = enc_out
        .iter()
        .next()
        .ok_or_else(|| "Moonshine encoder returned no output".to_string())?;
    let hidden_view = hidden_val
        .try_extract_array::<f32>()
        .map_err(|e| format!("encoder hidden extract error: {e}"))?;
    let hidden_shape = hidden_view.shape();
    let hidden_arr = Array3::from_shape_vec(
        (hidden_shape[0], hidden_shape[1], hidden_shape[2]),
        hidden_view.iter().copied().collect(),
    )
    .map_err(|e| format!("hidden shape error: {e}"))?;

    // Greedy decoding (stateless pass with use_cache_branch=false for compatibility
    // across Moonshine seq2seq ONNX exports).
    let max_tokens = ((audio.len() as f32 / STT_SAMPLE_RATE as f32) * 6.0)
        .ceil()
        .clamp(16.0, MAX_DECODE_TOKENS as f32) as usize;
    let mut tokens: Vec<i64> = vec![bos_id];

    for _ in 0..max_tokens {
        let dec_ids = Array2::from_shape_vec((1, tokens.len()), tokens.clone())
            .map_err(|e| format!("decoder ids shape error: {e}"))?;
        let dec_ids_t =
            Tensor::from_array(dec_ids).map_err(|e| format!("decoder ids tensor error: {e}"))?;
        let hidden_t = Tensor::from_array(hidden_arr.clone())
            .map_err(|e| format!("decoder hidden tensor error: {e}"))?;
        let use_cache = Tensor::from_array(Array1::from_vec(vec![false]))
            .map_err(|e| format!("use_cache tensor error: {e}"))?;

        let dec_out = decoder
            .run(ort::inputs![
                "input_ids" => dec_ids_t,
                "encoder_hidden_states" => hidden_t,
                "use_cache_branch" => use_cache,
            ])
            .map_err(|e| format!("Moonshine decoder failed: {e}"))?;
        let (_, logits_val) = dec_out
            .iter()
            .next()
            .ok_or_else(|| "Moonshine decoder returned no logits".to_string())?;
        let logits = logits_val
            .try_extract_array::<f32>()
            .map_err(|e| format!("logits extract error: {e}"))?;
        let shape = logits.shape();
        let vocab_size = shape[shape.len() - 1];
        let last_row_start = logits.len() - vocab_size;
        let next_tok = argmax(&logits.as_slice().unwrap_or(&[])[last_row_start..]) as i64;
        if next_tok == eos_id {
            break;
        }
        tokens.push(next_tok);
    }

    Ok(tokens)
}

fn run_streaming(
    frontend: &mut Session,
    encoder: &mut Session,
    adapter: &mut Session,
    cross_kv: &mut Session,
    decoder_kv: &mut Session,
    cfg: &StreamingConfig,
    audio: &[f32],
) -> Result<Vec<i64>, String> {
    // Pad audio to a multiple of 640 samples (required by Moonshine streaming frontend).
    let chunk_unit = (cfg.frame_len * 8).max(640);
    let padded_len = audio.len().div_ceil(chunk_unit) * chunk_unit;
    let mut padded = vec![0.0f32; padded_len];
    padded[..audio.len()].copy_from_slice(audio);

    // 1. Frontend: audio_chunk [1, N] + zero initial states -> features [1, T, D]
    let audio_t = Tensor::from_array(
        Array2::from_shape_vec((1, padded_len), padded)
            .map_err(|e| format!("frontend audio shape error: {e}"))?,
    )
    .map_err(|e| format!("frontend audio tensor error: {e}"))?;

    let mut fe_inputs: Vec<(String, ort::value::DynValue)> = Vec::new();
    for inp in frontend.inputs() {
        let name = inp.name().to_string();
        if name == "audio_chunk" {
            fe_inputs.push((name, audio_t.into_dyn()));
            break;
        }
    }
    if fe_inputs.is_empty() {
        return Err("Moonshine streaming frontend missing 'audio_chunk' input".to_string());
    }

    // Build zeroed state tensors matching each frontend ONNX input's exact dtype & shape.
    for inp in frontend.inputs() {
        let name = inp.name();
        if name == "audio_chunk" {
            continue;
        }
        let dims: Vec<usize> = inp
            .dtype()
            .tensor_shape()
            .map(|s| s.iter().map(|&d| if d <= 0 { 1 } else { d as usize }).collect())
            .unwrap_or_else(|| vec![1]);
        let elem_type = inp.dtype().tensor_type();
        let dyn_val: ort::value::DynValue = match elem_type {
            Some(ort::value::TensorElementType::Int64) => {
                let arr = ndarray::ArrayD::<i64>::zeros(ndarray::IxDyn(&dims));
                Tensor::from_array(arr)
                    .map_err(|e| format!("frontend state '{name}' i64 tensor error: {e}"))?
                    .into_dyn()
            }
            Some(ort::value::TensorElementType::Int32) => {
                let arr = ndarray::ArrayD::<i32>::zeros(ndarray::IxDyn(&dims));
                Tensor::from_array(arr)
                    .map_err(|e| format!("frontend state '{name}' i32 tensor error: {e}"))?
                    .into_dyn()
            }
            Some(ort::value::TensorElementType::Bool) => {
                let arr = ndarray::ArrayD::from_elem(ndarray::IxDyn(&dims), false);
                Tensor::from_array(arr)
                    .map_err(|e| format!("frontend state '{name}' bool tensor error: {e}"))?
                    .into_dyn()
            }
            _ => {
                let arr = ndarray::ArrayD::<f32>::zeros(ndarray::IxDyn(&dims));
                Tensor::from_array(arr)
                    .map_err(|e| format!("frontend state '{name}' f32 tensor error: {e}"))?
                    .into_dyn()
            }
        };
        fe_inputs.push((name.to_string(), dyn_val));
    }

    let fe_out = frontend
        .run(fe_inputs)
        .map_err(|e| format!("Moonshine streaming frontend failed: {e}"))?;
    let fe_first = fe_out
        .iter()
        .next()
        .map(|(_, v)| v)
        .ok_or_else(|| "Moonshine frontend produced no features".to_string())?;
    let features = match fe_out.get("features") {
        Some(v) => v.try_extract_array::<f32>(),
        None => fe_first.try_extract_array::<f32>(),
    }
    .map_err(|e| format!("features extract error: {e}"))?;
    let fshape = features.shape();
    let feat_arr = Array3::from_shape_vec(
        (fshape[0], fshape[1], fshape[2]),
        features.iter().copied().collect(),
    )
    .map_err(|e| format!("features shape error: {e}"))?;

    // 2. Encoder
    let enc_out = encoder
        .run(ort::inputs!["features" => Tensor::from_array(feat_arr).map_err(|e| format!("{e}"))?])
        .map_err(|e| format!("Moonshine streaming encoder failed: {e}"))?;
    let enc_val = enc_out
        .iter()
        .next()
        .map(|(_, v)| v)
        .ok_or_else(|| "encoder returned no output".to_string())?;
    let enc_arr_view = enc_val
        .try_extract_array::<f32>()
        .map_err(|e| format!("encoder extract error: {e}"))?;
    let eshape = enc_arr_view.shape();
    let enc_arr = Array3::from_shape_vec(
        (eshape[0], eshape[1], eshape[2]),
        enc_arr_view.iter().copied().collect(),
    )
    .map_err(|e| format!("encoded shape error: {e}"))?;

    // 3. Adapter (encoded + pos_offset=0)
    let pos_t = Tensor::from_array(Array1::from_vec(vec![0i64])).map_err(|e| format!("{e}"))?;
    let adapter_out = adapter
        .run(ort::inputs![
            "encoded" => Tensor::from_array(enc_arr).map_err(|e| format!("{e}"))?,
            "pos_offset" => pos_t,
        ])
        .map_err(|e| format!("Moonshine streaming adapter failed: {e}"))?;
    let mem_val = adapter_out
        .iter()
        .next()
        .map(|(_, v)| v)
        .ok_or_else(|| "adapter returned no memory".to_string())?;
    let mem_view = mem_val
        .try_extract_array::<f32>()
        .map_err(|e| format!("memory extract error: {e}"))?;
    let mshape = mem_view.shape();
    let mem_arr = Array3::from_shape_vec(
        (mshape[0], mshape[1], mshape[2]),
        mem_view.iter().copied().collect(),
    )
    .map_err(|e| format!("memory shape error: {e}"))?;

    // 4. Cross K/V precompute
    let ckv_out = cross_kv
        .run(ort::inputs!["memory" => Tensor::from_array(mem_arr).map_err(|e| format!("{e}"))?])
        .map_err(|e| format!("Moonshine streaming cross_kv failed: {e}"))?;
    let k_cross = ckv_out
        .get("k_cross")
        .ok_or_else(|| "cross_kv missing k_cross".to_string())?
        .try_extract_array::<f32>()
        .map_err(|e| format!("k_cross extract error: {e}"))?
        .to_owned();
    let v_cross = ckv_out
        .get("v_cross")
        .ok_or_else(|| "cross_kv missing v_cross".to_string())?
        .try_extract_array::<f32>()
        .map_err(|e| format!("v_cross extract error: {e}"))?
        .to_owned();

    // 5. Autoregressive decoder with self K/V cache
    let max_tokens = ((audio.len() as f32 / STT_SAMPLE_RATE as f32) * 6.0)
        .ceil()
        .clamp(16.0, MAX_DECODE_TOKENS as f32) as usize;
    let mut k_self =
        ndarray::Array5::<f32>::zeros((cfg.depth, 1, cfg.nheads, 0, cfg.head_dim)).into_dyn();
    let mut v_self =
        ndarray::Array5::<f32>::zeros((cfg.depth, 1, cfg.nheads, 0, cfg.head_dim)).into_dyn();
    let mut current_tok = cfg.bos_id;
    let mut out_tokens = vec![cfg.bos_id];

    let k_cross_name = decoder_kv
        .inputs()
        .iter()
        .find(|i| i.name() == "out_k_cross" || i.name() == "k_cross")
        .map(|i| i.name().to_string())
        .unwrap_or_else(|| "out_k_cross".to_string());
    let v_cross_name = decoder_kv
        .inputs()
        .iter()
        .find(|i| i.name() == "out_v_cross" || i.name() == "v_cross")
        .map(|i| i.name().to_string())
        .unwrap_or_else(|| "out_v_cross".to_string());

    for _ in 0..max_tokens {
        let tok_t = Tensor::from_array(
            Array2::from_shape_vec((1, 1), vec![current_tok]).map_err(|e| format!("{e}"))?,
        )
        .map_err(|e| format!("{e}"))?;
        let dec_inputs: Vec<(String, ort::value::DynValue)> = vec![
            ("token".to_string(), tok_t.into_dyn()),
            (
                "k_self".to_string(),
                Tensor::from_array(k_self.clone())
                    .map_err(|e| format!("{e}"))?
                    .into_dyn(),
            ),
            (
                "v_self".to_string(),
                Tensor::from_array(v_self.clone())
                    .map_err(|e| format!("{e}"))?
                    .into_dyn(),
            ),
            (
                k_cross_name.clone(),
                Tensor::from_array(k_cross.clone())
                    .map_err(|e| format!("{e}"))?
                    .into_dyn(),
            ),
            (
                v_cross_name.clone(),
                Tensor::from_array(v_cross.clone())
                    .map_err(|e| format!("{e}"))?
                    .into_dyn(),
            ),
        ];
        let dec_out = decoder_kv
            .run(dec_inputs)
            .map_err(|e| format!("Moonshine streaming decoder failed: {e}"))?;

        let dec_first = dec_out
            .iter()
            .next()
            .map(|(_, v)| v)
            .ok_or_else(|| "decoder_kv missing logits".to_string())?;
        let logits = match dec_out.get("logits") {
            Some(v) => v.try_extract_array::<f32>(),
            None => dec_first.try_extract_array::<f32>(),
        }
        .map_err(|e| format!("logits extract error: {e}"))?;
        let lshape = logits.shape();
        let vocab_size = lshape[lshape.len() - 1];
        let slice = logits.as_slice().unwrap_or(&[]);
        let next_tok = argmax(&slice[slice.len().saturating_sub(vocab_size)..]) as i64;

        if next_tok == cfg.eos_id {
            break;
        }
        out_tokens.push(next_tok);
        current_tok = next_tok;

        if let Some(ks) = dec_out.get("out_k_self") {
            k_self = ks
                .try_extract_array::<f32>()
                .map_err(|e| format!("out_k_self error: {e}"))?
                .to_owned();
        }
        if let Some(vs) = dec_out.get("out_v_self") {
            v_self = vs
                .try_extract_array::<f32>()
                .map_err(|e| format!("out_v_self error: {e}"))?
                .to_owned();
        }
    }

    Ok(out_tokens)
}

fn argmax(values: &[f32]) -> usize {
    let mut best_idx = 0;
    let mut best_val = f32::NEG_INFINITY;
    for (idx, &v) in values.iter().enumerate() {
        if v > best_val {
            best_val = v;
            best_idx = idx;
        }
    }
    best_idx
}

/// Linearly resamples mono audio to `target_rate` when capture rate differs from 16 kHz.
fn resample_linear(samples: &[f32], src_rate: u32, dst_rate: u32) -> Vec<f32> {
    if samples.is_empty() || src_rate == dst_rate {
        return samples.to_vec();
    }
    let ratio = (src_rate as f64) / (dst_rate as f64);
    let out_len = ((samples.len() as f64) / ratio).round().max(1.0) as usize;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let pos = (i as f64) * ratio;
        let idx = (pos.floor() as usize).min(samples.len() - 1);
        let next = (idx + 1).min(samples.len() - 1);
        let frac = (pos - (idx as f64)) as f32;
        out.push(samples[idx] * (1.0 - frac) + samples[next] * frac);
    }
    out
}

/// Parses `model.vocab` + `added_tokens` from Hugging Face `tokenizer.json`.
pub fn load_id_to_token(path: &Path) -> Result<HashMap<i64, String>, String> {
    let body = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read tokenizer.json: {e}"))?;
    let root: serde_json::Value =
        serde_json::from_str(&body).map_err(|e| format!("invalid tokenizer.json: {e}"))?;
    let mut map = HashMap::new();

    if let Some(vocab) = root.get("model").and_then(|m| m.get("vocab")) {
        if let Some(obj) = vocab.as_object() {
            for (tok, id) in obj {
                if let Some(n) = id.as_i64() {
                    map.insert(n, tok.clone());
                }
            }
        } else if let Some(arr) = vocab.as_array() {
            for (idx, entry) in arr.iter().enumerate() {
                if let Some(tok) = entry
                    .as_array()
                    .and_then(|pair| pair.first())
                    .and_then(|v| v.as_str())
                {
                    map.insert(idx as i64, tok.to_string());
                }
            }
        }
    }

    if let Some(added) = root.get("added_tokens").and_then(|v| v.as_array()) {
        for item in added {
            if let (Some(id), Some(content)) = (
                item.get("id").and_then(|v| v.as_i64()),
                item.get("content").and_then(|v| v.as_str()),
            ) {
                map.insert(id, content.to_string());
            }
        }
    }

    if map.is_empty() {
        return Err("tokenizer.json contains no vocabulary entries".to_string());
    }
    Ok(map)
}

/// Decodes SentencePiece / BPE token IDs into clean UTF-8 text.
pub fn decode_token_ids(ids: &[i64], id_to_token: &HashMap<i64, String>) -> String {
    let mut raw = String::new();
    for &id in ids {
        let Some(tok) = id_to_token.get(&id) else {
            continue;
        };
        if (tok.starts_with('<') && tok.ends_with('>')) || tok == "[PAD]" {
            continue;
        }
        raw.push_str(tok);
    }
    raw.replace(['▁', 'Ġ'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_token_ids_strips_specials_and_joins_sentencepiece() {
        let mut map = HashMap::new();
        map.insert(1, "<s>".to_string());
        map.insert(2, "</s>".to_string());
        map.insert(10, "▁Hola".to_string());
        map.insert(11, "▁mundo".to_string());
        map.insert(12, "!".to_string());
        let text = decode_token_ids(&[1, 10, 11, 12, 2], &map);
        assert_eq!(text, "Hola mundo!");
    }

    #[test]
    fn resample_linear_halves_length_from_32k_to_16k() {
        let input = vec![0.5f32; 3200];
        let out = resample_linear(&input, 32_000, 16_000);
        assert_eq!(out.len(), 1600);
    }

    #[test]
    #[ignore]
    fn live_moonshine_small_es_runs_on_silence() {
        let home = std::env::var("HOME").expect("HOME");
        let data_dir = std::path::PathBuf::from(home)
            .join("Library/Application Support/com.ipepio.nabla");
        let engine = VoiceSttEngine::new();
        let samples = vec![0.0f32; 16_000]; // 1 second of silence
        let res = engine
            .transcribe_samples(&data_dir, "moonshine-small-es", &samples, 16_000)
            .expect("moonshine-small-es transcribe");
        assert_eq!(res.duration_ms, 1000);
    }
}
