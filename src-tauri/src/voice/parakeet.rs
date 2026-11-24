//! Embedded local CPU Speech-to-Text via ONNX Runtime (`ort`) for NVIDIA
//! Parakeet TDT 0.6B v2 (`istupakov/parakeet-tdt-0.6b-v2-onnx`).
//!
//! Pipeline:
//! 1. `nemo128.onnx`: 16 kHz PCM waveform -> 128-bin NeMo log-mel features `[1, 128, T]`.
//! 2. `encoder-model.int8.onnx`: Conformer encoder -> `outputs [1, D, T_enc]`.
//! 3. `decoder_joint-model.int8.onnx`: greedy Token-and-Duration Transducer (TDT)
//!    decoding with LSTM state cache (`input_states_1`, `input_states_2`) + `vocab.txt`.

use std::path::{Path, PathBuf};

use ndarray::{Array1, Array2, Array3, ArrayD, IxDyn};
use ort::environment::Environment;
use ort::session::{Session, builder::GraphOptimizationLevel};
use ort::value::{DynValue, Tensor};

const TDT_DURATIONS: [usize; 5] = [0, 1, 2, 3, 4];
const MAX_SYMBOLS_PER_STEP: usize = 10;

pub struct LoadedParakeet {
    preprocessor: Session,
    encoder: Session,
    decoder_joint: Session,
    vocab: Vec<String>,
    blank_id: usize,
    state1_shape: Vec<usize>,
    state2_shape: Vec<usize>,
}

impl LoadedParakeet {
    pub fn load(env: &Environment, base: &Path, threads: usize) -> Result<Self, String> {
        let pre_path = resolve_file(base, &["nemo128.onnx"], "Parakeet nemo128.onnx")?;
        let enc_path = resolve_file(
            base,
            &["encoder-model.int8.onnx", "encoder-model.onnx"],
            "Parakeet encoder ONNX",
        )?;
        let dec_path = resolve_file(
            base,
            &["decoder_joint-model.int8.onnx", "decoder_joint-model.onnx"],
            "Parakeet decoder_joint ONNX",
        )?;
        let vocab_path = resolve_file(base, &["vocab.txt"], "Parakeet vocab.txt")?;

        let build = |path: &Path| -> Result<Session, String> {
            Session::builder(env)
                .map_err(|e| format!("Parakeet ORT builder error: {e}"))?
                .with_optimization_level(GraphOptimizationLevel::Level3)
                .map_err(|e| format!("Parakeet ORT opt error: {e}"))?
                .with_intra_threads(threads)
                .map_err(|e| format!("Parakeet ORT threads error: {e}"))?
                .commit_from_file(path)
                .map_err(|e| format!("failed to load {}: {e}", path.display()))
        };

        let preprocessor = build(&pre_path)?;
        let encoder = build(&enc_path)?;
        let decoder_joint = build(&dec_path)?;
        let (vocab, blank_id) = load_parakeet_vocab(&vocab_path)?;

        let state1_shape = infer_state_shape(&decoder_joint, "input_states_1", &[2, 1, 640]);
        let state2_shape = infer_state_shape(&decoder_joint, "input_states_2", &[2, 1, 640]);

        Ok(Self {
            preprocessor,
            encoder,
            decoder_joint,
            vocab,
            blank_id,
            state1_shape,
            state2_shape,
        })
    }

