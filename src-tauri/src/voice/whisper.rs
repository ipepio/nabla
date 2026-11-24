//! Embedded local CPU Speech-to-Text via ONNX Runtime (`ort`) for OpenAI Whisper
//! (`onnx-community/whisper-small` and `onnx-community/whisper-large-v3-turbo`).
//!
//! Pipeline:
//! 1. Pure-Rust 16 kHz Slaney Log-Mel Spectrogram (`[1, n_mels, 3000]`, 80 or 128 bins).
//! 2. `encoder_model_*.onnx`: mel features -> `encoder_hidden_states`.
//! 3. `decoder_model_merged_*.onnx`: autoregressive KV-cached greedy decoding with
//!    automatic language detection on step 0 and GPT-2 byte-level BPE decoding.

use std::collections::HashMap;
use std::f32::consts::PI;
use std::path::{Path, PathBuf};

use ndarray::{Array1, Array2, Array3, ArrayD, IxDyn};
use ort::environment::Environment;
use ort::session::{Session, builder::GraphOptimizationLevel};
use ort::value::{DynValue, Tensor};

const WHISPER_SAMPLE_RATE: usize = 16_000;
const WHISPER_N_FFT: usize = 400;
const WHISPER_HOP_LENGTH: usize = 160;
const WHISPER_CHUNK_SAMPLES: usize = 30 * WHISPER_SAMPLE_RATE; // 480_000
const WHISPER_N_FRAMES: usize = 3000;
const WHISPER_MAX_TOKENS: usize = 224;

struct WhisperTokens {
    sot: i64,
    eot: i64,
    transcribe: i64,
    notimestamps: i64,
    lang_es: i64,
    lang_ids: Vec<i64>,
    id_to_piece: HashMap<i64, String>,
    special_ids: std::collections::HashSet<i64>,
}

pub struct LoadedWhisper {
    encoder: Session,
    decoder: Session,
    n_mels: usize,
    tokens: WhisperTokens,
    mel_filters: Vec<Vec<f32>>,
    hann: Vec<f32>,
    cos_table: Vec<f32>,
    sin_table: Vec<f32>,
}

impl LoadedWhisper {
    pub fn load(env: &Environment, base: &Path, threads: usize) -> Result<Self, String> {
        let encoder_path = resolve_whisper_file(
            base,
            &[
                "onnx/encoder_model_quantized.onnx",
                "onnx/encoder_model_q4.onnx",
                "onnx/encoder_model.onnx",
                "encoder_model.onnx",
            ],
            "Whisper encoder ONNX",
        )?;
        let decoder_path = resolve_whisper_file(
            base,
            &[
                "onnx/decoder_model_merged_quantized.onnx",
                "onnx/decoder_model_merged_q4.onnx",
                "onnx/decoder_model_merged.onnx",
                "decoder_model_merged.onnx",
            ],
            "Whisper merged decoder ONNX",
        )?;

        let build = |path: &Path| -> Result<Session, String> {
            Session::builder(env)
                .map_err(|e| format!("Whisper ORT builder error: {e}"))?
                .with_optimization_level(GraphOptimizationLevel::Level3)
                .map_err(|e| format!("Whisper ORT opt error: {e}"))?
                .with_intra_threads(threads)
                .map_err(|e| format!("Whisper ORT threads error: {e}"))?
                .commit_from_file(path)
                .map_err(|e| format!("failed to load {}: {e}", path.display()))
        };

        let encoder = build(&encoder_path)?;
        let decoder = build(&decoder_path)?;
        let n_mels = detect_n_mels(base, &encoder);
        let tokens = load_whisper_tokens(&base.join("tokenizer.json"))?;

        let n_freqs = WHISPER_N_FFT / 2 + 1;
        let mel_filters = build_slaney_mel_filters(n_mels, n_freqs, WHISPER_SAMPLE_RATE as f32);
        let hann: Vec<f32> = (0..WHISPER_N_FFT)
            .map(|n| 0.5 * (1.0 - (2.0 * PI * (n as f32) / (WHISPER_N_FFT as f32)).cos()))
            .collect();
        let mut cos_table = Vec::with_capacity(n_freqs * WHISPER_N_FFT);
        let mut sin_table = Vec::with_capacity(n_freqs * WHISPER_N_FFT);
        for k in 0..n_freqs {
            let factor = 2.0 * PI * (k as f32) / (WHISPER_N_FFT as f32);
            for n in 0..WHISPER_N_FFT {
                let angle = factor * (n as f32);
                cos_table.push(angle.cos());
                sin_table.push(angle.sin());
            }
        }

        Ok(Self {
            encoder,
            decoder,
            n_mels,
            tokens,
            mel_filters,
            hann,
            cos_table,
            sin_table,
        })
    }

