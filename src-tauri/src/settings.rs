// Settings module: user-editable preferences (settings.json in app_config_dir).
// Rule (see .docs/02-decisiones.md): "files are for what the human configures;
// the DB is for what the app accumulates." Settings load whole, write whole.
// Unknown fields are tolerated (forward-compatible) so older builds still parse
// files written by newer ones.
//
// The permission modes also live here, in two pieces: the preference the human
// sets (`Settings::permission_mode`) and the decision table that documents what
// each mode allows (`decide`). The enforcement is the pi extension in
// `resources/nabla-permissions.ts`, which carries its own copy of that table —
// the mode reaches it through `<data_dir>/permissions.json`, never through the
// spawn's environment (L4 of `permission-modes.md`: env is fixed at spawn time,
// so a mode baked into it could not change without restarting the session).

use std::path::{Path, PathBuf};

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
    /// S1/S2 of `permission-modes.md`: what the agent may do without asking,
    /// `"read"` | `"write"` | `"yolo"`. Kept as the raw string the file holds,
    /// like `language`: an unrecognized value is never rewritten here and reads
    /// as `"write"` (`Settings::mode`), which is the fail-safe direction (L4).
    /// An absent field (a file written before this preference existed) means
    /// `"write"`.
    #[serde(default = "default_permission_mode")]
    pub permission_mode: String,
    /// S5 of `tool-drawer-and-mcp.md`: which drawer tools are active, by tool id
    /// (`files`, `changes`, `terminal`, `browser`). Files and Changes ship on; the
    /// two tools that reach outside the app (a real shell, a driven browser) are
    /// opt-in. An absent field takes `default_tools`, and an unknown id is kept
    /// verbatim — a tool this build does not know is not erased from the file.
    #[serde(default = "default_tools")]
    pub enabled_tools: Vec<String>,
    /// S2 of `composer-inputs.md`: the thinking level (effort) a NEW session
    /// starts with, forwarded to pi as `--thinking <level>`. Kept as the raw
    /// string the file holds, like `language` and `permission_mode`: a value this
    /// build does not know is never rewritten here and reads as "no level"
    /// (`Settings::thinking`), which makes the spawn omit the flag and leaves pi
    /// on its own default. An absent field means `"off"` — the maintainer's
    /// standing default: pi has no per-model level, so a static non-off value
    /// would impose reasoning nobody asked for on every fresh session.
    #[serde(default = "default_thinking_level")]
    pub thinking_level: String,
    /// S1 of `web-search.md`: the agent's `web_search` tool. **Off** by default:
    /// the query leaves this machine for a third-party server, so a network
    /// capability is something the human turns on, never something an upgrade
    /// grants. An absent field (a file written before this preference existed)
    /// means off.
    #[serde(default)]
    pub web_search_enabled: bool,
    /// S1: the SearXNG instance that answers `web_search`, as the human typed it
    /// (a bare host is accepted; `websearch::normalize_instance` adds the
    /// scheme). **Empty means "any public instance"**, which is the best-effort
    /// path: public instances rate-limit or refuse the JSON API, so a configured
    /// instance is the reliable one. Kept verbatim here, trimmed by the reader.
    #[serde(default)]
    pub web_search_instance: String,
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

/// S6 of `permission-modes.md`: write is the default, so a fresh install (and a
/// settings file written before the preference existed) keeps today's behaviour
/// for files and starts asking before commands — the intended change of S6.
fn default_permission_mode() -> String {
    PermissionMode::Write.as_str().to_string()
}

/// S2 of `composer-inputs.md`: the levels pi documents for `--thinking`
/// (pi docs/cli.md:71). One list, so the command that validates a request and
/// the spawn that forwards it can never disagree about the vocabulary.
pub const THINKING_LEVELS: &[&str] = &["off", "minimal", "low", "medium", "high", "xhigh", "max"];

/// S2: the level a fresh install, or a settings file written before this
/// preference existed, starts with. `off`, the maintainer's decision: no
/// per-model default exists in pi, so a static non-off level would impose
/// reasoning on every fresh session that nobody asked for. `off` is the cheap
/// start, and the human raises it deliberately when a session deserves it.
fn default_thinking_level() -> String {
    "off".to_string()
}