    pub fn transcribe(&mut self, audio_16k: &[f32]) -> Result<String, String> {
        // 1. NeMo 128-mel feature extraction
        let wav_len = audio_16k.len();
        let wav_arr = Array2::from_shape_vec((1, wav_len), audio_16k.to_vec())
            .map_err(|e| format!("Parakeet waveform shape error: {e}"))?;
        let wav_lens_arr = Array1::from_vec(vec![wav_len as i64]);

        let (features, feat_len) = {
            let mut pre_inputs: Vec<(String, DynValue)> = Vec::with_capacity(2);
            for (idx, inp) in self.preprocessor.inputs().iter().enumerate() {
                let val: DynValue = if idx == 0 {
                    Tensor::from_array(wav_arr.clone())
                        .map_err(|e| format!("{e}"))?
                        .into_dyn()
                } else {
                    Tensor::from_array(wav_lens_arr.clone())
                        .map_err(|e| format!("{e}"))?
                        .into_dyn()
                };
                pre_inputs.push((inp.name().to_string(), val));
            }
            let pre_out = self
                .preprocessor
                .run(pre_inputs)
                .map_err(|e| format!("Parakeet nemo128 failed: {e}"))?;
            let all: Vec<_> = pre_out.iter().collect();
            if all.len() < 2 {
                return Err("Parakeet nemo128 returned fewer than 2 outputs".to_string());
            }
            let feat = all[0]
                .1
                .try_extract_array::<f32>()
                .map_err(|e| format!("Parakeet features extract error: {e}"))?
                .to_owned();
            let flen = all[1]
                .1
                .try_extract_array::<i64>()
                .map_err(|e| format!("Parakeet features_lens extract error: {e}"))?
                .iter()
                .next()
                .copied()
                .unwrap_or(0);
            (feat, flen)
        };

        // 2. Conformer encoder
        let (enc_outputs, enc_len) = {
            let flen_arr = Array1::from_vec(vec![feat_len]);
            let mut enc_inputs: Vec<(String, DynValue)> = Vec::with_capacity(2);
            for (idx, inp) in self.encoder.inputs().iter().enumerate() {
                let val: DynValue = if idx == 0 {
                    Tensor::from_array(features.clone())
                        .map_err(|e| format!("{e}"))?
                        .into_dyn()
                } else {
                    Tensor::from_array(flen_arr.clone())
                        .map_err(|e| format!("{e}"))?
                        .into_dyn()
                };
                enc_inputs.push((inp.name().to_string(), val));
            }
            let enc_out = self
                .encoder
                .run(enc_inputs)
                .map_err(|e| format!("Parakeet encoder failed: {e}"))?;
            let all: Vec<_> = enc_out.iter().collect();
            if all.len() < 2 {
                return Err("Parakeet encoder returned fewer than 2 outputs".to_string());
            }
            let eo = all[0]
                .1
                .try_extract_array::<f32>()
                .map_err(|e| format!("Parakeet encoder outputs extract error: {e}"))?
                .to_owned();
            let elen = all[1]
                .1
                .try_extract_array::<i64>()
                .map_err(|e| format!("Parakeet encoded_lengths extract error: {e}"))?
                .iter()
                .next()
                .copied()
                .unwrap_or(0) as usize;
            (eo, elen)
        };

        // enc_outputs shape: [1, enc_dim, total_t]
        let sh = enc_outputs.shape();
        if sh.len() != 3 {
            return Err(format!("unexpected Parakeet encoder output shape {sh:?}"));
        }
        let enc_dim = sh[1];
        let total_t = enc_len.min(sh[2]);
        if total_t == 0 {
            return Ok(String::new());
        }

        // 3. Greedy TDT decoding loop
        let mut state1 = ArrayD::<f32>::zeros(IxDyn(&self.state1_shape));
        let mut state2 = ArrayD::<f32>::zeros(IxDyn(&self.state2_shape));
        let mut last_token = self.blank_id as i32;
        let vocab_size = self.vocab.len();
        let mut emitted: Vec<usize> = Vec::new();

        let mut t = 0usize;
        let mut emitted_at_t = 0usize;
        while t < total_t {
            let mut frame = Vec::with_capacity(enc_dim);
            for d in 0..enc_dim {
                frame.push(enc_outputs[[0, d, t]]);
            }
            let enc_frame = Array3::from_shape_vec((1, enc_dim, 1), frame)
                .map_err(|e| format!("Parakeet frame shape error: {e}"))?;
            let target_arr = Array2::from_elem((1, 1), last_token);
            let target_len_arr = Array1::from_vec(vec![1i32]);

            let (logits, out_s1, out_s2) = run_parakeet_joint(
                &mut self.decoder_joint,
                enc_frame,
                target_arr,
                target_len_arr,
                &state1,
                &state2,
            )?;

            if logits.len() < vocab_size {
                return Err(format!(
                    "Parakeet joint logits size {} < vocab {}",
                    logits.len(),
                    vocab_size
                ));
            }

            let token = argmax_slice(&logits[..vocab_size]);
            let dur_logits = &logits[vocab_size..];
            let dur_idx = if dur_logits.is_empty() {
                1
            } else {
                argmax_slice(dur_logits)
            };
            let skip = TDT_DURATIONS.get(dur_idx).copied().unwrap_or(1);

            if token != self.blank_id {
                emitted.push(token);
                last_token = token as i32;
                state1 = out_s1;
                state2 = out_s2;
                emitted_at_t += 1;
            }

            if skip > 0 {
                t += skip;
                emitted_at_t = 0;
            } else if token == self.blank_id || emitted_at_t >= MAX_SYMBOLS_PER_STEP {
                t += 1;
                emitted_at_t = 0;
            }
        }

        Ok(decode_parakeet_tokens(&emitted, &self.vocab, self.blank_id))
    }
}

type ParakeetJointOut = (Vec<f32>, ArrayD<f32>, ArrayD<f32>);

