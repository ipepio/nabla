// Settings module: user-editable preferences (settings.json in app_config_dir).
// Rule (see .docs/02-decisiones.md): "files are for what the human configures;
// the DB is for what the app accumulates." Settings load whole, write whole.
// Unknown fields are tolerated (forward-compatible) so older builds still parse
// files written by newer ones.

use std::path::Path;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    /// UI theme: "dark" | "light" | "system".
    pub theme: String,
    /// pi binary name or path; resolved at runtime (see .docs/03-referencias.md,
    /// FaqFirebase binary resolution as future reference).
    pub pi_binary: String,
    /// S7: pi provider for new sessions. "nan" is the provider configured in
    /// `~/.pi/agent/models.json`; the selector lists its models.
    #[serde(default = "default_provider")]
    pub provider: String,
    /// S7: model id inside `provider` for new sessions.
    #[serde(default = "default_model")]
    pub model: String,
    /// S2 of `i18n-and-settings-polish.md`: interface language, `"en"` | `"es"`.
    /// The UI normalizes anything else to English; the file keeps what was
    /// written, so a locale added later is not silently rewritten here.
    #[serde(default = "default_language")]
    pub language: String,
    /// Worksheet S2: models the human hid from the selector, as
    /// `"<provider>/<modelId>"` pairs. Nabla's own preference, not pi's: the
    /// catalogues Nabla lists are read-only, so hiding never edits pi's files and
    /// works for a configured and a catalog model alike. An absent field (a file
    /// written before this preference existed) hides nothing.
    #[serde(default)]
    pub hidden_models: Vec<String>,
    /// S2 of `chat-preferences-and-queue.md`: the "Mostrar razonamiento"
    /// toggle. Off, the reasoning block is not rendered (neither live nor in
    /// recorded history). An absent field (a file written before this
    /// preference existed) means `true`, like pi and a fresh install.
    #[serde(default = "default_true")]
    pub show_reasoning: bool,
    /// S3 of `chat-preferences-and-queue.md`: the "Encolar mensajes" toggle.
    /// On, a message sent while the agent is working is queued for the session
    /// instead of being rejected. An absent field means `true`.
    #[serde(default = "default_true")]
    pub queue_messages: bool,
    /// S5 of `tool-drawer-and-mcp.md`: which drawer tools are active, by tool id
    /// (`files`, `changes`, `terminal`, `browser`). Files and Changes ship on; the
    /// two tools that reach outside the app (a real shell, a driven browser) are
    /// opt-in. An absent field takes `default_tools`, and an unknown id is kept
    /// verbatim — a tool this build does not know is not erased from the file.
    #[serde(default = "default_tools")]
    pub enabled_tools: Vec<String>,
}

/// S2/S3 of `chat-preferences-and-queue.md`: both chat toggles ship enabled, so
/// a settings file written before they existed keeps today's behaviour with
/// reasoning shown and sending turned into a queue.
fn default_true() -> bool {
    true
}

/// S7 default provider. Also the value a settings file written before this
/// preference existed gets, so an upgrade never rejects an older file.
fn default_provider() -> String {
    "nan".to_string()
}

/// S7 default model inside `default_provider`.
fn default_model() -> String {
    "deepseek-v4-flash".to_string()
}

/// S2 of `i18n-and-settings-polish.md`: English is the default interface
/// language, so a settings file written before the preference existed (or a
/// fresh install) opens in `en`.
fn default_language() -> String {
    "en".to_string()
}

/// S5 of `tool-drawer-and-mcp.md`: the tools a fresh install shows. Terminal and
/// Browser stay out on purpose — they run arbitrary commands and drive a real
/// browser, so the human turns them on explicitly in Settings.
pub fn default_tools() -> Vec<String> {
    vec!["files".to_string(), "changes".to_string()]
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            theme: "dark".to_string(),
            pi_binary: "pi".to_string(),
            provider: default_provider(),
            model: default_model(),
            language: default_language(),
            // Nothing is hidden until the human hides it.
            hidden_models: Vec::new(),
            show_reasoning: default_true(),
            queue_messages: default_true(),
            enabled_tools: default_tools(),
        }
    }
}

#[derive(Debug)]
pub enum SettingsError {
    Io(String),
    Parse(String),
    /// S3 of `model-visibility.md`: a hidden-model selection needs both halves of
    /// the pair, so a blank provider or model id never reaches the file.
    InvalidHiddenModel(String),
}

