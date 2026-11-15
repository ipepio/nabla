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

impl Default for Settings {
    fn default() -> Self {
        Settings {
            theme: "dark".to_string(),
            pi_binary: "pi".to_string(),
            provider: default_provider(),
            model: default_model(),
        }
    }
}

#[derive(Debug)]
pub enum SettingsError {
    Io(String),
    Parse(String),
}

impl std::fmt::Display for SettingsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SettingsError::Io(m) => write!(f, "settings io error: {m}"),
            SettingsError::Parse(m) => write!(f, "settings parse error: {m}"),
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
