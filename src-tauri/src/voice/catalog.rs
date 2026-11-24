//! Curated & custom local voice model catalog (TTS & STT, 100% embedded CPU).
//!
//! Models live under `<data_dir>/models/{tts,stt}/<model-id>/`.
//! Custom models added by the user are persisted in `<data_dir>/models/custom-models.json`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Whether a model performs Text-to-Speech or Speech-to-Text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VoiceModelKind {
    Tts,
    Stt,
}

impl VoiceModelKind {
    pub fn dir_name(self) -> &'static str {
        match self {
            VoiceModelKind::Tts => "tts",
            VoiceModelKind::Stt => "stt",
        }
    }
}

/// Downloadable file belonging to a model or a voice pack.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceFileSpec {
    /// Relative path inside `<data_dir>/models/<kind>/<model-id>/`.
    pub rel_path: String,
    /// HTTPS download URL.
    pub url: String,
    /// Approximate size in bytes (informational before download).
    pub approx_bytes: u64,
}

/// One downloadable voice profile belonging to a TTS model (e.g. Kokoro `ef_dora`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoicePackSpec {
    pub id: String,
    pub name: String,
    pub language: String,
    pub gender: String,
    pub rel_path: String,
    pub url: String,
    pub approx_bytes: u64,
}

/// Runtime status of a voice pack as inspected on disk.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoicePackStatus {
    pub id: String,
    pub name: String,
    pub language: String,
    pub gender: String,
    pub approx_bytes: u64,
    pub installed: bool,
    pub installed_bytes: u64,
}

/// Host hardware snapshot used to recommend the best local CPU models.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostHardwareInfo {
    pub cpu_cores: u32,
    pub total_ram_gb: u32,
    pub arch: String,
    pub recommended_stt_id: String,
    pub recommended_tts_id: String,
}

/// Detects host CPU cores, total RAM (GiB), and architecture to recommend
/// appropriate local CPU voice models.
pub fn detect_host_hardware() -> HostHardwareInfo {
    let cpu_cores = std::thread::available_parallelism()
        .map(|n| n.get() as u32)
        .unwrap_or(4);
    let total_ram_gb = detect_total_ram_gb().unwrap_or(8);
    let arch = std::env::consts::ARCH.to_string();

    let recommended_stt_id = if total_ram_gb >= 16 && cpu_cores >= 8 {
        "moonshine-small-es".to_string()
    } else {
        "moonshine-tiny-es".to_string()
    };
    let recommended_tts_id = "kokoro-82m".to_string();

    HostHardwareInfo {
        cpu_cores,
        total_ram_gb,
        arch,
        recommended_stt_id,
        recommended_tts_id,
    }
}

fn detect_total_ram_gb() -> Option<u32> {
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("sysctl")
            .args(["-n", "hw.memsize"])
            .output()
            .ok()?;
        let bytes: u64 = String::from_utf8(out.stdout).ok()?.trim().parse().ok()?;
        Some(((bytes as f64) / (1024.0 * 1024.0 * 1024.0)).round().max(1.0) as u32)
    }
    #[cfg(target_os = "linux")]
    {
        let body = std::fs::read_to_string("/proc/meminfo").ok()?;
        let line = body.lines().find(|l| l.starts_with("MemTotal:"))?;
        let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
        Some(((kb as f64) / (1024.0 * 1024.0)).round().max(1.0) as u32)
    }
    #[cfg(target_os = "windows")]
    {
        let out = std::process::Command::new("wmic")
            .args(["ComputerSystem", "get", "TotalPhysicalMemory", "/value"])
            .output()
            .ok()?;
        let text = String::from_utf8(out.stdout).ok()?;
        let line = text.lines().find(|l| l.starts_with("TotalPhysicalMemory="))?;
        let bytes: u64 = line.split('=').nth(1)?.trim().parse().ok()?;
        Some(((bytes as f64) / (1024.0 * 1024.0 * 1024.0)).round().max(1.0) as u32)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        None
    }
}

/// One model definition in the catalog (curated or user-imported).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceModelSpec {
    pub id: String,
    pub kind: VoiceModelKind,
    /// Engine family: `"kokoro"`, `"moonshine-streaming"`, `"moonshine-seq2seq"`,
    /// `"whisper"`, `"parakeet"`, `"pocket-tts"`, `"phonon"`.
    pub engine: String,
    /// Display family name for grouping variants (e.g. `"Moonshine Streaming (ES)"`, `"Whisper"`).
    #[serde(default)]
    pub family: String,
    /// Short size/variant label inside the family (e.g. `"Tiny · 34M"`, `"Small · 123M"`).
    #[serde(default)]
    pub variant_label: String,
    pub name: String,
    pub description: String,
    pub params: String,
    pub languages: Vec<String>,
    /// `"native"` | `"chunked"` | `"windowed"`.
    pub streaming: String,
    pub license: String,
    pub repo_id: String,
    pub approx_bytes: u64,
    #[serde(default)]
    pub min_ram_gb: u32,
    #[serde(default = "default_runtime_ready")]
    pub runtime_ready: bool,
    pub files: Vec<VoiceFileSpec>,
    pub voices: Vec<VoicePackSpec>,
    pub custom: bool,
}

fn default_runtime_ready() -> bool {
    true
}

/// DTO sent across the Tauri seam describing one model and its live disk state.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceModelEntry {
    pub id: String,
    pub kind: VoiceModelKind,
    pub engine: String,
    pub family: String,
    pub variant_label: String,
    pub name: String,
    pub description: String,
    pub params: String,
    pub languages: Vec<String>,
    pub streaming: String,
    pub license: String,
    pub repo_id: String,
    pub approx_bytes: u64,
    pub min_ram_gb: u32,
    pub recommended: bool,
    pub runtime_ready: bool,
    /// `"not_installed"` | `"downloading"` | `"installed"`.
    pub status: String,
    pub installed_bytes: u64,
    pub voices: Vec<VoicePackStatus>,
    pub custom: bool,
}

/// Input payload for `voice_import_custom_model`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomVoiceModelDraft {
    pub kind: VoiceModelKind,
    pub engine: String,
    pub name: String,
    pub repo_or_url: String,
    #[serde(default)]
    pub language: String,
}