    pub fn transcribe(&mut self, audio_16k: &[f32]) -> Result<String, String> {
        // Process audio in 30-second windows (standard Whisper chunking)
        let mut parts = Vec::new();
        let mut offset = 0usize;
        while offset < audio_16k.len() {
            let end = (offset + WHISPER_CHUNK_SAMPLES).min(audio_16k.len());
            let chunk = &audio_16k[offset..end];
            let text = self.transcribe_chunk(chunk)?;
            if !text.is_empty() {
                parts.push(text);
            }
            offset = end;
        }
        Ok(parts.join(" "))
    }

    fn transcribe_chunk(&mut self, chunk: &[f32]) -> Result<String, String> {
        let mel = self.compute_log_mel(chunk);
        let mel_tensor =
            Tensor::from_array(mel).map_err(|e| format!("Whisper mel tensor error: {e}"))?;

        let enc_out = self
            .encoder
            .run(ort::inputs!["input_features" => mel_tensor])
            .map_err(|e| format!("Whisper encoder failed: {e}"))?;
        let (_, enc_val) = enc_out
            .iter()
            .next()
            .ok_or_else(|| "Whisper encoder returned no hidden states".to_string())?;
        let encoder_hidden = enc_val
            .try_extract_array::<f32>()
            .map_err(|e| format!("Whisper encoder extract error: {e}"))?
            .to_owned();
        drop(enc_out);

        let mut kv_cache = build_empty_whisper_kv(&self.decoder);

        // Step 0: feed <|startoftranscript|> with use_cache_branch = false to populate
        // both encoder cross-attention KV cache and decoder self-attention KV cache,
        // and detect spoken language from the language token logits.
        let (logits0, next_kv0) = run_whisper_decoder_step(
            &mut self.decoder,
            &[self.tokens.sot],
            &encoder_hidden,
            &kv_cache,
            false,
        )?;
        kv_cache = next_kv0;

        let lang_token = pick_best_language_token(&logits0, &self.tokens);

        // Step 1: feed detected language token
        let (_, next_kv1) = run_whisper_decoder_step(
            &mut self.decoder,
            &[lang_token],
            &encoder_hidden,
            &kv_cache,
            true,
        )?;
        merge_decoder_kv(&mut kv_cache, next_kv1);

        // Step 2: feed <|transcribe|>
        let (_, next_kv2) = run_whisper_decoder_step(
            &mut self.decoder,
            &[self.tokens.transcribe],
            &encoder_hidden,
            &kv_cache,
            true,
        )?;
        merge_decoder_kv(&mut kv_cache, next_kv2);

        // Step 3: feed <|notimestamps|> -> logits predict first text token
        let (mut logits, next_kv3) = run_whisper_decoder_step(
            &mut self.decoder,
            &[self.tokens.notimestamps],
            &encoder_hidden,
            &kv_cache,
            true,
        )?;
        merge_decoder_kv(&mut kv_cache, next_kv3);

        let mut generated: Vec<i64> = Vec::new();
        for _ in 0..WHISPER_MAX_TOKENS {
            let next_id = argmax_f32(&logits);
            if next_id == self.tokens.eot {
                break;
            }
            // Prevent degenerate infinite repetition
            if generated.len() >= 8
                && generated[generated.len() - 4..].iter().all(|&id| id == next_id)
            {
                break;
            }
            generated.push(next_id);

            let (next_logits, next_kv) = run_whisper_decoder_step(
                &mut self.decoder,
                &[next_id],
                &encoder_hidden,
                &kv_cache,
                true,
            )?;
            merge_decoder_kv(&mut kv_cache, next_kv);
            logits = next_logits;
        }

        Ok(decode_whisper_bpe(&generated, &self.tokens))
    }

