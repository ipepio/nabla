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
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            theme: "dark".to_string(),
            pi_binary: "pi".to_string(),
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
    }

    #[test]
    fn save_then_load_round_trips() {
        let cfg = TempCfg::new("roundtrip");
        let mut s = Settings::default();
        s.theme = "light".into();
        s.pi_binary = "/opt/pi/bin/pi".into();
        save(&cfg.0, &s).expect("save");
        let loaded = load(&cfg.0).expect("load");
        assert_eq!(loaded, s);
    }

    #[test]
    fn unknown_fields_are_tolerated_not_fatal() {
        let cfg = TempCfg::new("unknown_fields");
        std::fs::write(
            &cfg.0,
            r#"{"theme":"light","piBinary":"pi","futureField":{"x":1}}"#,
        )
        .expect("write fixture");
        let s = load(&cfg.0).expect("load with unknown field must succeed");
        assert_eq!(s.theme, "light");
        assert_eq!(s.pi_binary, "pi");
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