impl std::fmt::Display for SettingsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SettingsError::Io(m) => write!(f, "settings io error: {m}"),
            SettingsError::Parse(m) => write!(f, "settings parse error: {m}"),
            SettingsError::InvalidHiddenModel(entry) => write!(
                f,
                "hidden model entry needs a provider and a model id: {entry:?}"
            ),
        }
    }
}

impl std::error::Error for SettingsError {}

pub type SettingsResult<T> = Result<T, SettingsError>;

/// Loads settings from `path`; returns defaults when the file does not exist.
pub fn load(path: &Path) -> SettingsResult<Settings> {
    match std::fs::read_to_string(path) {
        Ok(raw) => {
            serde_json::from_str(&raw).map_err(|e| SettingsError::Parse(e.to_string()))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Settings::default()),
        Err(e) => Err(SettingsError::Io(e.to_string())),
    }
}

/// Saves settings atomically: write temp file, then rename over the target.
pub fn save(path: &Path, settings: &Settings) -> SettingsResult<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| SettingsError::Io(e.to_string()))?;
    }
    let json = serde_json::to_string_pretty(settings).map_err(|e| SettingsError::Parse(e.to_string()))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json).map_err(|e| SettingsError::Io(e.to_string()))?;
    std::fs::rename(&tmp, path).map_err(|e| SettingsError::Io(e.to_string()))?;
    Ok(())
}

/// S2 of `model-visibility.md`: the `"<provider>/<modelId>"` key of one hidden
/// model. One function builds it, so the settings file and the listing can never
/// disagree about the format. A provider id never carries a `/` (pi's own
/// discipline), so the first separator is unambiguous even though a model id may
/// carry slashes (`meta-llama/llama-3.2-1b`).
pub fn hidden_model_key(provider: &str, model_id: &str) -> String {
    format!("{}/{}", provider.trim(), model_id.trim())
}