const HF_RESOLVE: &str = "https://huggingface.co";

fn hf_url(repo: &str, file: &str) -> String {
    format!("{HF_RESOLVE}/{repo}/resolve/main/{file}")
}

/// Root folder for all downloaded voice models: `<data_dir>/models`.
pub fn models_root(data_dir: &Path) -> PathBuf {
    data_dir.join("models")
}

/// Directory holding one model's files: `<data_dir>/models/{tts,stt}/<id>`.
pub fn model_dir(data_dir: &Path, kind: VoiceModelKind, id: &str) -> PathBuf {
    models_root(data_dir).join(kind.dir_name()).join(id)
}

fn custom_models_path(data_dir: &Path) -> PathBuf {
    models_root(data_dir).join("custom-models.json")
}

fn kokoro_voice(repo: &str, id: &str, name: &str, language: &str, gender: &str) -> VoicePackSpec {
    let rel_path = format!("voices/{id}.bin");
    VoicePackSpec {
        id: id.to_string(),
        name: name.to_string(),
        language: language.to_string(),
        gender: gender.to_string(),
        url: hf_url(repo, &rel_path),
        rel_path,
        approx_bytes: 522_240,
    }
}

fn phonon_voice(id: &str, name: &str, language: &str, gender: &str) -> VoicePackSpec {
    let voices_repo = "gradium/phonon-voices";
    let rel_path = format!("voices/{id}.safetensors");
    VoicePackSpec {
        id: id.to_string(),
        name: name.to_string(),
        language: language.to_string(),
        gender: gender.to_string(),
        url: hf_url(voices_repo, &rel_path),
        rel_path,
        approx_bytes: 256_136,
    }
}