    /// Computes OpenAI Whisper log-mel spectrogram `[1, n_mels, 3000]`.
    fn compute_log_mel(&self, audio: &[f32]) -> Array3<f32> {
        let mut padded = vec![0.0f32; WHISPER_CHUNK_SAMPLES];
        let copy_len = audio.len().min(WHISPER_CHUNK_SAMPLES);
        padded[..copy_len].copy_from_slice(&audio[..copy_len]);

        let pad = WHISPER_N_FFT / 2;
        let mut reflect = vec![0.0f32; WHISPER_CHUNK_SAMPLES + 2 * pad];
        for i in 0..pad {
            reflect[i] = padded[(pad - i).min(WHISPER_CHUNK_SAMPLES - 1)];
        }
        reflect[pad..pad + WHISPER_CHUNK_SAMPLES].copy_from_slice(&padded);
        for i in 0..pad {
            reflect[pad + WHISPER_CHUNK_SAMPLES + i] =
                padded[WHISPER_CHUNK_SAMPLES.saturating_sub(2 + i)];
        }

        let n_freqs = WHISPER_N_FFT / 2 + 1;
        let mut mel_spec = vec![0.0f32; self.n_mels * WHISPER_N_FRAMES];
        let mut frame_buf = [0.0f32; WHISPER_N_FFT];
        let mut mag_sq = vec![0.0f32; n_freqs];

        // Only compute real STFT for frames covering the non-zero audio region
        let active_frames = (copy_len.div_ceil(WHISPER_HOP_LENGTH) + 2).min(WHISPER_N_FRAMES);

        let mut max_log = f32::NEG_INFINITY;
        for f in 0..WHISPER_N_FRAMES {
            if f < active_frames {
                let start = f * WHISPER_HOP_LENGTH;
                for n in 0..WHISPER_N_FFT {
                    frame_buf[n] = reflect[start + n] * self.hann[n];
                }
                for (k, mag) in mag_sq.iter_mut().enumerate() {
                    let base = k * WHISPER_N_FFT;
                    let mut re = 0.0f32;
                    let mut im = 0.0f32;
                    for n in 0..WHISPER_N_FFT {
                        let x = frame_buf[n];
                        re += x * self.cos_table[base + n];
                        im -= x * self.sin_table[base + n];
                    }
                    *mag = re * re + im * im;
                }
                for m in 0..self.n_mels {
                    let filter = &self.mel_filters[m];
                    let mut energy = 0.0f32;
                    for (k, &w) in filter.iter().enumerate() {
                        if w != 0.0 {
                            energy += w * mag_sq[k];
                        }
                    }
                    let v = energy.max(1e-10).log10();
                    if v > max_log {
                        max_log = v;
                    }
                    mel_spec[m * WHISPER_N_FRAMES + f] = v;
                }
            } else {
                let v = -10.0f32;
                if v > max_log {
                    max_log = v;
                }
                for m in 0..self.n_mels {
                    mel_spec[m * WHISPER_N_FRAMES + f] = v;
                }
            }
        }

        let floor = max_log - 8.0;
        for v in &mut mel_spec {
            *v = (v.max(floor) + 4.0) / 4.0;
        }

        Array3::from_shape_vec((1, self.n_mels, WHISPER_N_FRAMES), mel_spec)
            .expect("mel spectrogram shape")
    }
}

fn build_empty_whisper_kv(decoder: &Session) -> HashMap<String, ArrayD<f32>> {
    let mut map = HashMap::new();
    for inp in decoder.inputs() {
        let name = inp.name();
        if !name.starts_with("past_key_values.") {
            continue;
        }
        let dims: Vec<usize> = inp
            .dtype()
            .tensor_shape()
            .map(|s| {
                s.iter()
                    .enumerate()
                    .map(|(idx, &d)| {
                        if d > 0 {
                            d as usize
                        } else if idx == 0 {
                            1
                        } else {
                            0
                        }
                    })
                    .collect()
            })
            .unwrap_or_else(|| vec![1, 12, 0, 64]);
        map.insert(name.to_string(), ArrayD::<f32>::zeros(IxDyn(&dims)));
    }
    map
}