/// S2: parses one `--thinking` word, trimmed and case-insensitively. `None` for
/// anything else, so a caller that must not guess (the command that applies the
/// human's choice) can reject it, and the spawn can omit the flag instead of
/// handing pi a word it would ignore.
pub fn parse_thinking_level(raw: &str) -> Option<&'static str> {
    let word = raw.trim().to_ascii_lowercase();
    THINKING_LEVELS
        .iter()
        .copied()
        .find(|level| *level == word)
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
            permission_mode: default_permission_mode(),
            enabled_tools: default_tools(),
            thinking_level: default_thinking_level(),
            // S1 of `web-search.md`: nothing reaches the network until the human
            // asks for it, and no instance is preferred over any other.
            web_search_enabled: false,
            web_search_instance: String::new(),
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

// ── permission modes (S1-S6 of `permission-modes.md`) ──────────────────────
//
// The ENFORCEMENT of these modes is the pi extension in
// `resources/nabla-permissions.ts`; what lives here is the preference the human
// sets and the decision table that documents the contract the extension mirrors
// (its own copy, because a TypeScript file loaded by pi cannot call Rust). The
// items below therefore have no in-process caller: they are the table's written
// form plus its tests, which is why the dead-code lint is silenced item by item
// here and nowhere else.

/// S1 of `permission-modes.md`: the three permission modes the human picks from
/// the chat header. `read` only inspects, `write` edits files and asks before a
/// command, `yolo` asks nothing (and says so, S5).
///
/// `Default` is `Write` (S6) and is also what an unknown stored value reads as:
/// a typo, a truncated write or a value from another build must never degrade
/// into `yolo` (L4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PermissionMode {
    Read,
    #[default]
    Write,
    Yolo,
}

impl PermissionMode {
    /// Parses one mode word. `None` for anything else, so a caller that must not
    /// guess (the command that persists the human's choice) can reject it.
    pub fn parse(raw: &str) -> Option<PermissionMode> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "read" => Some(PermissionMode::Read),
            "write" => Some(PermissionMode::Write),
            "yolo" => Some(PermissionMode::Yolo),
            _ => None,
        }
    }

    /// The word the settings file, the projection and the UI all use.
    pub fn as_str(self) -> &'static str {
        match self {
            PermissionMode::Read => "read",
            PermissionMode::Write => "write",
            PermissionMode::Yolo => "yolo",
        }
    }
}

/// What one tool call may do under one mode (S3-S5). `Allow` permits it, `Block`
/// vetoes it with the reason the model reads (pi turns it into the tool's error
/// text), and `Ask` hands the decision to the human through the extension's
/// `ctx.ui.confirm`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)] // contract mirror: the decision is enforced by the pi extension
pub enum PermissionDecision {
    Allow,
    Block(String),
    Ask,
}

/// L3 of `permission-modes.md`: the tools that only inspect, free in every mode.
#[allow(dead_code)] // contract mirror: read by `decide`, which is itself a mirror
pub const FREE_TOOLS: &[&str] = &["read", "grep", "find", "ls"];

/// L3: the tools that run a command: blocked in `read`, asked in `write`.
#[allow(dead_code)] // contract mirror
pub const SHELL_TOOLS: &[&str] = &["bash", "powershell"];

/// L3: the tools that write a file: blocked in `read`, allowed in `write`.
#[allow(dead_code)] // contract mirror
pub const FILE_TOOLS: &[&str] = &["edit", "write"];

/// S3: the veto text for `read` mode. The model reads it as the failed tool's
/// error, so it has to say what happened, why, and where the human changes it.
/// The pi extension carries the same wording: this is the contract it mirrors.
#[allow(dead_code)] // contract mirror: the extension prints this same text
pub fn read_mode_reason(tool: &str) -> String {
    format!(
        "Nabla is in \"read\" mode, so {tool} was blocked and nothing was executed or \
         written. Read mode only allows inspecting (read, grep, find, ls). The human \
         set this mode with the permission chip in the chat header and can change it \
         there; tell them what you need instead of retrying the same call."
    )
}