/// Built-in curated catalog of CPU-friendly, open-license TTS and STT models.
pub fn curated_models() -> Vec<VoiceModelSpec> {
    let kokoro_repo = "onnx-community/Kokoro-82M-v1.0-ONNX";
    let phonon_40m_repo = "empyrealworks/Phonon-40M-multilingual";
    let moonshine_tiny_es_repo = "Masterx/moonshine-streaming-tiny-es-ONNX";
    let moonshine_small_es_repo = "Masterx/moonshine-streaming-small-es-ONNX";
    let moonshine_tiny_en_repo = "onnx-community/moonshine-tiny-ONNX";
    let whisper_small_repo = "onnx-community/whisper-small";
    let whisper_turbo_repo = "onnx-community/whisper-large-v3-turbo";
    let parakeet_repo = "istupakov/parakeet-tdt-0.6b-v2-onnx";

    vec![
        VoiceModelSpec {
            id: "kokoro-82m".to_string(),
            kind: VoiceModelKind::Tts,
            engine: "kokoro".to_string(),
            family: "Kokoro 82M".to_string(),
            variant_label: "82M (q8)".to_string(),
            name: "Kokoro 82M (q8)".to_string(),
            description: "Fast, natural CPU TTS with Spanish & English voices.".to_string(),
            params: "82M".to_string(),
            languages: vec!["es".to_string(), "en".to_string()],
            streaming: "chunked".to_string(),
            license: "Apache 2.0".to_string(),
            repo_id: kokoro_repo.to_string(),
            approx_bytes: 92_400_000,
            min_ram_gb: 4,
            runtime_ready: true,
            files: vec![
                VoiceFileSpec {
                    rel_path: "onnx/model_quantized.onnx".to_string(),
                    url: hf_url(kokoro_repo, "onnx/model_quantized.onnx"),
                    approx_bytes: 92_400_000,
                },
                VoiceFileSpec {
                    rel_path: "tokenizer.json".to_string(),
                    url: hf_url(kokoro_repo, "tokenizer.json"),
                    approx_bytes: 4_096,
                },
            ],
            voices: vec![
                // Spanish
                kokoro_voice(kokoro_repo, "ef_dora", "Dora", "es", "female"),
                kokoro_voice(kokoro_repo, "em_alex", "Alex", "es", "male"),
                kokoro_voice(kokoro_repo, "em_santa", "Santa", "es", "male"),
                // English (US)
                kokoro_voice(kokoro_repo, "af_heart", "Heart", "en-US", "female"),
                kokoro_voice(kokoro_repo, "af_bella", "Bella", "en-US", "female"),
                kokoro_voice(kokoro_repo, "af_alloy", "Alloy", "en-US", "female"),
                kokoro_voice(kokoro_repo, "af_aoede", "Aoede", "en-US", "female"),
                kokoro_voice(kokoro_repo, "af_jessica", "Jessica", "en-US", "female"),
                kokoro_voice(kokoro_repo, "af_kore", "Kore", "en-US", "female"),
                kokoro_voice(kokoro_repo, "af_nicole", "Nicole", "en-US", "female"),
                kokoro_voice(kokoro_repo, "af_nova", "Nova", "en-US", "female"),
                kokoro_voice(kokoro_repo, "af_river", "River", "en-US", "female"),
                kokoro_voice(kokoro_repo, "af_sarah", "Sarah", "en-US", "female"),
                kokoro_voice(kokoro_repo, "af_sky", "Sky", "en-US", "female"),
                kokoro_voice(kokoro_repo, "am_adam", "Adam", "en-US", "male"),
                kokoro_voice(kokoro_repo, "am_echo", "Echo", "en-US", "male"),
                kokoro_voice(kokoro_repo, "am_eric", "Eric", "en-US", "male"),
                kokoro_voice(kokoro_repo, "am_fenrir", "Fenrir", "en-US", "male"),
                kokoro_voice(kokoro_repo, "am_liam", "Liam", "en-US", "male"),
                kokoro_voice(kokoro_repo, "am_michael", "Michael", "en-US", "male"),
                kokoro_voice(kokoro_repo, "am_onyx", "Onyx", "en-US", "male"),
                kokoro_voice(kokoro_repo, "am_puck", "Puck", "en-US", "male"),
                kokoro_voice(kokoro_repo, "am_santa", "Santa (US)", "en-US", "male"),
                // English (UK)
                kokoro_voice(kokoro_repo, "bf_alice", "Alice", "en-GB", "female"),
                kokoro_voice(kokoro_repo, "bf_emma", "Emma", "en-GB", "female"),
                kokoro_voice(kokoro_repo, "bf_isabella", "Isabella", "en-GB", "female"),
                kokoro_voice(kokoro_repo, "bf_lily", "Lily", "en-GB", "female"),
                kokoro_voice(kokoro_repo, "bm_daniel", "Daniel", "en-GB", "male"),
                kokoro_voice(kokoro_repo, "bm_fable", "Fable", "en-GB", "male"),
                kokoro_voice(kokoro_repo, "bm_george", "George", "en-GB", "male"),
                kokoro_voice(kokoro_repo, "bm_lewis", "Lewis", "en-GB", "male"),
                // French
                kokoro_voice(kokoro_repo, "ff_siwis", "Siwis", "fr", "female"),
                // Italian
                kokoro_voice(kokoro_repo, "if_sara", "Sara", "it", "female"),
                kokoro_voice(kokoro_repo, "im_nicola", "Nicola", "it", "male"),
                // Portuguese
                kokoro_voice(kokoro_repo, "pf_dora", "Dora (PT)", "pt", "female"),
                kokoro_voice(kokoro_repo, "pm_alex", "Alex (PT)", "pt", "male"),
                kokoro_voice(kokoro_repo, "pm_santa", "Santa (PT)", "pt", "male"),
                // Japanese
                kokoro_voice(kokoro_repo, "jf_alpha", "Alpha (JA)", "ja", "female"),
                kokoro_voice(kokoro_repo, "jf_gongitsune", "Gongitsune", "ja", "female"),
                kokoro_voice(kokoro_repo, "jf_nezumi", "Nezumi", "ja", "female"),
                kokoro_voice(kokoro_repo, "jf_tebukuro", "Tebukuro", "ja", "female"),
                kokoro_voice(kokoro_repo, "jm_kumo", "Kumo", "ja", "male"),
                // Hindi
                kokoro_voice(kokoro_repo, "hf_alpha", "Alpha (HI)", "hi", "female"),
                kokoro_voice(kokoro_repo, "hf_beta", "Beta (HI)", "hi", "female"),
                kokoro_voice(kokoro_repo, "hm_omega", "Omega", "hi", "male"),
                kokoro_voice(kokoro_repo, "hm_psi", "Psi", "hi", "male"),
                // Chinese
                kokoro_voice(kokoro_repo, "zf_xiaobei", "Xiaobei", "zh", "female"),
                kokoro_voice(kokoro_repo, "zf_xiaoni", "Xiaoni", "zh", "female"),
                kokoro_voice(kokoro_repo, "zf_xiaoxiao", "Xiaoxiao", "zh", "female"),
                kokoro_voice(kokoro_repo, "zf_xiaoyi", "Xiaoyi", "zh", "female"),
                kokoro_voice(kokoro_repo, "zm_yunjian", "Yunjian", "zh", "male"),
                kokoro_voice(kokoro_repo, "zm_yunxi", "Yunxi", "zh", "male"),
                kokoro_voice(kokoro_repo, "zm_yunxia", "Yunxia", "zh", "male"),
                kokoro_voice(kokoro_repo, "zm_yunyang", "Yunyang", "zh", "male"),
            ],
            custom: false,
        },
        VoiceModelSpec {
            id: "phonon-40m-es".to_string(),
            kind: VoiceModelKind::Tts,
            engine: "phonon-gguf".to_string(),
            family: "Phonon 40M".to_string(),
            variant_label: "40M Multilingual (q8 GGUF)".to_string(),
            name: "Phonon 40M Multilingual (q8)".to_string(),
            description: "Gradium 24 kHz native Rust TTS (~40M GGUF) with Spanish, English, French, German & Portuguese voices."
                .to_string(),
            params: "40M".to_string(),
            languages: vec![
                "es".to_string(),
                "en".to_string(),
                "fr".to_string(),
                "de".to_string(),
                "pt".to_string(),
            ],
            streaming: "native".to_string(),
            license: "CC BY-NC 4.0".to_string(),
            repo_id: phonon_40m_repo.to_string(),
            approx_bytes: 74_837_000,
            min_ram_gb: 2,
            runtime_ready: true,
            files: vec![
                VoiceFileSpec {
                    rel_path: "model.q8.gguf".to_string(),
                    url: hf_url(phonon_40m_repo, "model.q8.gguf"),
                    approx_bytes: 74_553_568,
                },
                VoiceFileSpec {
                    rel_path: "tokenizer.json".to_string(),
                    url: hf_url(phonon_40m_repo, "tokenizer.json"),
                    approx_bytes: 277_749,
                },
                VoiceFileSpec {
                    rel_path: "config.json".to_string(),
                    url: hf_url(phonon_40m_repo, "config.json"),
                    approx_bytes: 4_674,
                },
            ],
            voices: vec![
                // Spanish (Spain)
                phonon_voice("b6FvJAiokjdqIti4", "Noa", "es-ES", "female"),
                phonon_voice("3p0eIGbkmny71GlA", "Lucía-Sol", "es-ES", "female"),
                phonon_voice("c9wqBDmQiBie5q6Y", "Candela", "es-ES", "female"),
                phonon_voice("rmLBOdHrfkFym9MM", "Celia", "es-ES", "female"),
                phonon_voice("u2UscyAnHilpiwmf", "Alba", "es-ES", "female"),
                phonon_voice("4701YgRXjkBcg6sL", "Alicia", "es-ES", "female"),
                phonon_voice("EQcQFGkYfMn5YJgx", "Nerea", "es-ES", "female"),
                phonon_voice("jvPx8j8zLGQ3utZz", "Marcos", "es-ES", "male"),
                phonon_voice("tIjCgP8v3RY8UpgM", "Borja", "es-ES", "male"),
                phonon_voice("t-_TS1e-0GzDAX02", "Iker", "es-ES", "male"),
                phonon_voice("fghJGlv90qu8OJ36", "Nuno", "es-ES", "male"),
                phonon_voice("Ejb29qHnSOK0cIHY", "Izan", "es-ES", "male"),
                phonon_voice("FaLVVqpFQcDOtq45", "Guillermo", "es-ES", "male"),
                // Spanish (Mexico)
                phonon_voice("Sx8m6WdXBG06zwZT", "Rebeca", "es-MX", "female"),
                phonon_voice("s58clrg2fe0MO7Y-", "Regina", "es-MX", "female"),
                phonon_voice("evfxKSxpDIhmsNoq", "Danae", "es-MX", "female"),
                phonon_voice("KJpIQlbYrZPA4aLq", "Azucena", "es-MX", "female"),
                phonon_voice("qG5BXOJYrKEnXLIX", "Jimena", "es-MX", "female"),
                phonon_voice("VDwnGxAo68C8U8vC", "Ximena", "es-MX", "female"),
                phonon_voice("F05gqkzX7WhKVpSL", "Lalo", "es-MX", "male"),
                phonon_voice("ax7dW1XB0eiMfHgO", "Aldo", "es-MX", "male"),
                phonon_voice("ALczEazoF4n6PmGl", "Yael", "es-MX", "male"),
                phonon_voice("G5HLUsQd659UtRe7", "Ulises", "es-MX", "male"),
                phonon_voice("BkuASiOJRrJEB5qA", "Leonel", "es-MX", "male"),
                phonon_voice("PwgPEaweQILZdGc4", "Nico", "es-MX", "male"),
                // English
                phonon_voice("NbpkqMVS3CJeq2j8", "Zoey", "en-US", "female"),
                phonon_voice("NIWIOv5vTi5eRMj8", "Juniper", "en-US", "female"),
                phonon_voice("LMDIYE0Oa377Qh6Z", "Easton", "en-US", "male"),
                phonon_voice("d9XCyhjo4JhkHF1d", "Miles", "en-US", "male"),
                phonon_voice("6PWnV0Nq4wu7RVBT", "Maeve", "en-GB", "female"),
                phonon_voice("GgfEkEJtxZR7gnpy", "Freya", "en-GB", "female"),
            ],
            custom: false,
        },
        VoiceModelSpec {
            id: "moonshine-tiny-es".to_string(),
            kind: VoiceModelKind::Stt,
            engine: "moonshine-streaming".to_string(),
            family: "Moonshine Streaming (ES)".to_string(),
            variant_label: "Tiny · 34M".to_string(),
            name: "Moonshine Tiny Streaming (ES)".to_string(),
            description: "Ultra-lightweight Spanish streaming STT for CPU.".to_string(),
            params: "34M".to_string(),
            languages: vec!["es".to_string()],
            streaming: "native".to_string(),
            license: "MIT".to_string(),
            repo_id: moonshine_tiny_es_repo.to_string(),
            approx_bytes: 46_000_000,
            min_ram_gb: 2,
            runtime_ready: true,
            files: vec![
                VoiceFileSpec {
                    rel_path: "frontend.onnx".to_string(),
                    url: hf_url(moonshine_tiny_es_repo, "frontend.onnx"),
                    approx_bytes: 4_000_000,
                },
                VoiceFileSpec {
                    rel_path: "encoder_int8.onnx".to_string(),
                    url: hf_url(moonshine_tiny_es_repo, "encoder_int8.onnx"),
                    approx_bytes: 16_000_000,
                },
                VoiceFileSpec {
                    rel_path: "adapter_int8.onnx".to_string(),
                    url: hf_url(moonshine_tiny_es_repo, "adapter_int8.onnx"),
                    approx_bytes: 2_000_000,
                },
                VoiceFileSpec {
                    rel_path: "cross_kv_int8.onnx".to_string(),
                    url: hf_url(moonshine_tiny_es_repo, "cross_kv_int8.onnx"),
                    approx_bytes: 4_000_000,
                },
                VoiceFileSpec {
                    rel_path: "decoder_kv_int8.onnx".to_string(),
                    url: hf_url(moonshine_tiny_es_repo, "decoder_kv_int8.onnx"),
                    approx_bytes: 19_000_000,
                },
                VoiceFileSpec {
                    rel_path: "streaming_config.json".to_string(),
                    url: hf_url(moonshine_tiny_es_repo, "streaming_config.json"),
                    approx_bytes: 4_096,
                },
                VoiceFileSpec {
                    rel_path: "tokenizer.json".to_string(),
                    url: hf_url(moonshine_tiny_es_repo, "tokenizer.json"),
                    approx_bytes: 1_000_000,
                },
            ],
            voices: Vec::new(),
            custom: false,
        },
        VoiceModelSpec {
            id: "moonshine-small-es".to_string(),
            kind: VoiceModelKind::Stt,
            engine: "moonshine-streaming".to_string(),
            family: "Moonshine Streaming (ES)".to_string(),
            variant_label: "Small · 123M".to_string(),
            name: "Moonshine Small Streaming (ES)".to_string(),
            description: "Higher-accuracy Spanish streaming STT for CPU.".to_string(),
            params: "123M".to_string(),
            languages: vec!["es".to_string()],
            streaming: "native".to_string(),
            license: "MIT".to_string(),
            repo_id: moonshine_small_es_repo.to_string(),
            approx_bytes: 145_000_000,
            min_ram_gb: 8,
            runtime_ready: true,
            files: vec![
                VoiceFileSpec {
                    rel_path: "frontend.onnx".to_string(),
                    url: hf_url(moonshine_small_es_repo, "frontend.onnx"),
                    approx_bytes: 8_000_000,
                },
                VoiceFileSpec {
                    rel_path: "encoder_int8.onnx".to_string(),
                    url: hf_url(moonshine_small_es_repo, "encoder_int8.onnx"),
                    approx_bytes: 55_000_000,
                },
                VoiceFileSpec {
                    rel_path: "adapter_int8.onnx".to_string(),
                    url: hf_url(moonshine_small_es_repo, "adapter_int8.onnx"),
                    approx_bytes: 5_000_000,
                },
                VoiceFileSpec {
                    rel_path: "cross_kv_int8.onnx".to_string(),
                    url: hf_url(moonshine_small_es_repo, "cross_kv_int8.onnx"),
                    approx_bytes: 12_000_000,
                },
                VoiceFileSpec {
                    rel_path: "decoder_kv_int8.onnx".to_string(),
                    url: hf_url(moonshine_small_es_repo, "decoder_kv_int8.onnx"),
                    approx_bytes: 64_000_000,
                },
                VoiceFileSpec {
                    rel_path: "streaming_config.json".to_string(),
                    url: hf_url(moonshine_small_es_repo, "streaming_config.json"),
                    approx_bytes: 4_096,
                },
                VoiceFileSpec {
                    rel_path: "tokenizer.json".to_string(),
                    url: hf_url(moonshine_small_es_repo, "tokenizer.json"),
                    approx_bytes: 1_000_000,
                },
            ],
            voices: Vec::new(),
            custom: false,
        },
        VoiceModelSpec {
            id: "moonshine-tiny-en".to_string(),
            kind: VoiceModelKind::Stt,
            engine: "moonshine-seq2seq".to_string(),
            family: "Moonshine (EN)".to_string(),
            variant_label: "Tiny · 27M".to_string(),
            name: "Moonshine Tiny (EN)".to_string(),
            description: "Compact English CPU speech recognition model.".to_string(),
            params: "27M".to_string(),
            languages: vec!["en".to_string()],
            streaming: "chunked".to_string(),
            license: "MIT".to_string(),
            repo_id: moonshine_tiny_en_repo.to_string(),
            approx_bytes: 35_000_000,
            min_ram_gb: 2,
            runtime_ready: true,
            files: vec![
                VoiceFileSpec {
                    rel_path: "onnx/encoder_model_quantized.onnx".to_string(),
                    url: hf_url(moonshine_tiny_en_repo, "onnx/encoder_model_quantized.onnx"),
                    approx_bytes: 12_000_000,
                },
                VoiceFileSpec {
                    rel_path: "onnx/decoder_model_merged_quantized.onnx".to_string(),
                    url: hf_url(
                        moonshine_tiny_en_repo,
                        "onnx/decoder_model_merged_quantized.onnx",
                    ),
                    approx_bytes: 22_000_000,
                },
                VoiceFileSpec {
                    rel_path: "tokenizer.json".to_string(),
                    url: hf_url(moonshine_tiny_en_repo, "tokenizer.json"),
                    approx_bytes: 1_000_000,
                },
            ],
            voices: Vec::new(),
            custom: false,
        },
        VoiceModelSpec {
            id: "whisper-small".to_string(),
            kind: VoiceModelKind::Stt,
            engine: "whisper".to_string(),
            family: "Whisper".to_string(),
            variant_label: "Small (q8) · 244M".to_string(),
            name: "Whisper Small (q8)".to_string(),
            description: "Multilingual windowed CPU STT with strong Spanish & English accuracy.".to_string(),
            params: "244M".to_string(),
            languages: vec!["es".to_string(), "en".to_string(), "multi".to_string()],
            streaming: "windowed".to_string(),
            license: "MIT".to_string(),
            repo_id: whisper_small_repo.to_string(),
            approx_bytes: 251_600_000,
            min_ram_gb: 8,
            runtime_ready: true,
            files: vec![
                VoiceFileSpec {
                    rel_path: "onnx/encoder_model_quantized.onnx".to_string(),
                    url: hf_url(whisper_small_repo, "onnx/encoder_model_quantized.onnx"),
                    approx_bytes: 92_326_160,
                },
                VoiceFileSpec {
                    rel_path: "onnx/decoder_model_merged_quantized.onnx".to_string(),
                    url: hf_url(
                        whisper_small_repo,
                        "onnx/decoder_model_merged_quantized.onnx",
                    ),
                    approx_bytes: 156_750_845,
                },
                VoiceFileSpec {
                    rel_path: "tokenizer.json".to_string(),
                    url: hf_url(whisper_small_repo, "tokenizer.json"),
                    approx_bytes: 2_500_000,
                },
                VoiceFileSpec {
                    rel_path: "preprocessor_config.json".to_string(),
                    url: hf_url(whisper_small_repo, "preprocessor_config.json"),
                    approx_bytes: 1_024,
                },
            ],
            voices: Vec::new(),
            custom: false,
        },
        VoiceModelSpec {
            id: "whisper-turbo".to_string(),
            kind: VoiceModelKind::Stt,
            engine: "whisper".to_string(),
            family: "Whisper".to_string(),
            variant_label: "Large v3 Turbo (q4) · 809M".to_string(),
            name: "Whisper Large v3 Turbo (q4)".to_string(),
            description: "High-accuracy multilingual STT for machines with 16 GB+ RAM.".to_string(),
            params: "809M".to_string(),
            languages: vec!["es".to_string(), "en".to_string(), "multi".to_string()],
            streaming: "windowed".to_string(),
            license: "MIT".to_string(),
            repo_id: whisper_turbo_repo.to_string(),
            approx_bytes: 761_600_000,
            min_ram_gb: 16,
            runtime_ready: true,
            files: vec![
                VoiceFileSpec {
                    rel_path: "onnx/encoder_model_q4.onnx".to_string(),
                    url: hf_url(whisper_turbo_repo, "onnx/encoder_model_q4.onnx"),
                    approx_bytes: 424_942_775,
                },
                VoiceFileSpec {
                    rel_path: "onnx/decoder_model_merged_q4.onnx".to_string(),
                    url: hf_url(whisper_turbo_repo, "onnx/decoder_model_merged_q4.onnx"),
                    approx_bytes: 334_147_222,
                },
                VoiceFileSpec {
                    rel_path: "tokenizer.json".to_string(),
                    url: hf_url(whisper_turbo_repo, "tokenizer.json"),
                    approx_bytes: 2_500_000,
                },
                VoiceFileSpec {
                    rel_path: "preprocessor_config.json".to_string(),
                    url: hf_url(whisper_turbo_repo, "preprocessor_config.json"),
                    approx_bytes: 1_024,
                },
            ],
            voices: Vec::new(),
            custom: false,
        },
        VoiceModelSpec {
            id: "parakeet-tdt-v2".to_string(),
            kind: VoiceModelKind::Stt,
            engine: "parakeet".to_string(),
            family: "Parakeet TDT".to_string(),
            variant_label: "0.6B v2 (int8) · 600M".to_string(),
            name: "Parakeet TDT 0.6B v2 (int8)".to_string(),
            description: "Fast NVIDIA Conformer-TDT ASR model quantized for CPU.".to_string(),
            params: "600M".to_string(),
            languages: vec!["en".to_string()],
            streaming: "chunked".to_string(),
            license: "CC BY 4.0".to_string(),
            repo_id: parakeet_repo.to_string(),
            approx_bytes: 661_332_000,
            min_ram_gb: 12,
            runtime_ready: true,
            files: vec![
                VoiceFileSpec {
                    rel_path: "encoder-model.int8.onnx".to_string(),
                    url: hf_url(parakeet_repo, "encoder-model.int8.onnx"),
                    approx_bytes: 652_184_014,
                },
                VoiceFileSpec {
                    rel_path: "decoder_joint-model.int8.onnx".to_string(),
                    url: hf_url(parakeet_repo, "decoder_joint-model.int8.onnx"),
                    approx_bytes: 8_998_286,
                },
                VoiceFileSpec {
                    rel_path: "nemo128.onnx".to_string(),
                    url: hf_url(parakeet_repo, "nemo128.onnx"),
                    approx_bytes: 139_764,
                },
                VoiceFileSpec {
                    rel_path: "vocab.txt".to_string(),
                    url: hf_url(parakeet_repo, "vocab.txt"),
                    approx_bytes: 9_384,
                },
            ],
            voices: Vec::new(),
            custom: false,
        },
    ]
}

