//! Embedded local CPU Text-to-Speech via Gradium's official Rust runtime (`ptts`)
//! for Phonon 40M Multilingual (`empyrealworks/Phonon-40M-multilingual` +
//! `gradium/phonon-voices`).

use std::path::{Path, PathBuf};

use base64::Engine as _;
use ptts::checkpoint::read_config;
use ptts::preprocess::Lang;
use ptts::synth::Synth;

use super::tts::{VoiceSynthesizeResult, encode_wav_pcm16};

pub struct LoadedPhonon {
    pub model_id: String,
    pub voice_id: String,
    pub lang_key: String,
    synth: Synth,
}

impl LoadedPhonon {
    pub fn load(
        model_id: &str,
        base: &Path,
        voice_id: &str,
        language: &str,
    ) -> Result<Self, String> {
        let model_path = resolve_gguf_model(base)?;
        let config_path = base.join("config.json");
        let tokenizer_path = base.join("tokenizer.json");
        let voice_path = resolve_voice_safetensors(base, voice_id)?;

        if !config_path.is_file() {
            return Err("Phonon config.json not found — re-download in Settings → Voice.".to_string());
        }
        if !tokenizer_path.is_file() {
            return Err(
                "Phonon tokenizer.json not found — re-download in Settings → Voice.".to_string(),
            );
        }

        let cfg = read_config(&config_path)
            .map_err(|e| format!("failed to read Phonon config.json: {e}"))?;
        let lang = parse_lang(language);
        let voice_key = if voice_id.trim().is_empty() {
            "default".to_string()
        } else {
            voice_id.trim().to_string()
        };

        let synth = Synth::builder(cfg, &model_path, lang)
            .tokenizer_file(&tokenizer_path)
            .add_voice(&voice_key, &voice_path)
            .voice(&voice_key)
            .build()
            .map_err(|e| format!("failed to load Phonon 40M model: {e}"))?;

        Ok(Self {
            model_id: model_id.to_string(),
            voice_id: voice_key,
            lang_key: lang_key(language),
            synth,
        })
    }

    pub fn matches(&self, model_id: &str, voice_id: &str, language: &str) -> bool {
        let v = if voice_id.trim().is_empty() {
            "default"
        } else {
            voice_id.trim()
        };
        self.model_id == model_id && self.voice_id == v && self.lang_key == lang_key(language)
    }

    pub fn synthesize(&self, text: &str) -> Result<VoiceSynthesizeResult, String> {
        let pcm = self
            .synth
            .say(text)
            .map_err(|e| format!("Phonon 40M synthesis failed: {e}"))?;
        if pcm.is_empty() {
            return Err("Phonon 40M produced empty audio".to_string());
        }
        let sample_rate = self.synth.sample_rate();
        let duration_ms = ((pcm.len() as u64) * 1000) / (sample_rate as u64);
        let wav = encode_wav_pcm16(&pcm, sample_rate);

        Ok(VoiceSynthesizeResult {
            wav_base64: base64::engine::general_purpose::STANDARD.encode(wav),
            sample_rate,
            duration_ms,
        })
    }
}

fn lang_key(language: &str) -> String {
    language
        .trim()
        .to_lowercase()
        .split(['-', '_'])
        .next()
        .unwrap_or("es")
        .to_string()
}

fn parse_lang(language: &str) -> Lang {
    match lang_key(language).as_str() {
        "en" => Lang::En,
        "fr" => Lang::Fr,
        "de" => Lang::De,
        "pt" => Lang::Pt,
        _ => Lang::Es,
    }
}

fn resolve_gguf_model(base: &Path) -> Result<PathBuf, String> {
    [
        base.join("model.q8.gguf"),
        base.join("model.q4k.gguf"),
        base.join("model.safetensors"),
    ]
    .into_iter()
    .find(|p| p.is_file())
    .ok_or_else(|| {
        "Phonon 40M model.q8.gguf is not installed. Download it in Settings → Voice.".to_string()
    })
}

fn resolve_voice_safetensors(base: &Path, voice_id: &str) -> Result<PathBuf, String> {
    let clean = voice_id.trim();
    if !clean.is_empty() {
        let exact = base.join("voices").join(format!("{clean}.safetensors"));
        if exact.is_file() {
            return Ok(exact);
        }
    }
    let fallback = base.join("default-voice.safetensors");
    if fallback.is_file() {
        return Ok(fallback);
    }
    Err(format!(
        "Phonon voice '{}' is not downloaded. Download a voice in Settings → Voice.",
        if clean.is_empty() { "default" } else { clean }
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_lang_maps_locale_prefixes() {
        assert_eq!(lang_key("es-ES"), "es");
        assert_eq!(lang_key("en-US"), "en");
        assert_eq!(lang_key("fr"), "fr");
    }

    #[test]
    #[ignore]
    fn live_phonon_40m_es_synthesizes_intelligible_spanish() {
        let home = std::env::var("HOME").expect("HOME");
        let data_dir = PathBuf::from(home).join("Library/Application Support/com.ipepio.nabla");
        let base = data_dir.join("models/tts/phonon-40m-es");
        let engine = LoadedPhonon::load("phonon-40m-es", &base, "b6FvJAiokjdqIti4", "es-ES")
            .expect("load phonon-40m-es");
        let res = engine
            .synthesize("Hola mundo, esta es una prueba de voz en español.")
            .expect("phonon-40m-es synthesize");
        let wav = base64::engine::general_purpose::STANDARD
            .decode(&res.wav_base64)
            .expect("decode b64");
        let samples: Vec<f32> = wav[44..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| (i16::from_le_bytes(*c) as f32) / 32768.0)
            .collect();
        let stt = crate::voice::VoiceSttEngine::new();
        let heard = stt
            .transcribe_samples(&data_dir, "moonshine-small-es", &samples, res.sample_rate)
            .expect("moonshine transcribe phonon audio");
        eprintln!(
            "Phonon 40M ES (Noa) synthesized {}ms -> Moonshine heard: {:?}",
            res.duration_ms, heard.text
        );
        assert!(res.duration_ms > 500, "too short: {}ms", res.duration_ms);
        assert!(
            heard.text.to_lowercase().contains("hola")
                || heard.text.to_lowercase().contains("prueba")
                || heard.text.to_lowercase().contains("español"),
            "Phonon 40M audio was not intelligible Spanish! Heard: {:?}",
            heard.text
        );
    }
}