/// S4: the veto text for a command the human did not approve in `write` mode.
#[allow(dead_code)] // contract mirror: the extension prints this same text
pub fn command_denied_reason(tool: &str) -> String {
    format!(
        "The human did not approve that {tool} command, so it was not run. Do not \
         retry it as is: ask what to do differently, or propose a step that does not \
         need the command."
    )
}

/// S3-S5: the decision table, as the loaded extension implements it.
///
/// `Allow` and `Block` are the whole truth for `read` and `yolo`; `Ask` is what
/// `write` mode answers for a command, and only the extension can turn it into a
/// question (the chip reads this table to stay honest about what each mode does).
/// A tool this build does not name is never gated in v1 (L3): the browser bridge
/// and the MCP tools reach outside the workspace and are decided in their own
/// feature, so blocking them here would claim a decision nobody made.
#[allow(dead_code)] // contract mirror: tests pin the table the extension implements
pub fn decide(mode: PermissionMode, tool: &str) -> PermissionDecision {
    let tool = tool.trim();
    if FREE_TOOLS.contains(&tool) {
        return PermissionDecision::Allow;
    }
    match mode {
        PermissionMode::Yolo => PermissionDecision::Allow,
        PermissionMode::Read => {
            if SHELL_TOOLS.contains(&tool) || FILE_TOOLS.contains(&tool) {
                PermissionDecision::Block(read_mode_reason(tool))
            } else {
                PermissionDecision::Allow
            }
        }
        PermissionMode::Write => {
            if SHELL_TOOLS.contains(&tool) {
                PermissionDecision::Ask
            } else {
                PermissionDecision::Allow
            }
        }
    }
}

impl Settings {
    /// S2/L4: the mode the file holds, with an unknown value reading as `write`
    /// (never `yolo`): a hand edit that is not one of the three words must not
    /// uncage the agent.
    pub fn mode(&self) -> PermissionMode {
        PermissionMode::parse(&self.permission_mode).unwrap_or_default()
    }

    /// S2 of `composer-inputs.md`: the thinking level the file names, or `None`
    /// when it holds a word this build does not know. The spawn omits `--thinking`
    /// in that case and pi keeps its own default: guessing a level would change
    /// the agent's reasoning on a typo's behalf.
    pub fn thinking(&self) -> Option<&'static str> {
        parse_thinking_level(&self.thinking_level)
    }
}

/// L4: where the loaded pi extension reads the current mode. `<data_dir>` is the
/// same absolute directory the spawn forwards as `NABLA_DATA_DIR`, so the file
/// and the reader can only agree.
pub fn permissions_projection_path(data_dir: &Path) -> PathBuf {
    data_dir.join("permissions.json")
}

/// Writes `body` to `path` atomically: a temp file in the same directory, then a
/// rename over the target, so a reader never sees a half-written file.
///
/// One helper because every JSON store Nabla owns writes this way (L4 of
/// `permission-modes.md` for the projection, and the settings file itself); a
/// second copy of it is exactly how one of them would end up non-atomic.
pub(crate) fn write_atomic(path: &Path, body: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, path)
}