fn merge_decoder_kv(
    cache: &mut HashMap<String, ArrayD<f32>>,
    next: HashMap<String, ArrayD<f32>>,
) {
    for (k, v) in next {
        // When `use_cache_branch = true`, Whisper merged decoder only updates `.decoder.` KV;
        // `.encoder.` outputs may be empty placeholders, so keep the step-0 encoder KV intact.
        if k.contains(".encoder.") && v.iter().next().is_none() {
            continue;
        }
        cache.insert(k, v);
    }
}

type WhisperDecodeStep = (Vec<f32>, HashMap<String, ArrayD<f32>>);

fn run_whisper_decoder_step(
    decoder: &mut Session,
    input_ids: &[i64],
    encoder_hidden: &ArrayD<f32>,
    kv_cache: &HashMap<String, ArrayD<f32>>,
    use_cache: bool,
) -> Result<WhisperDecodeStep, String> {
    let ids_arr = Array2::from_shape_vec((1, input_ids.len()), input_ids.to_vec())
        .map_err(|e| format!("Whisper input_ids shape error: {e}"))?;
    let use_cache_arr = Array1::from_vec(vec![use_cache]);

    let mut inputs: Vec<(String, DynValue)> = Vec::with_capacity(kv_cache.len() + 3);
    for inp in decoder.inputs() {
        let name = inp.name();
        let val: DynValue = match name {
            "input_ids" => Tensor::from_array(ids_arr.clone())
                .map_err(|e| format!("{e}"))?
                .into_dyn(),
            "encoder_hidden_states" => Tensor::from_array(encoder_hidden.clone())
                .map_err(|e| format!("{e}"))?
                .into_dyn(),
            "use_cache_branch" => Tensor::from_array(use_cache_arr.clone())
                .map_err(|e| format!("{e}"))?
                .into_dyn(),
            other => {
                let arr = kv_cache
                    .get(other)
                    .ok_or_else(|| format!("missing Whisper KV input '{other}'"))?;
                Tensor::from_array(arr.clone())
                    .map_err(|e| format!("{e}"))?
                    .into_dyn()
            }
        };
        inputs.push((name.to_string(), val));
    }

    let outputs = decoder
        .run(inputs)
        .map_err(|e| format!("Whisper decoder step failed: {e}"))?;

    let mut last_logits = Vec::new();
    let mut next_kv = HashMap::new();
    for (name, val) in outputs.iter() {
        if name == "logits" {
            let view = val
                .try_extract_array::<f32>()
                .map_err(|e| format!("Whisper logits extract error: {e}"))?;
            let sh = view.shape();
            let vocab = *sh.last().unwrap_or(&0);
            let flat: Vec<f32> = view.iter().copied().collect();
            if vocab > 0 && flat.len() >= vocab {
                last_logits = flat[flat.len() - vocab..].to_vec();
            }
        } else if let Some(rest) = name.strip_prefix("present.") {
            let arr = val
                .try_extract_array::<f32>()
                .map_err(|e| format!("Whisper extract '{name}' error: {e}"))?
                .to_owned();
            next_kv.insert(format!("past_key_values.{rest}"), arr);
        }
    }

    Ok((last_logits, next_kv))
}

fn pick_best_language_token(logits: &[f32], tokens: &WhisperTokens) -> i64 {
    let mut best_id = tokens.lang_es;
    let mut best_val = f32::NEG_INFINITY;
    for &id in &tokens.lang_ids {
        if let Some(&v) = logits.get(id as usize)
            && v > best_val
        {
            best_val = v;
            best_id = id;
        }
    }
    best_id
}

fn argmax_f32(slice: &[f32]) -> i64 {
    let mut best_idx = 0usize;
    let mut best_val = f32::NEG_INFINITY;
    for (i, &v) in slice.iter().enumerate() {
        if v > best_val {
            best_val = v;
            best_idx = i;
        }
    }
    best_idx as i64
}