/// Reads user-imported custom model specs from `<data_dir>/models/custom-models.json`.
pub fn read_custom_models(data_dir: &Path) -> Vec<VoiceModelSpec> {
    let Ok(body) = std::fs::read_to_string(custom_models_path(data_dir)) else {
        return Vec::new();
    };
    serde_json::from_str::<Vec<VoiceModelSpec>>(&body).unwrap_or_default()
}

fn write_custom_models(data_dir: &Path, models: &[VoiceModelSpec]) -> Result<(), String> {
    let root = models_root(data_dir);
    std::fs::create_dir_all(&root).map_err(|e| format!("cannot create models dir: {e}"))?;
    let json = serde_json::to_string_pretty(models)
        .map_err(|e| format!("cannot serialize custom models: {e}"))?;
    let target = custom_models_path(data_dir);
    let tmp = root.join("custom-models.json.tmp");
    std::fs::write(&tmp, json).map_err(|e| format!("cannot write custom models: {e}"))?;
    std::fs::rename(&tmp, &target).map_err(|e| format!("cannot commit custom models: {e}"))?;
    Ok(())
}

/// Combined catalog: curated models followed by user-imported custom models.
pub fn all_model_specs(data_dir: &Path) -> Vec<VoiceModelSpec> {
    let mut specs = curated_models();
    specs.extend(read_custom_models(data_dir));
    specs
}