fn run_parakeet_joint(
    session: &mut Session,
    enc_frame: Array3<f32>,
    targets: Array2<i32>,
    target_length: Array1<i32>,
    state1: &ArrayD<f32>,
    state2: &ArrayD<f32>,
) -> Result<ParakeetJointOut, String> {
    let mut inputs: Vec<(String, DynValue)> = Vec::with_capacity(5);
    for inp in session.inputs() {
        let name = inp.name();
        let val: DynValue = match name {
            "encoder_outputs" => Tensor::from_array(enc_frame.clone())
                .map_err(|e| format!("{e}"))?
                .into_dyn(),
            "targets" => Tensor::from_array(targets.clone())
                .map_err(|e| format!("{e}"))?
                .into_dyn(),
            "target_length" => Tensor::from_array(target_length.clone())
                .map_err(|e| format!("{e}"))?
                .into_dyn(),
            "input_states_1" => Tensor::from_array(state1.clone())
                .map_err(|e| format!("{e}"))?
                .into_dyn(),
            "input_states_2" => Tensor::from_array(state2.clone())
                .map_err(|e| format!("{e}"))?
                .into_dyn(),
            other => return Err(format!("unexpected Parakeet decoder_joint input '{other}'")),
        };
        inputs.push((name.to_string(), val));
    }

    let outputs = session
        .run(inputs)
        .map_err(|e| format!("Parakeet decoder_joint failed: {e}"))?;

    let mut logits = Vec::new();
    let mut out_s1 = state1.clone();
    let mut out_s2 = state2.clone();
    for (name, val) in outputs.iter() {
        match name {
            "outputs" => {
                let view = val
                    .try_extract_array::<f32>()
                    .map_err(|e| format!("Parakeet logits extract error: {e}"))?;
                logits = view.iter().copied().collect();
            }
            "output_states_1" => {
                out_s1 = val
                    .try_extract_array::<f32>()
                    .map_err(|e| format!("Parakeet output_states_1 extract error: {e}"))?
                    .to_owned();
            }
            "output_states_2" => {
                out_s2 = val
                    .try_extract_array::<f32>()
                    .map_err(|e| format!("Parakeet output_states_2 extract error: {e}"))?
                    .to_owned();
            }
            _ => {}
        }
    }

    Ok((logits, out_s1, out_s2))
}

fn infer_state_shape(session: &Session, name: &str, fallback: &[usize]) -> Vec<usize> {
    session
        .inputs()
        .iter()
        .find(|i| i.name() == name)
        .and_then(|i| i.dtype().tensor_shape())
        .map(|s| {
            s.iter()
                .map(|&d| if d > 0 { d as usize } else { 1 })
                .collect()
        })
        .unwrap_or_else(|| fallback.to_vec())
}

fn argmax_slice(slice: &[f32]) -> usize {
    let mut best_idx = 0usize;
    let mut best_val = f32::NEG_INFINITY;
    for (i, &v) in slice.iter().enumerate() {
        if v > best_val {
            best_val = v;
            best_idx = i;
        }
    }
    best_idx
}

fn load_parakeet_vocab(path: &Path) -> Result<(Vec<String>, usize), String> {
    let body = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read Parakeet vocab.txt: {e}"))?;
    let mut entries: Vec<(usize, String)> = Vec::new();
    for line in body.lines() {
        let trimmed = line.trim_end_matches('\r');
        if trimmed.is_empty() {
            continue;
        }
        if let Some((piece, id_str)) = trimmed.rsplit_once(' ')
            && let Ok(id) = id_str.trim().parse::<usize>()
        {
            entries.push((id, piece.to_string()));
        }
    }
    if entries.is_empty() {
        return Err("Parakeet vocab.txt is empty".to_string());
    }
    let max_id = entries.iter().map(|(id, _)| *id).max().unwrap_or(0);
    let mut vocab = vec![String::new(); max_id + 1];
    let mut blank_id = max_id;
    for (id, piece) in entries {
        if piece == "<blk>" {
            blank_id = id;
        }
        vocab[id] = piece;
    }
    Ok((vocab, blank_id))
}

fn decode_parakeet_tokens(ids: &[usize], vocab: &[String], blank_id: usize) -> String {
    let mut out = String::new();
    for &id in ids {
        if id == blank_id {
            continue;
        }
        let Some(piece) = vocab.get(id) else {
            continue;
        };
        if piece.is_empty() || (piece.starts_with('<') && piece.ends_with('>')) {
            continue;
        }
        if let Some(rest) = piece.strip_prefix('▁') {
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(rest);
        } else {
            out.push_str(piece);
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn resolve_file(base: &Path, candidates: &[&str], label: &str) -> Result<PathBuf, String> {
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
    fn decode_parakeet_tokens_joins_sentencepiece_and_skips_blank() {
        let vocab = vec![
            "▁Hello".to_string(),
            "▁world".to_string(),
            "!".to_string(),
            "<blk>".to_string(),
        ];
        assert_eq!(decode_parakeet_tokens(&[0, 3, 1, 2], &vocab, 3), "Hello world!");
    }
}