fn detect_n_mels(base: &Path, encoder: &Session) -> usize {
    if let Some(inp) = encoder.inputs().first()
        && let Some(sh) = inp.dtype().tensor_shape()
        && let Some(&dim1) = sh.get(1)
        && (dim1 == 80 || dim1 == 128)
    {
        return dim1 as usize;
    }
    if let Ok(body) = std::fs::read_to_string(base.join("preprocessor_config.json"))
        && let Ok(root) = serde_json::from_str::<serde_json::Value>(&body)
        && let Some(n) = root.get("feature_size").and_then(|v| v.as_u64())
        && (n == 80 || n == 128)
    {
        return n as usize;
    }
    80
}

fn hz_to_mel_slaney(hz: f32) -> f32 {
    let f_min = 0.0f32;
    let f_sp = 200.0 / 3.0;
    let min_log_hz = 1000.0;
    let min_log_mel = (min_log_hz - f_min) / f_sp;
    let logstep = (6.4f32).ln() / 27.0;
    if hz >= min_log_hz {
        min_log_mel + (hz / min_log_hz).ln() / logstep
    } else {
        (hz - f_min) / f_sp
    }
}

fn mel_to_hz_slaney(mel: f32) -> f32 {
    let f_min = 0.0f32;
    let f_sp = 200.0 / 3.0;
    let min_log_hz = 1000.0;
    let min_log_mel = (min_log_hz - f_min) / f_sp;
    let logstep = (6.4f32).ln() / 27.0;
    if mel >= min_log_mel {
        min_log_hz * (logstep * (mel - min_log_mel)).exp()
    } else {
        f_min + f_sp * mel
    }
}

fn build_slaney_mel_filters(n_mels: usize, n_freqs: usize, sr: f32) -> Vec<Vec<f32>> {
    let mel_min = hz_to_mel_slaney(0.0);
    let mel_max = hz_to_mel_slaney(sr / 2.0);
    let mel_pts: Vec<f32> = (0..=(n_mels + 1))
        .map(|i| mel_min + (mel_max - mel_min) * (i as f32) / ((n_mels + 1) as f32))
        .collect();
    let hz_pts: Vec<f32> = mel_pts.into_iter().map(mel_to_hz_slaney).collect();
    let fft_freqs: Vec<f32> = (0..n_freqs)
        .map(|k| (k as f32) * (sr / 2.0) / ((n_freqs - 1) as f32))
        .collect();

    let mut filters = vec![vec![0.0f32; n_freqs]; n_mels];
    for m in 0..n_mels {
        let f_left = hz_pts[m];
        let f_center = hz_pts[m + 1];
        let f_right = hz_pts[m + 2];
        let enorm = 2.0 / (f_right - f_left).max(1e-6);
        for (k, &f) in fft_freqs.iter().enumerate() {
            let w = if f >= f_left && f <= f_center {
                (f - f_left) / (f_center - f_left).max(1e-6)
            } else if f > f_center && f <= f_right {
                (f_right - f) / (f_right - f_center).max(1e-6)
            } else {
                0.0
            };
            filters[m][k] = w * enorm;
        }
    }
    filters
}

fn load_whisper_tokens(path: &Path) -> Result<WhisperTokens, String> {
    let body = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read Whisper tokenizer.json: {e}"))?;
    let root: serde_json::Value = serde_json::from_str(&body)
        .map_err(|e| format!("invalid Whisper tokenizer.json: {e}"))?;

    let mut id_to_piece = HashMap::new();
    let mut piece_to_id = HashMap::new();
    let mut special_ids = std::collections::HashSet::new();

    if let Some(vocab) = root
        .get("model")
        .and_then(|m| m.get("vocab"))
        .and_then(|v| v.as_object())
    {
        for (tok, id_val) in vocab {
            if let Some(id) = id_val.as_i64() {
                id_to_piece.insert(id, tok.clone());
                piece_to_id.insert(tok.clone(), id);
            }
        }
    }
    if let Some(added) = root.get("added_tokens").and_then(|v| v.as_array()) {
        for item in added {
            let Some(id) = item.get("id").and_then(|v| v.as_i64()) else {
                continue;
            };
            let Some(content) = item.get("content").and_then(|v| v.as_str()) else {
                continue;
            };
            id_to_piece.insert(id, content.to_string());
            piece_to_id.insert(content.to_string(), id);
            if item.get("special").and_then(|v| v.as_bool()).unwrap_or(false)
                || (content.starts_with("<|") && content.ends_with("|>"))
            {
                special_ids.insert(id);
            }
        }
    }

    let sot = *piece_to_id.get("<|startoftranscript|>").unwrap_or(&50258);
    let eot = *piece_to_id.get("<|endoftext|>").unwrap_or(&50257);
    let transcribe = *piece_to_id.get("<|transcribe|>").unwrap_or(&50359);
    let notimestamps = *piece_to_id.get("<|notimestamps|>").unwrap_or(&50363);
    let lang_es = *piece_to_id.get("<|es|>").unwrap_or(&50262);

    let mut lang_ids = Vec::new();
    for code in ["es", "en", "fr", "de", "pt", "it", "ca", "gl", "eu"] {
        let key = format!("<|{code}|>");
        if let Some(&id) = piece_to_id.get(&key) {
            lang_ids.push(id);
        }
    }

    Ok(WhisperTokens {
        sot,
        eot,
        transcribe,
        notimestamps,
        lang_es,
        lang_ids,
        id_to_piece,
        special_ids,
    })
}