/// S2/S3 of `model-visibility.md`: adds or removes one hidden model in the
/// settings file and answers whether the file changed.
///
/// Hiding is Nabla state, never pi's: the pair is stored here and nothing is
/// written to `models.json` or the cached catalogue. Both halves are trimmed and
/// must be non-empty (`InvalidHiddenModel`), otherwise nothing is read or
/// written. The operation is idempotent — hiding a hidden model and unhiding a
/// visible one are no-ops that write nothing and answer `false` — and the list
/// never holds a duplicate.
pub fn set_hidden_model(
    path: &Path,
    provider: &str,
    model_id: &str,
    hidden: bool,
) -> SettingsResult<bool> {
    let provider = provider.trim();
    let model_id = model_id.trim();
    if provider.is_empty() || model_id.is_empty() {
        return Err(SettingsError::InvalidHiddenModel(format!(
            "{provider}/{model_id}"
        )));
    }
    let key = hidden_model_key(provider, model_id);

    let mut settings = load(path)?;
    let changed = if hidden {
        if settings.hidden_models.iter().any(|entry| entry == &key) {
            false
        } else {
            settings.hidden_models.push(key);
            true
        }
    } else {
        let before = settings.hidden_models.len();
        settings.hidden_models.retain(|entry| entry != &key);
        settings.hidden_models.len() != before
    };
    // A no-op writes nothing, so an existing file stays byte-identical.
    if changed {
        save(path, &settings)?;
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempCfg(std::path::PathBuf);
    impl TempCfg {
        fn new(tag: &str) -> Self {
            let mut p = std::env::temp_dir();
            p.push(format!(
                "nabla_settings_{}_{}_{tag}.json",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            TempCfg(p)
        }
    }
    impl Drop for TempCfg {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    #[test]
    fn missing_file_returns_defaults() {
        let cfg = TempCfg::new("missing");
        let s = load(&cfg.0).expect("load");
        assert_eq!(s, Settings::default());
        assert_eq!(s.theme, "dark");
        assert_eq!(s.pi_binary, "pi");
        // S7: the default engine configuration is the NaN provider and its
        // default model, so a fresh install spawns pi with something usable.
        assert_eq!(s.provider, "nan");
        assert_eq!(s.model, "deepseek-v4-flash");
        // S2 of `i18n-and-settings-polish.md`: the interface starts in English.
        assert_eq!(s.language, "en");
    }

    // S7: provider and model are persisted alongside the rest of the settings.
    #[test]
    fn save_then_load_round_trips() {
        let cfg = TempCfg::new("roundtrip");
        let mut s = Settings::default();
        s.theme = "light".into();
        s.pi_binary = "/opt/pi/bin/pi".into();
        s.provider = "nan".into();
        s.model = "glm5.3".into();
        save(&cfg.0, &s).expect("save");
        let loaded = load(&cfg.0).expect("load");
        assert_eq!(loaded, s);
    }

    // S7: the file is camelCase, so the TS contract mirrors these keys exactly.
    #[test]
    fn serialized_keys_are_camel_case_and_include_provider_and_model() {
        let json = serde_json::to_value(Settings::default()).expect("serialize");
        assert_eq!(json["piBinary"], serde_json::json!("pi"));
        assert_eq!(json["provider"], serde_json::json!("nan"));
        assert_eq!(json["model"], serde_json::json!("deepseek-v4-flash"));
    }

    #[test]
    fn unknown_fields_are_tolerated_not_fatal() {
        let cfg = TempCfg::new("unknown_fields");
        std::fs::write(
            &cfg.0,
            r#"{"theme":"light","piBinary":"pi","provider":"nan","model":"glm5.3","futureField":{"x":1}}"#,
        )
        .expect("write fixture");
        let s = load(&cfg.0).expect("load with unknown field must succeed");
        assert_eq!(s.theme, "light");
        assert_eq!(s.pi_binary, "pi");
        // S7: a settings file written by hand sets the engine to use.
        assert_eq!(s.provider, "nan");
        assert_eq!(s.model, "glm5.3");
        // S2/S3 of `chat-preferences-and-queue.md`: an unknown field does not
        // stop the known ones (or the new defaults) from being read.
        assert!(s.show_reasoning);
        assert!(s.queue_messages);
    }

    // S7 + settings header rule: a file written before provider/model existed
    // must still load. Missing fields take their defaults; the file is never
    // rejected because a newer build added a preference.
    #[test]
    fn settings_file_without_provider_and_model_uses_defaults() {
        let cfg = TempCfg::new("pre_provider");
        std::fs::write(&cfg.0, r#"{"theme":"light","piBinary":"/opt/pi"}"#)
            .expect("write fixture");
        let s = load(&cfg.0).expect("an older settings file must load");
        assert_eq!(s.theme, "light");
        assert_eq!(s.pi_binary, "/opt/pi");
        assert_eq!(s.provider, "nan");
        assert_eq!(s.model, "deepseek-v4-flash");
    }

    // S2/S3 of `chat-preferences-and-queue.md`: both chat preferences ship on,
    // like pi: reasoning visible and sending while the agent works queues.
    #[test]
    fn defaults_show_reasoning_and_queue_messages() {
        let s = Settings::default();
        assert!(
            s.show_reasoning,
            "S2: the reasoning block is shown by default"
        );
        assert!(
            s.queue_messages,
            "S3: sending while the agent works queues by default"
        );

        // camelCase on the wire, so `preferences.ts` mirrors these keys.
        let json = serde_json::to_value(Settings::default()).expect("serialize");
        assert_eq!(json["showReasoning"], serde_json::json!(true));
        assert_eq!(json["queueMessages"], serde_json::json!(true));
    }

    // Settings header rule + S2/S3: a file written before these preferences
    // existed must load, with both toggles on and every other key preserved.
    #[test]
    fn settings_file_without_chat_preferences_uses_the_true_defaults() {
        let cfg = TempCfg::new("pre_chat_prefs");
        std::fs::write(
            &cfg.0,
            r#"{"theme":"light","piBinary":"pi","provider":"nan","model":"glm5.3","language":"es"}"#,
        )
        .expect("write fixture");
        let s = load(&cfg.0).expect("an older settings file must load");
        assert_eq!(s.language, "es");
        assert!(s.show_reasoning, "an absent preference means reasoning on");
        assert!(s.queue_messages, "an absent preference means queueing on");
    }

    // S2/S3: `false` is a real choice, so it survives save then load — the
    // toggle would otherwise flip back on every relaunch.
    #[test]
    fn chat_preferences_round_trip_as_false() {
        let cfg = TempCfg::new("chat_prefs_roundtrip");
        let s = Settings {
            show_reasoning: false,
            queue_messages: false,
            ..Settings::default()
        };
        save(&cfg.0, &s).expect("save");

        let loaded = load(&cfg.0).expect("load");
        assert!(!loaded.show_reasoning, "reasoning stays hidden");
        assert!(!loaded.queue_messages, "sending stays blocked");

        let json = serde_json::to_value(&loaded).expect("serialize");
        assert_eq!(json["showReasoning"], serde_json::json!(false));
        assert_eq!(json["queueMessages"], serde_json::json!(false));
    }

    // S2 of `i18n-and-settings-polish.md`: the language is a preference like the
    // others — saved with the file and read back unchanged.
    #[test]
    fn language_is_saved_and_read_back() {
        let cfg = TempCfg::new("language_roundtrip");
        let s = Settings {
            language: "es".into(),
            ..Settings::default()
        };
        save(&cfg.0, &s).expect("save");
        let loaded = load(&cfg.0).expect("load");
        assert_eq!(loaded.language, "es");
    }

    // S2: the key is `language`, so the TS contract mirrors it field by field.
    #[test]
    fn serialized_language_key_is_camel_case() {
        let json = serde_json::to_value(Settings::default()).expect("serialize");
        assert_eq!(json["language"], serde_json::json!("en"));
    }

    // S2 + settings header rule: a file written before `language` existed must
    // still load, and it keeps working in the default locale instead of failing.
    #[test]
    fn settings_file_without_language_uses_the_english_default() {
        let cfg = TempCfg::new("pre_language");
        std::fs::write(
            &cfg.0,
            r#"{"theme":"light","piBinary":"pi","provider":"nan","model":"glm5.3"}"#,
        )
        .expect("write fixture");
        let s = load(&cfg.0).expect("an older settings file must load");
        assert_eq!(s.theme, "light");
        assert_eq!(s.model, "glm5.3");
        assert_eq!(s.language, "en");
    }

    // ── enabled drawer tools (S5 of `tool-drawer-and-mcp.md`) ───────────────

    // S5: a fresh install opens with Files and Changes on and the two opt-in
    // tools (Terminal, Browser) off, which is exactly this list.
    #[test]
    fn default_enabled_tools_are_files_and_changes() {
        let s = Settings::default();
        assert_eq!(
            s.enabled_tools,
            vec!["files".to_string(), "changes".to_string()],
            "S5: Files and Changes ship on; Terminal and Browser are opt-in"
        );

        // camelCase on the wire, so `preferences.ts` mirrors this key.
        let json = serde_json::to_value(&s).expect("serialize");
        assert_eq!(
            json["enabledTools"],
            serde_json::json!(["files", "changes"])
        );
    }

    // S5: the switches are a preference like the others — the list is written
    // verbatim and read back unchanged, including the opt-in tools.
    #[test]
    fn enabled_tools_round_trip() {
        let cfg = TempCfg::new("enabled_tools_roundtrip");
        let s = Settings {
            enabled_tools: vec![
                "files".to_string(),
                "terminal".to_string(),
                "browser".to_string(),
            ],
            ..Settings::default()
        };
        save(&cfg.0, &s).expect("save");

        let loaded = load(&cfg.0).expect("load");
        assert_eq!(
            loaded.enabled_tools,
            vec![
                "files".to_string(),
                "terminal".to_string(),
                "browser".to_string()
            ],
            "S5: a tool turned on stays on across relaunches"
        );
        // An empty list is a real choice too: every tool turned off.
        save(
            &cfg.0,
            &Settings {
                enabled_tools: Vec::new(),
                ..Settings::default()
            },
        )
        .expect("save");
        assert!(!load(&cfg.0).expect("load").enabled_tools.contains(&"files".to_string()));
    }

    // S5 + settings header rule: a file written before the switches existed must
    // still load, showing the same tools a fresh install shows.
    #[test]
    fn settings_file_without_enabled_tools_uses_the_default_tools() {
        let cfg = TempCfg::new("pre_enabled_tools");
        std::fs::write(
            &cfg.0,
            r#"{"theme":"light","piBinary":"pi","provider":"nan","model":"glm5.3","language":"es"}"#,
        )
        .expect("write fixture");
        let s = load(&cfg.0).expect("an older settings file must load");
        assert_eq!(s.theme, "light");
        assert_eq!(
            s.enabled_tools,
            vec!["files".to_string(), "changes".to_string()],
            "an absent key means the shipped set, never an empty drawer"
        );
    }

    // ── hidden models (S2 of `model-visibility.md`) ─────────────────────────

    // S2 + settings header rule: a file written before the preference existed
    // must still load, with nothing hidden.
    #[test]
    fn settings_file_without_hidden_models_loads_an_empty_list() {
        let cfg = TempCfg::new("pre_hidden");
        std::fs::write(
            &cfg.0,
            r#"{"theme":"light","piBinary":"pi","provider":"nan","model":"glm5.3","language":"es"}"#,
        )
        .expect("write fixture");
        let s = load(&cfg.0).expect("an older settings file must load");
        assert_eq!(s.language, "es");
        assert!(
            s.hidden_models.is_empty(),
            "nothing is hidden until the human hides it: {:?}",
            s.hidden_models
        );
    }

    // S2: the list is a preference like the others — camelCase on the wire, kept
    // verbatim by the file.
    #[test]
    fn hidden_models_round_trip_and_serialize_camel_case() {
        let cfg = TempCfg::new("hidden_roundtrip");
        let s = Settings {
            hidden_models: vec![
                hidden_model_key("nan", "glm5.3"),
                hidden_model_key("openai", "gpt-5.4"),
            ],
            ..Settings::default()
        };
        save(&cfg.0, &s).expect("save");
        let loaded = load(&cfg.0).expect("load");
        assert_eq!(loaded, s);
        assert_eq!(
            loaded.hidden_models,
            vec!["nan/glm5.3".to_string(), "openai/gpt-5.4".to_string()],
            "an entry is `<provider>/<modelId>`"
        );

        let json = serde_json::to_value(Settings::default()).expect("serialize");
        assert_eq!(json["hiddenModels"], serde_json::json!([]));
    }

    // S2: hiding is idempotent state, never a log: the pair is stored once and a
    // second identical request changes nothing (so no write is reported).
    #[test]
    fn set_model_hidden_adds_the_pair_once() {
        let cfg = TempCfg::new("hidden_add");

        assert!(
            set_hidden_model(&cfg.0, "nan", "glm5.3", true).expect("hide"),
            "the first hide writes the settings file"
        );
        assert_eq!(
            load(&cfg.0).expect("load").hidden_models,
            vec!["nan/glm5.3".to_string()]
        );

        assert!(
            !set_hidden_model(&cfg.0, "nan", "glm5.3", true).expect("hide again"),
            "hiding an already hidden model changes nothing"
        );
        assert_eq!(
            load(&cfg.0).expect("load").hidden_models,
            vec!["nan/glm5.3".to_string()],
            "no duplicate entry is appended"
        );

        // Surrounding whitespace is the same pair.
        assert!(!set_hidden_model(&cfg.0, " nan ", " glm5.3 ", true).expect("trimmed"));
        assert_eq!(load(&cfg.0).expect("load").hidden_models.len(), 1);
    }

    // S2: unhiding removes exactly that pair and leaves the rest of the list.
    #[test]
    fn set_model_hidden_removes_only_the_pair_asked_for() {
        let cfg = TempCfg::new("hidden_remove");
        let s = Settings {
            hidden_models: vec!["nan/glm5.3".to_string(), "nan/qwen3.6".to_string()],
            ..Settings::default()
        };
        save(&cfg.0, &s).expect("save");

        assert!(
            set_hidden_model(&cfg.0, "nan", "glm5.3", false).expect("unhide"),
            "the pair was hidden, so the file changes"
        );
        assert_eq!(
            load(&cfg.0).expect("load").hidden_models,
            vec!["nan/qwen3.6".to_string()]
        );

        assert!(
            !set_hidden_model(&cfg.0, "nan", "glm5.3", false).expect("unhide again"),
            "unhiding a visible model changes nothing"
        );
        assert_eq!(
            load(&cfg.0).expect("load").hidden_models,
            vec!["nan/qwen3.6".to_string()]
        );
    }

    // S3: a pair without both halves is not a model, so it never reaches the file.
    #[test]
    fn set_model_hidden_rejects_a_blank_provider_or_model_id() {
        let cfg = TempCfg::new("hidden_blank");
        for (provider, model_id) in [("", "glm5.3"), ("nan", ""), ("  ", "  "), ("", "")] {
            match set_hidden_model(&cfg.0, provider, model_id, true) {
                Err(SettingsError::InvalidHiddenModel(_)) => {}
                other => panic!(
                    "expected InvalidHiddenModel for ({provider:?}, {model_id:?}), got: {other:?}"
                ),
            }
        }
        assert!(
            !cfg.0.exists(),
            "a rejected request writes no settings file"
        );
    }

    #[test]
    fn malformed_json_is_a_parse_error_not_defaults() {
        let cfg = TempCfg::new("malformed");
        std::fs::write(&cfg.0, "{ not json").expect("write fixture");
        match load(&cfg.0) {
            Err(SettingsError::Parse(_)) => {}
            other => panic!("expected Parse error, got: {other:?}"),
        }
    }
}