/// S2/L4: writes `{"mode":"<mode>"}` for the extension to read on its next tool
/// call. Called at boot and on every change, which is what makes a mode switch
/// affect the very next call without restarting the session.
///
/// Atomic like the settings file: temp file, then rename, so a reader never sees
/// a half-written projection (and falls back to `write` for no reason).
pub fn write_permission_projection(data_dir: &Path, mode: PermissionMode) -> SettingsResult<()> {
    let path = permissions_projection_path(data_dir);
    let body = serde_json::json!({ "mode": mode.as_str() });
    let json = serde_json::to_string(&body).map_err(|e| SettingsError::Parse(e.to_string()))?;
    write_atomic(&path, &json).map_err(|e| SettingsError::Io(e.to_string()))
}

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
    let json = serde_json::to_string_pretty(settings).map_err(|e| SettingsError::Parse(e.to_string()))?;
    write_atomic(path, &json).map_err(|e| SettingsError::Io(e.to_string()))
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

    // ── permission modes (S1-S6 of `permission-modes.md`) ────────────────────

    // S6: write is the default, so a settings file written before this
    // preference existed (or a fresh install) keeps today's behaviour for files.
    #[test]
    fn permission_mode_defaults_to_write() {
        let s = Settings::default();
        assert_eq!(s.permission_mode, "write");
        assert_eq!(s.mode(), PermissionMode::Write);

        // camelCase on the wire, so `types.ts` mirrors this key.
        let json = serde_json::to_value(Settings::default()).expect("serialize");
        assert_eq!(json["permissionMode"], serde_json::json!("write"));
    }

    // The settings header rule + S6: an older file loads as write, and every
    // other key survives.
    #[test]
    fn settings_file_without_permission_mode_loads_as_write() {
        let cfg = TempCfg::new("pre_permission_mode");
        std::fs::write(
            &cfg.0,
            r#"{"theme":"light","piBinary":"pi","provider":"nan","model":"glm5.3"}"#,
        )
        .expect("write fixture");
        let s = load(&cfg.0).expect("an older settings file must load");
        assert_eq!(s.theme, "light");
        assert_eq!(s.permission_mode, "write");
        assert_eq!(s.mode(), PermissionMode::Write);
    }

    // S2: the mode is a preference like the others — saved and read back
    // unchanged, so the chip reflects what the file holds on mount.
    #[test]
    fn permission_mode_round_trips() {
        let cfg = TempCfg::new("permission_mode_roundtrip");
        for mode in [PermissionMode::Read, PermissionMode::Write, PermissionMode::Yolo] {
            let s = Settings {
                permission_mode: mode.as_str().to_string(),
                ..Settings::default()
            };
            save(&cfg.0, &s).expect("save");
            let loaded = load(&cfg.0).expect("load");
            assert_eq!(loaded.mode(), mode, "{mode:?} must survive the file");
            assert_eq!(loaded.permission_mode, mode.as_str());
        }
    }

    // L4/fail-safe: a value this build does not know reads as `write`, never as
    // `yolo` — a typo in the file must not uncage the agent.
    #[test]
    fn an_unknown_permission_mode_reads_as_write() {
        let cfg = TempCfg::new("unknown_permission_mode");
        std::fs::write(
            &cfg.0,
            r#"{"theme":"dark","piBinary":"pi","permissionMode":"lectura"}"#,
        )
        .expect("write fixture");
        let s = load(&cfg.0).expect("load");
        assert_eq!(s.mode(), PermissionMode::Write);
    }

    // L3: the four read-only tools are free in every mode — a read mode that
    // also blocked `read` would leave the agent unable to look at anything.
    #[test]
    fn decide_allows_the_free_tools_in_every_mode() {
        for mode in [PermissionMode::Read, PermissionMode::Write, PermissionMode::Yolo] {
            for tool in ["read", "grep", "find", "ls"] {
                assert_eq!(
                    decide(mode, tool),
                    PermissionDecision::Allow,
                    "{tool} in {mode:?}"
                );
            }
        }
    }

    // S3: read mode blocks every tool that touches the disk or runs a command,
    // and the reason the model reads names the mode and where to change it.
    #[test]
    fn decide_blocks_the_acting_tools_in_read_mode() {
        for tool in ["bash", "edit", "write", "powershell"] {
            match decide(PermissionMode::Read, tool) {
                PermissionDecision::Block(reason) => {
                    assert!(reason.contains("read"), "{tool}: {reason}");
                    assert!(reason.contains(tool), "{tool}: {reason}");
                    assert!(
                        reason.contains("chat header"),
                        "the reason must say where the mode is changed: {reason}"
                    );
                }
                other => panic!("{tool} in read mode must be blocked, got {other:?}"),
            }
        }
    }

    // S4: write mode allows file edits and asks before running a command.
    #[test]
    fn decide_asks_for_shell_tools_and_allows_file_edits_in_write_mode() {
        for tool in ["edit", "write"] {
            assert_eq!(decide(PermissionMode::Write, tool), PermissionDecision::Allow);
        }
        for tool in ["bash", "powershell"] {
            assert_eq!(decide(PermissionMode::Write, tool), PermissionDecision::Ask);
        }
    }

    // S5: yolo asks nothing and blocks nothing.
    #[test]
    fn decide_allows_everything_in_yolo_mode() {
        for tool in [
            "bash",
            "powershell",
            "edit",
            "write",
            "read",
            "grep",
            "find",
            "ls",
            "nabla-browser_navigate",
        ] {
            assert_eq!(
                decide(PermissionMode::Yolo, tool),
                PermissionDecision::Allow,
                "{tool}"
            );
        }
    }

    // L3: v1 gates three tool families, not every tool that exists: a tool this
    // build does not know (the browser bridge) is never blocked by this gate.
    #[test]
    fn decide_leaves_unknown_tools_alone_in_every_mode() {
        for mode in [PermissionMode::Read, PermissionMode::Write, PermissionMode::Yolo] {
            for tool in ["nabla-browser_navigate", "some_future_tool"] {
                assert_eq!(decide(mode, tool), PermissionDecision::Allow, "{tool} in {mode:?}");
            }
        }
    }

    // The mode reaches a live child by file (L4), so the projection is written
    // where the extension reads it and is rewritten on every change.
    #[test]
    fn write_permission_projection_writes_the_mode_the_extension_reads() {
        let mut dir = std::env::temp_dir();
        dir.push(format!(
            "nabla_permissions_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        // Deliberately missing: the writer creates it, like the data dir.
        assert!(!dir.exists());

        write_permission_projection(&dir, PermissionMode::Read).expect("write projection");
        let path = permissions_projection_path(&dir);
        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some("permissions.json")
        );
        let raw = std::fs::read_to_string(&path).expect("read projection");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&raw).expect("json"),
            serde_json::json!({ "mode": "read" })
        );

        // A change overwrites the same file: the extension reads it per call.
        write_permission_projection(&dir, PermissionMode::Yolo).expect("rewrite projection");
        let raw = std::fs::read_to_string(&path).expect("read projection");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&raw).expect("json"),
            serde_json::json!({ "mode": "yolo" })
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── thinking level (S2 of `composer-inputs.md`) ─────────────────────────

    // S2: the effort of the next session is a preference like the rest. The
    // standing default is `off`, the maintainer's decision: no per-model
    // default exists in pi, so a static non-off level would impose reasoning
    // nobody asked for on every fresh session. `off` is the cheap start, and
    // the human raises it deliberately when a session deserves it.
    #[test]
    fn thinking_level_defaults_to_off_and_serializes_camel_case() {
        let s = Settings::default();
        assert_eq!(s.thinking_level, "off");
        assert_eq!(s.thinking(), Some("off"));

        // camelCase on the wire, so `types.ts` mirrors this key.
        let json = serde_json::to_value(Settings::default()).expect("serialize");
        assert_eq!(json["thinkingLevel"], serde_json::json!("off"));
    }

    // S2 + the settings header rule: a file written before the preference
    // existed loads with the standing default and every other key intact.
    #[test]
    fn settings_file_without_thinking_level_uses_off() {
        let cfg = TempCfg::new("pre_thinking_level");
        std::fs::write(
            &cfg.0,
            r#"{"theme":"light","piBinary":"pi","provider":"nan","model":"glm5.3"}"#,
        )
        .expect("write fixture");
        let s = load(&cfg.0).expect("an older settings file must load");
        assert_eq!(s.theme, "light");
        assert_eq!(s.thinking_level, "off");
    }

    // S2: the level is written verbatim and read back unchanged, so a relaunch
    // starts the next session with the effort the human picked.
    #[test]
    fn thinking_level_round_trips() {
        let cfg = TempCfg::new("thinking_roundtrip");
        for level in THINKING_LEVELS {
            let s = Settings {
                thinking_level: (*level).to_string(),
                ..Settings::default()
            };
            save(&cfg.0, &s).expect("save");
            let loaded = load(&cfg.0).expect("load");
            assert_eq!(loaded.thinking_level, *level, "{level} must survive the file");
            assert_eq!(loaded.thinking(), Some(*level));
        }
    }

    // S2: the words are pi's own (`--thinking`, pi docs/cli.md:71). Anything else
    // is not a level, so a caller that must not guess can reject it instead of
    // writing a value pi would silently ignore.
    #[test]
    fn parse_thinking_level_accepts_only_the_documented_words() {
        for level in ["off", "minimal", "low", "medium", "high", "xhigh", "max"] {
            assert_eq!(parse_thinking_level(level), Some(level));
        }
        // Surrounding whitespace and case are normalized, not rejected.
        assert_eq!(parse_thinking_level("  HIGH "), Some("high"));
        for bad in ["", "  ", "maximo", "on", "none", "high!"] {
            assert_eq!(parse_thinking_level(bad), None, "{bad:?} is not a level");
        }
    }

    // S2/fail-safe: a hand edit that is not a level reads as "no level chosen",
    // so the spawn omits `--thinking` and pi keeps its own default instead of
    // being handed a word it cannot parse.
    #[test]
    fn an_unknown_stored_thinking_level_reads_as_none() {
        let cfg = TempCfg::new("unknown_thinking_level");
        std::fs::write(
            &cfg.0,
            r#"{"theme":"dark","piBinary":"pi","thinkingLevel":"mucho"}"#,
        )
        .expect("write fixture");
        let s = load(&cfg.0).expect("load");
        assert_eq!(s.thinking_level, "mucho", "the file keeps what was written");
        assert_eq!(s.thinking(), None, "but it names no level this build knows");
    }

    // ── web search (S1 of `web-search.md`) ──────────────────────────────────

    // S1: a fresh install searches nothing. The tool is opt-in because the query
    // leaves the machine, and "any public instance" is the empty preference.
    #[test]
    fn web_search_defaults_to_off_and_any_instance() {
        let s = Settings::default();
        assert!(!s.web_search_enabled, "S1: web search ships off");
        assert_eq!(s.web_search_instance, "", "empty means any public instance");

        // camelCase on the wire, so `types.ts` mirrors these keys.
        let json = serde_json::to_value(Settings::default()).expect("serialize");
        assert_eq!(json["webSearchEnabled"], serde_json::json!(false));
        assert_eq!(json["webSearchInstance"], serde_json::json!(""));
    }

    // S1 + the settings header rule: a file written before the preference
    // existed loads with the tool off and every other key intact.
    #[test]
    fn settings_file_without_web_search_loads_it_off() {
        let cfg = TempCfg::new("pre_web_search");
        std::fs::write(
            &cfg.0,
            r#"{"theme":"light","piBinary":"pi","provider":"nan","model":"glm5.3","language":"es"}"#,
        )
        .expect("write fixture");
        let s = load(&cfg.0).expect("an older settings file must load");
        assert_eq!(s.language, "es");
        assert!(!s.web_search_enabled, "an absent preference means off");
        assert_eq!(s.web_search_instance, "");
    }

    // S1: the toggle and the instance are preferences like the rest — written
    // and read back unchanged, so a relaunch keeps the instance the human chose.
    #[test]
    fn web_search_round_trips() {
        let cfg = TempCfg::new("web_search_roundtrip");
        let s = Settings {
            web_search_enabled: true,
            web_search_instance: "https://searx.example".to_string(),
            ..Settings::default()
        };
        save(&cfg.0, &s).expect("save");
        let loaded = load(&cfg.0).expect("load");
        assert!(loaded.web_search_enabled, "the tool stays on");
        assert_eq!(loaded.web_search_instance, "https://searx.example");

        // Turning it off is a real choice too, and the instance is not erased by
        // it: the human can flip the switch back without retyping the url.
        save(
            &cfg.0,
            &Settings {
                web_search_enabled: false,
                ..loaded
            },
        )
        .expect("save");
        let off = load(&cfg.0).expect("load");
        assert!(!off.web_search_enabled);
        assert_eq!(off.web_search_instance, "https://searx.example");
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