/// Decodes GPT-2 byte-level BPE pieces (used by Whisper) into UTF-8 text.
fn decode_whisper_bpe(ids: &[i64], tokens: &WhisperTokens) -> String {
    let byte_decoder = gpt2_unicode_to_byte();
    let mut raw_bytes = Vec::new();
    for &id in ids {
        if tokens.special_ids.contains(&id) {
            continue;
        }
        let Some(piece) = tokens.id_to_piece.get(&id) else {
            continue;
        };
        if piece.starts_with("<|") && piece.ends_with("|>") {
            continue;
        }
        for ch in piece.chars() {
            if let Some(&b) = byte_decoder.get(&ch) {
                raw_bytes.push(b);
            } else {
                let mut buf = [0u8; 4];
                raw_bytes.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
            }
        }
    }
    String::from_utf8_lossy(&raw_bytes)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn gpt2_unicode_to_byte() -> HashMap<char, u8> {
    let mut bs: Vec<u8> = Vec::new();
    bs.extend(b'!'..=b'~');
    bs.extend(0xA1u8..=0xACu8);
    bs.extend(0xAEu8..=0xFFu8);
    let mut cs: Vec<u32> = bs.iter().map(|&b| b as u32).collect();
    let mut n = 0u32;
    for b in 0u8..=255 {
        if !bs.contains(&b) {
            bs.push(b);
            cs.push(256 + n);
            n += 1;
        }
    }
    let mut map = HashMap::with_capacity(256);
    for (b, c) in bs.into_iter().zip(cs) {
        if let Some(ch) = char::from_u32(c) {
            map.insert(ch, b);
        }
    }
    map
}

fn resolve_whisper_file(base: &Path, candidates: &[&str], label: &str) -> Result<PathBuf, String> {
    candidates
        .iter()
        .map(|rel| base.join(rel))
        .find(|p| p.is_file())
        .ok_or_else(|| format!("{label} not found — download the model in Settings → Voice."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpt2_bpe_decoder_recovers_utf8_spanish_accents() {
        let mut id_to_piece = HashMap::new();
        // "ĠespaÃ±ol" in GPT-2 byte-level BPE -> " español"
        id_to_piece.insert(10, "ĠespaÃ±ol".to_string());
        id_to_piece.insert(50257, "<|endoftext|>".to_string());
        let mut special_ids = std::collections::HashSet::new();
        special_ids.insert(50257);
        let tokens = WhisperTokens {
            sot: 50258,
            eot: 50257,
            transcribe: 50359,
            notimestamps: 50363,
            lang_es: 50262,
            lang_ids: vec![50262],
            id_to_piece,
            special_ids,
        };
        assert_eq!(decode_whisper_bpe(&[10, 50257], &tokens), "español");
    }

    #[test]
    fn slaney_mel_filterbank_has_expected_dimensions() {
        let f80 = build_slaney_mel_filters(80, 201, 16_000.0);
        assert_eq!(f80.len(), 80);
        assert_eq!(f80[0].len(), 201);
        let f128 = build_slaney_mel_filters(128, 201, 16_000.0);
        assert_eq!(f128.len(), 128);
    }
}