pub fn find_model_spec(data_dir: &Path, model_id: &str) -> Option<VoiceModelSpec> {
    all_model_specs(data_dir)
        .into_iter()
        .find(|m| m.id == model_id)
}

fn dir_size(path: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    let mut total = 0u64;
    for entry in entries.filter_map(|e| e.ok()) {
        let p = entry.path();
        if p.is_dir() {
            total += dir_size(&p);
        } else if let Ok(meta) = entry.metadata() {
            total += meta.len();
        }
    }
    total
}

/// Builds a `VoiceModelEntry` by inspecting `<data_dir>/models/{tts,stt}/<id>/`.
pub fn inspect_model(
    data_dir: &Path,
    spec: &VoiceModelSpec,
    downloading_ids: &HashSet<String>,
    host: &HostHardwareInfo,
) -> VoiceModelEntry {
    let base = model_dir(data_dir, spec.kind, &spec.id);
    let core_installed = !spec.files.is_empty()
        && spec
            .files
            .iter()
            .all(|f| base.join(&f.rel_path).is_file());

    let status = if downloading_ids.contains(&spec.id) {
        "downloading"
    } else if core_installed {
        "installed"
    } else {
        "not_installed"
    };

    let installed_bytes = if base.is_dir() { dir_size(&base) } else { 0 };

    let voices = spec
        .voices
        .iter()
        .map(|v| {
            let vp = base.join(&v.rel_path);
            let meta = std::fs::metadata(&vp).ok();
            let installed = meta.as_ref().is_some_and(|m| m.is_file() && m.len() > 0);
            VoicePackStatus {
                id: v.id.clone(),
                name: v.name.clone(),
                language: v.language.clone(),
                gender: v.gender.clone(),
                approx_bytes: v.approx_bytes,
                installed,
                installed_bytes: meta.map(|m| m.len()).unwrap_or(0),
            }
        })
        .collect();

    let recommended = spec.id == host.recommended_stt_id || spec.id == host.recommended_tts_id;

    VoiceModelEntry {
        id: spec.id.clone(),
        kind: spec.kind,
        engine: spec.engine.clone(),
        family: if spec.family.is_empty() {
            spec.name.clone()
        } else {
            spec.family.clone()
        },
        variant_label: if spec.variant_label.is_empty() {
            spec.params.clone()
        } else {
            spec.variant_label.clone()
        },
        name: spec.name.clone(),
        description: spec.description.clone(),
        params: spec.params.clone(),
        languages: spec.languages.clone(),
        streaming: spec.streaming.clone(),
        license: spec.license.clone(),
        repo_id: spec.repo_id.clone(),
        approx_bytes: spec.approx_bytes,
        min_ram_gb: spec.min_ram_gb,
        recommended,
        runtime_ready: spec.runtime_ready,
        status: status.to_string(),
        installed_bytes,
        voices,
        custom: spec.custom,
    }
}

/// Lists all models (curated + custom) with their current installation status.
pub fn list_catalog(
    data_dir: &Path,
    downloading_ids: &HashSet<String>,
) -> Vec<VoiceModelEntry> {
    let host = detect_host_hardware();
    all_model_specs(data_dir)
        .iter()
        .map(|spec| inspect_model(data_dir, spec, downloading_ids, &host))
        .collect()
}

fn sanitize_slug(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut prev_dash = false;
    for ch in raw.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash && !out.is_empty() {
            out.push('-');
            prev_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

/// Normalizes a Hugging Face repo input (`owner/repo` or `https://huggingface.co/owner/repo`)
/// or direct HTTPS `.onnx` / `.gguf` URL.
fn parse_repo_or_url(raw: &str) -> Result<(String, Option<String>), String> {
    let trimmed = raw.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return Err("Repository or model URL is required".to_string());
    }
    if trimmed.starts_with("https://") {
        let is_direct_weight = trimmed.ends_with(".onnx") || trimmed.ends_with(".gguf");
        if let Some(rest) = trimmed.strip_prefix("https://huggingface.co/") {
            let parts: Vec<&str> = rest.split('/').filter(|s| !s.is_empty()).collect();
            if parts.len() >= 2 && !is_direct_weight {
                return Ok((format!("{}/{}", parts[0], parts[1]), None));
            }
        }
        if is_direct_weight {
            return Ok((trimmed.to_string(), Some(trimmed.to_string())));
        }
        return Err(
            "URL must be a Hugging Face repo (https://huggingface.co/owner/repo) or a direct .onnx / .gguf HTTPS URL"
                .to_string(),
        );
    }
    if trimmed.starts_with("http://") {
        return Err("Only HTTPS URLs are allowed for model downloads".to_string());
    }
    let parts: Vec<&str> = trimmed.split('/').filter(|s| !s.is_empty()).collect();
    if parts.len() == 2
        && parts
            .iter()
            .all(|p| p.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')))
    {
        return Ok((format!("{}/{}", parts[0], parts[1]), None));
    }
    Err("Enter a Hugging Face repo (owner/repo) or a direct HTTPS .onnx / .gguf URL".to_string())
}

/// Registers a user-supplied ONNX/GGUF model in `<data_dir>/models/custom-models.json`.
pub fn import_custom_model(
    data_dir: &Path,
    draft: CustomVoiceModelDraft,
) -> Result<VoiceModelEntry, String> {
    let name = draft.name.trim();
    if name.is_empty() {
        return Err("Model name is required".to_string());
    }
    let engine = draft.engine.trim().to_ascii_lowercase();
    let valid_engine = match draft.kind {
        VoiceModelKind::Stt => matches!(
            engine.as_str(),
            "moonshine-streaming" | "moonshine-seq2seq" | "whisper" | "parakeet"
        ),
        VoiceModelKind::Tts => matches!(engine.as_str(), "kokoro" | "phonon-gguf"),
    };
    if !valid_engine {
        return Err(format!("Unsupported engine '{engine}' for {:?}", draft.kind));
    }
    let (repo_id, direct_url) = parse_repo_or_url(&draft.repo_or_url)?;
    let slug = sanitize_slug(name);
    if slug.is_empty() {
        return Err("Model name must include letters or digits".to_string());
    }
    let id = format!("custom-{slug}");

    let existing_all = all_model_specs(data_dir);
    if existing_all.iter().any(|m| m.id == id) {
        return Err(format!("A model with id '{id}' already exists"));
    }

    let files = if let Some(url) = direct_url {
        let rel_path = if engine == "phonon-gguf" || url.ends_with(".gguf") {
            "model.q8.gguf".to_string()
        } else {
            "model.onnx".to_string()
        };
        vec![VoiceFileSpec {
            rel_path,
            url,
            approx_bytes: 0,
        }]
    } else {
        match engine.as_str() {
            "kokoro" => vec![
                VoiceFileSpec {
                    rel_path: "onnx/model_quantized.onnx".to_string(),
                    url: hf_url(&repo_id, "onnx/model_quantized.onnx"),
                    approx_bytes: 92_400_000,
                },
                VoiceFileSpec {
                    rel_path: "tokenizer.json".to_string(),
                    url: hf_url(&repo_id, "tokenizer.json"),
                    approx_bytes: 4_096,
                },
            ],
            "phonon-gguf" => vec!["model.q8.gguf", "tokenizer.json", "config.json"]
                .into_iter()
                .map(|f| VoiceFileSpec {
                    rel_path: f.to_string(),
                    url: hf_url(&repo_id, f),
                    approx_bytes: 0,
                })
                .collect(),
            "moonshine-streaming" => vec![
                "frontend.onnx",
                "encoder_int8.onnx",
                "adapter_int8.onnx",
                "cross_kv_int8.onnx",
                "decoder_kv_int8.onnx",
                "streaming_config.json",
                "tokenizer.json",
            ]
            .into_iter()
            .map(|f| VoiceFileSpec {
                rel_path: f.to_string(),
                url: hf_url(&repo_id, f),
                approx_bytes: 0,
            })
            .collect(),
            "whisper" => vec![
                "onnx/encoder_model_quantized.onnx",
                "onnx/decoder_model_merged_quantized.onnx",
                "tokenizer.json",
                "preprocessor_config.json",
            ]
            .into_iter()
            .map(|f| VoiceFileSpec {
                rel_path: f.to_string(),
                url: hf_url(&repo_id, f),
                approx_bytes: 0,
            })
            .collect(),
            "parakeet" => vec![
                "encoder-model.int8.onnx",
                "decoder_joint-model.int8.onnx",
                "nemo128.onnx",
                "vocab.txt",
            ]
            .into_iter()
            .map(|f| VoiceFileSpec {
                rel_path: f.to_string(),
                url: hf_url(&repo_id, f),
                approx_bytes: 0,
            })
            .collect(),
            _ => vec![
                "onnx/encoder_model_quantized.onnx",
                "onnx/decoder_model_merged_quantized.onnx",
                "tokenizer.json",
            ]
            .into_iter()
            .map(|f| VoiceFileSpec {
                rel_path: f.to_string(),
                url: hf_url(&repo_id, f),
                approx_bytes: 0,
            })
            .collect(),
        }
    };

    let voices = match engine.as_str() {
        "phonon-gguf" => curated_models()
            .into_iter()
            .find(|m| m.engine == "phonon-gguf")
            .map(|m| m.voices)
            .unwrap_or_default(),
        "kokoro" => vec![
            kokoro_voice(&repo_id, "ef_dora", "Dora", "es", "female"),
            kokoro_voice(&repo_id, "em_alex", "Alex", "es", "male"),
            kokoro_voice(&repo_id, "af_heart", "Heart", "en-US", "female"),
            kokoro_voice(&repo_id, "am_adam", "Adam", "en-US", "male"),
        ],
        _ => Vec::new(),
    };

    let lang = draft.language.trim().to_string();
    let languages = if lang.is_empty() {
        vec!["es".to_string(), "en".to_string()]
    } else {
        vec![lang]
    };

    let streaming = match engine.as_str() {
        "whisper" => "windowed",
        "kokoro" | "parakeet" | "moonshine-seq2seq" => "chunked",
        _ => "native",
    }
    .to_string();

    let spec = VoiceModelSpec {
        id,
        kind: draft.kind,
        engine,
        family: name.to_string(),
        variant_label: "Custom".to_string(),
        name: name.to_string(),
        description: format!("Custom model ({repo_id})"),
        params: "custom".to_string(),
        languages,
        streaming,
        license: "Custom".to_string(),
        repo_id,
        approx_bytes: 0,
        min_ram_gb: 4,
        runtime_ready: true,
        files,
        voices,
        custom: true,
    };

    let mut customs = read_custom_models(data_dir);
    customs.push(spec.clone());
    write_custom_models(data_dir, &customs)?;

    let host = detect_host_hardware();
    Ok(inspect_model(data_dir, &spec, &HashSet::new(), &host))
}

/// Deletes either one voice pack or an entire model directory from disk.
/// If a custom model is deleted as a whole, its entry is also removed from `custom-models.json`.
pub fn delete_model(
    data_dir: &Path,
    model_id: &str,
    voice_id: Option<&str>,
) -> Result<bool, String> {
    let Some(spec) = find_model_spec(data_dir, model_id) else {
        return Ok(false);
    };
    let base = model_dir(data_dir, spec.kind, &spec.id);

    if let Some(vid) = voice_id {
        let Some(voice) = spec.voices.iter().find(|v| v.id == vid) else {
            return Ok(false);
        };
        let path = base.join(&voice.rel_path);
        if path.is_file() {
            std::fs::remove_file(&path).map_err(|e| format!("cannot remove voice file: {e}"))?;
            return Ok(true);
        }
        return Ok(false);
    }

    let mut removed = false;
    if base.is_dir() {
        std::fs::remove_dir_all(&base).map_err(|e| format!("cannot remove model directory: {e}"))?;
        removed = true;
    }
    if spec.custom {
        let mut customs = read_custom_models(data_dir);
        let before = customs.len();
        customs.retain(|m| m.id != model_id);
        if customs.len() != before {
            write_custom_models(data_dir, &customs)?;
            removed = true;
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "nabla_voice_catalog_{}_{tag}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn curated_catalog_lists_kokoro_and_moonshine_models() {
        let dir = temp_dir("curated");
        let entries = list_catalog(&dir, &HashSet::new());
        let ids: Vec<&str> = entries.iter().map(|e| e.id.as_str()).collect();
        assert!(ids.contains(&"kokoro-82m"));
        assert!(ids.contains(&"moonshine-tiny-es"));
        assert!(ids.contains(&"moonshine-small-es"));
        assert!(ids.contains(&"moonshine-tiny-en"));

        let kokoro = entries.iter().find(|e| e.id == "kokoro-82m").unwrap();
        assert_eq!(kokoro.status, "not_installed");
        assert!(kokoro.voices.iter().any(|v| v.id == "ef_dora"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn installed_status_reflects_files_and_voices_on_disk() {
        let dir = temp_dir("disk_status");
        let base = model_dir(&dir, VoiceModelKind::Tts, "kokoro-82m");
        std::fs::create_dir_all(base.join("onnx")).unwrap();
        std::fs::create_dir_all(base.join("voices")).unwrap();
        std::fs::write(base.join("onnx/model_quantized.onnx"), b"fake-onnx").unwrap();
        std::fs::write(base.join("tokenizer.json"), b"{}").unwrap();
        std::fs::write(base.join("voices/ef_dora.bin"), b"fake-voice").unwrap();

        let entries = list_catalog(&dir, &HashSet::new());
        let kokoro = entries.iter().find(|e| e.id == "kokoro-82m").unwrap();
        assert_eq!(kokoro.status, "installed");
        assert!(kokoro.installed_bytes >= 19);
        assert!(kokoro.voices.iter().find(|v| v.id == "ef_dora").unwrap().installed);
        assert!(!kokoro.voices.iter().find(|v| v.id == "em_alex").unwrap().installed);

        assert!(delete_model(&dir, "kokoro-82m", Some("ef_dora")).unwrap());
        let after = list_catalog(&dir, &HashSet::new());
        let kokoro_after = after.iter().find(|e| e.id == "kokoro-82m").unwrap();
        assert_eq!(kokoro_after.status, "installed");
        assert!(!kokoro_after.voices.iter().find(|v| v.id == "ef_dora").unwrap().installed);

        assert!(delete_model(&dir, "kokoro-82m", None).unwrap());
        let after_all = list_catalog(&dir, &HashSet::new());
        assert_eq!(
            after_all.iter().find(|e| e.id == "kokoro-82m").unwrap().status,
            "not_installed"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn custom_model_import_validates_and_persists() {
        let dir = temp_dir("custom");
        assert!(import_custom_model(
            &dir,
            CustomVoiceModelDraft {
                kind: VoiceModelKind::Stt,
                engine: "moonshine-streaming".to_string(),
                name: "Bad HTTP".to_string(),
                repo_or_url: "http://example.com/model.onnx".to_string(),
                language: "es".to_string(),
            },
        )
        .is_err());

        let imported = import_custom_model(
            &dir,
            CustomVoiceModelDraft {
                kind: VoiceModelKind::Stt,
                engine: "moonshine-streaming".to_string(),
                name: "My Spanish STT".to_string(),
                repo_or_url: "https://huggingface.co/someone/my-moonshine-es".to_string(),
                language: "es".to_string(),
            },
        )
        .expect("import custom model");
        assert_eq!(imported.id, "custom-my-spanish-stt");
        assert_eq!(imported.repo_id, "someone/my-moonshine-es");
        assert!(imported.custom);

        let all = list_catalog(&dir, &HashSet::new());
        assert!(all.iter().any(|m| m.id == "custom-my-spanish-stt"));

        assert!(delete_model(&dir, "custom-my-spanish-stt", None).unwrap());
        let after = list_catalog(&dir, &HashSet::new());
        assert!(!after.iter().any(|m| m.id == "custom-my-spanish-stt"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
