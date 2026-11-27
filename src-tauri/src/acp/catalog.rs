//! ACP (Agent Client Protocol) agent catalog: discovery, binary resolution,
//! authentication detection, grouped Antigravity models + effort mapping,
//! auto-detected known ACP CLIs, and user-defined custom ACP CLIs persisted in
//! `<data_dir>/acp-custom.json`.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

use crate::providers::{MODEL_SOURCE_CATALOG, ModelInfo, ProviderModels};

/// Prefix used in `provider` ids for ACP-backed CLIs (e.g. `"acp:antigravity"`).
pub const ACP_RUNTIME_PREFIX: &str = "acp:";

const EFFORT_ORDER: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];
const CUSTOM_FILE_NAME: &str = "acp-custom.json";
const CONFIG_FILE_NAME: &str = "acp-config.json";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct AcpEnabledConfig {
    enabled_ids: Vec<String>,
}

#[derive(Debug, Clone)]
struct GroupedAgyModel {
    base_id: String,
    label: String,
    efforts: Vec<(String, String)>,
}

static AGY_MODELS_CACHE: OnceLock<Mutex<Option<Vec<GroupedAgyModel>>>> = OnceLock::new();

/// Extracts the ACP agent id (`"acp:antigravity"` -> `Some("antigravity")`).
pub fn acp_agent_id(runtime: &str) -> Option<&str> {
    runtime
        .strip_prefix(ACP_RUNTIME_PREFIX)
        .filter(|s| !s.is_empty())
}

/// Formats an ACP provider/runtime key (`"antigravity"` -> `"acp:antigravity"`).
pub fn runtime_key(agent_id: &str) -> String {
    format!("{ACP_RUNTIME_PREFIX}{agent_id}")
}

/// Status of one ACP CLI connection exposed to Settings -> Providers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpAgentStatus {
    pub id: String,
    pub runtime: String,
    pub name: String,
    pub description: String,
    pub command: String,
    pub args: Vec<String>,
    pub installed: bool,
    pub authenticated: bool,
    pub login_command: String,
    pub login_args: Vec<String>,
    pub hint: String,
    pub custom: bool,
    /// Opt-in switch controlled by the human in Settings -> Providers.
    /// Auto-detected CLIs start disabled (`false`) and never appear in the
    /// model picker until explicitly enabled.
    pub enabled: bool,
}

/// User-configured custom ACP CLI saved in `<data_dir>/acp-custom.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomAcpAgent {
    pub id: String,
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub login_command: Option<String>,
    #[serde(default)]
    pub login_args: Vec<String>,
}

/// Payload accepted by `acp_save_custom_agent`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomAcpAgentDraft {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub login_command: Option<String>,
    #[serde(default)]
    pub login_args: Vec<String>,
}

/// Resolved launch specification for spawning an ACP child process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpLaunchSpec {
    pub id: String,
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
}

/// Common user/system executable directories on macOS and Linux that desktop
/// launchers often omit from `PATH` (Snap, Homebrew, nvm/fnm/volta/bun/pnpm,
/// cargo, `~/.local/bin`).
pub fn extra_path_dirs() -> Vec<PathBuf> {
    let mut extra = Vec::new();
    if let Some(home) = home_dir() {
        extra.extend([
            home.join(".local/bin"),
            home.join(".npm-global/bin"),
            home.join(".local/share/pnpm"),
            home.join(".volta/bin"),
            home.join(".bun/bin"),
            home.join(".cargo/bin"),
            home.join(".asdf/shims"),
            home.join(".mise/shims"),
            home.join(".fnm/aliases/default/bin"),
        ]);
        // Latest nvm-managed Node bin directory, if present.
        let nvm_versions = home.join(".nvm/versions/node");
        if let Ok(entries) = fs::read_dir(&nvm_versions) {
            let mut versions: Vec<PathBuf> = entries
                .filter_map(Result::ok)
                .map(|e| e.path().join("bin"))
                .filter(|p| p.is_dir())
                .collect();
            versions.sort();
            versions.reverse();
            extra.extend(versions);
        }
    }
    extra.extend([
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
        PathBuf::from("/snap/bin"),
        PathBuf::from("/var/lib/snapd/snap/bin"),
        PathBuf::from("/home/linuxbrew/.linuxbrew/bin"),
    ]);
    extra
}

/// Searches `PATH` (plus common macOS/Linux user bin dirs) for an executable.
pub fn find_in_path(binary: &str) -> Option<PathBuf> {
    let as_path = Path::new(binary);
    if as_path.is_absolute() && as_path.is_file() {
        return Some(as_path.to_path_buf());
    }
    let mut dirs: Vec<PathBuf> = env::var_os("PATH")
        .map(|p| env::split_paths(&p).collect())
        .unwrap_or_default();

    for extra in extra_path_dirs() {
        if !dirs.iter().any(|d| d == &extra) {
            dirs.push(extra);
        }
    }

    #[cfg(windows)]
    let candidates = [
        format!("{binary}.exe"),
        format!("{binary}.cmd"),
        format!("{binary}.bat"),
        binary.to_string(),
    ];
    #[cfg(not(windows))]
    let candidates = [binary.to_string()];

    for dir in dirs {
        for name in &candidates {
            let full = dir.join(name);
            if full.is_file() {
                return Some(full);
            }
        }
    }
    None
}

fn home_dir() -> Option<PathBuf> {
    env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

fn file_non_empty(path: &Path) -> bool {
    fs::metadata(path).map(|m| m.len() > 2).unwrap_or(false)
}

fn dir_has_files(path: &Path) -> bool {
    fs::read_dir(path)
        .map(|mut it| it.next().is_some())
        .unwrap_or(false)
}

fn env_non_empty(name: &str) -> bool {
    env::var(name).map(|v| !v.trim().is_empty()).unwrap_or(false)
}

/// Checks whether Antigravity CLI credentials exist under a given home directory,
/// covering both standard installs (`~/.gemini/antigravity-cli`) and Linux Snap
/// installs (`~/snap/antigravity-cli/{common,current}/.gemini/antigravity-cli`).
pub fn has_antigravity_auth_in(home: &Path) -> bool {
    let agy_dir = home.join(".gemini").join("antigravity-cli");
    let gemini_dir = home.join(".gemini");
    if file_non_empty(&agy_dir.join("antigravity-oauth-token"))
        || dir_has_files(&agy_dir)
        || file_non_empty(&gemini_dir.join("oauth_creds.json"))
        || file_non_empty(&gemini_dir.join("google_accounts.json"))
    {
        return true;
    }

    for snap_slot in ["common", "current"] {
        let snap_gemini = home
            .join("snap")
            .join("antigravity-cli")
            .join(snap_slot)
            .join(".gemini");
        let snap_agy = snap_gemini.join("antigravity-cli");
        if file_non_empty(&snap_agy.join("antigravity-oauth-token"))
            || file_non_empty(&snap_gemini.join("oauth_creds.json"))
            || file_non_empty(&snap_gemini.join("google_accounts.json"))
        {
            return true;
        }
    }
    false
}

fn detect_antigravity_auth() -> bool {
    if env_non_empty("GEMINI_API_KEY") {
        return true;
    }
    if let Ok(app_dir) = env::var("ANTIGRAVITY_APP_DATA_DIR") {
        let dir = PathBuf::from(app_dir.trim());
        if !dir.as_os_str().is_empty()
            && (file_non_empty(&dir.join("antigravity-oauth-token")) || dir_has_files(&dir))
        {
            return true;
        }
    }
    if let Some(home) = home_dir() {
        return has_antigravity_auth_in(&home);
    }
    false
}

fn detect_claude_auth() -> bool {
    if env_non_empty("ANTHROPIC_API_KEY") {
        return true;
    }
    if let Some(home) = home_dir() {
        let claude_dir = home.join(".claude");
        if file_non_empty(&claude_dir.join(".credentials.json"))
            || file_non_empty(&home.join(".claude.json"))
            || dir_has_files(&claude_dir)
        {
            return true;
        }
    }
    false
}

fn detect_gemini_auth() -> bool {
    if env_non_empty("GEMINI_API_KEY") || env_non_empty("GOOGLE_API_KEY") {
        return true;
    }
    if let Some(home) = home_dir() {
        let gemini_dir = home.join(".gemini");
        if file_non_empty(&gemini_dir.join("oauth_creds.json"))
            || file_non_empty(&gemini_dir.join("google_accounts.json"))
            || file_non_empty(&gemini_dir.join("settings.json"))
        {
            return true;
        }
    }
    false
}

fn detect_codex_auth() -> bool {
    if env_non_empty("OPENAI_API_KEY") {
        return true;
    }
    if let Some(home) = home_dir() {
        return file_non_empty(&home.join(".codex").join("auth.json"));
    }
    false
}

fn slugify_id(raw: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for ch in raw.trim().to_ascii_lowercase().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            prev_dash = false;
        } else if !prev_dash && !out.is_empty() {
            out.push('-');
            prev_dash = true;
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "custom-cli".to_string()
    } else {
        trimmed
    }
}

fn config_store_path(data_dir: &Path) -> PathBuf {
    data_dir.join(CONFIG_FILE_NAME)
}

fn load_enabled_ids(data_dir: &Path) -> Vec<String> {
    let Ok(raw) = fs::read_to_string(config_store_path(data_dir)) else {
        return Vec::new();
    };
    serde_json::from_str::<AcpEnabledConfig>(&raw)
        .map(|c| c.enabled_ids)
        .unwrap_or_default()
}

fn write_enabled_ids(data_dir: &Path, enabled_ids: &[String]) -> Result<(), String> {
    fs::create_dir_all(data_dir).map_err(|e| format!("failed to create data dir: {e}"))?;
    let path = config_store_path(data_dir);
    let tmp = data_dir.join(format!("{CONFIG_FILE_NAME}.tmp"));
    let body = serde_json::to_string_pretty(&AcpEnabledConfig {
        enabled_ids: enabled_ids.to_vec(),
    })
    .map_err(|e| e.to_string())?;
    fs::write(&tmp, body).map_err(|e| format!("failed to write ACP config: {e}"))?;
    fs::rename(&tmp, &path).map_err(|e| format!("failed to persist ACP config: {e}"))?;
    Ok(())
}

/// Explicitly enables or disables an ACP CLI connection for the model picker.
pub fn set_agent_enabled(
    data_dir: &Path,
    agent_id: &str,
    enabled: bool,
) -> Result<Vec<AcpAgentStatus>, String> {
    let mut ids = load_enabled_ids(data_dir);
    if enabled {
        if !ids.iter().any(|id| id == agent_id) {
            ids.push(agent_id.to_string());
        }
    } else {
        ids.retain(|id| id != agent_id);
    }
    write_enabled_ids(data_dir, &ids)?;
    Ok(list_agents(data_dir))
}

fn custom_store_path(data_dir: &Path) -> PathBuf {
    data_dir.join(CUSTOM_FILE_NAME)
}

pub fn load_custom_agents(data_dir: &Path) -> Vec<CustomAcpAgent> {
    let path = custom_store_path(data_dir);
    let Ok(raw) = fs::read_to_string(path) else {
        return Vec::new();
    };
    serde_json::from_str(&raw).unwrap_or_default()
}

fn write_custom_agents(data_dir: &Path, items: &[CustomAcpAgent]) -> Result<(), String> {
    fs::create_dir_all(data_dir).map_err(|e| format!("failed to create data dir: {e}"))?;
    let path = custom_store_path(data_dir);
    let tmp = data_dir.join(format!("{CUSTOM_FILE_NAME}.tmp"));
    let body = serde_json::to_string_pretty(items).map_err(|e| e.to_string())?;
    fs::write(&tmp, body).map_err(|e| format!("failed to write custom ACP agents: {e}"))?;
    fs::rename(&tmp, &path).map_err(|e| format!("failed to persist custom ACP agents: {e}"))?;
    Ok(())
}

pub fn save_custom_agent(
    data_dir: &Path,
    draft: CustomAcpAgentDraft,
) -> Result<Vec<AcpAgentStatus>, String> {
    let name = draft.name.trim().to_string();
    let command = draft.command.trim().to_string();
    if name.is_empty() {
        return Err("CLI name is required".to_string());
    }
    if command.is_empty() {
        return Err("CLI command is required".to_string());
    }
    let id = format!("custom-{}", slugify_id(&name));
    let mut list = load_custom_agents(data_dir);
    let entry = CustomAcpAgent {
        id: id.clone(),
        name,
        command,
        args: draft
            .args
            .into_iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
        login_command: draft
            .login_command
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
        login_args: draft
            .login_args
            .into_iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
    };
    if let Some(existing) = list.iter_mut().find(|a| a.id == id) {
        *existing = entry;
    } else {
        list.push(entry);
    }
    write_custom_agents(data_dir, &list)?;
    // A custom CLI explicitly added by the user starts enabled.
    set_agent_enabled(data_dir, &id, true)
}

pub fn delete_custom_agent(data_dir: &Path, agent_id: &str) -> Result<Vec<AcpAgentStatus>, String> {
    let mut list = load_custom_agents(data_dir);
    list.retain(|a| a.id != agent_id);
    write_custom_agents(data_dir, &list)?;
    set_agent_enabled(data_dir, agent_id, false)
}

fn split_effort_suffix(id: &str, label: &str) -> (String, String, Option<String>) {
    for level in EFFORT_ORDER {
        let id_suffix = format!("-{level}");
        if let Some(base_id) = id.strip_suffix(&id_suffix) {
            let trimmed_label = strip_label_effort_suffix(label);
            return (base_id.to_string(), trimmed_label, Some(level.to_string()));
        }
    }
    (id.to_string(), label.to_string(), None)
}

fn strip_label_effort_suffix(label: &str) -> String {
    let trimmed = label.trim();
    for suffix in ["(Low)", "(Medium)", "(High)", "(XHigh)", "(Max)"] {
        if let Some(base) = trimmed.strip_suffix(suffix) {
            return base.trim_end().to_string();
        }
    }
    trimmed.to_string()
}

fn group_antigravity_models(raw_pairs: Vec<(String, String)>) -> Vec<GroupedAgyModel> {
    let mut grouped: Vec<GroupedAgyModel> = Vec::new();
    for (wire_id, raw_label) in raw_pairs {
        let (base_id, clean_label, effort) = split_effort_suffix(&wire_id, &raw_label);
        if let Some(existing) = grouped.iter_mut().find(|g| g.base_id == base_id) {
            if let Some(level) = effort
                && !existing.efforts.iter().any(|(l, _)| l == &level)
            {
                existing.efforts.push((level, wire_id));
            }
        } else {
            let mut efforts = Vec::new();
            if let Some(level) = effort {
                efforts.push((level, wire_id));
            }
            grouped.push(GroupedAgyModel {
                base_id,
                label: clean_label,
                efforts,
            });
        }
    }
    for entry in &mut grouped {
        entry.efforts.sort_by_key(|(lvl, _)| {
            EFFORT_ORDER
                .iter()
                .position(|k| k == lvl)
                .unwrap_or(EFFORT_ORDER.len())
        });
    }
    grouped
}

fn discover_antigravity_models(agy_cmd: &str) -> Vec<GroupedAgyModel> {
    let cache = AGY_MODELS_CACHE.get_or_init(|| Mutex::new(None));
    if let Ok(guard) = cache.lock()
        && let Some(ref cached) = *guard
    {
        return cached.clone();
    }

    let parsed = Command::new(agy_cmd)
        .arg("models")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| {
            let stdout = String::from_utf8_lossy(&o.stdout);
            stdout
                .lines()
                .filter_map(|line| {
                    let (id, label) = line.split_once('\t')?;
                    let id = id.trim();
                    let label = label.trim();
                    if id.is_empty() || label.is_empty() {
                        return None;
                    }
                    Some((id.to_string(), label.to_string()))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    if parsed.is_empty() {
        // Do not cache fallback failures so signing in later immediately discovers real models.
        vec![GroupedAgyModel {
            base_id: "default".to_string(),
            label: "Antigravity (Default)".to_string(),
            efforts: Vec::new(),
        }]
    } else {
        let grouped = group_antigravity_models(parsed);
        if let Ok(mut guard) = cache.lock() {
            *guard = Some(grouped.clone());
        }
        grouped
    }
}

/// Returns the effort levels supported by an ACP agent's selected model.
pub fn thinking_levels_for(agent_id: &str, base_model_id: &str) -> Vec<String> {
    if agent_id != "antigravity" {
        return Vec::new();
    }
    let agy_cmd = find_in_path("agy")
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| "agy".to_string());
    let models = discover_antigravity_models(&agy_cmd);
    let clean_base = split_effort_suffix(base_model_id, base_model_id).0;
    models
        .into_iter()
        .find(|m| m.base_id == clean_base)
        .map(|m| m.efforts.into_iter().map(|(lvl, _)| lvl).collect())
        .unwrap_or_default()
}

/// Resolves a grouped UI model id + Nabla effort level into the concrete wire
/// model id expected by the ACP agent.
pub fn resolve_wire_model(agent_id: &str, base_model_id: &str, effort: Option<&str>) -> String {
    if agent_id != "antigravity" || base_model_id.trim().is_empty() || base_model_id == "default" {
        return base_model_id.to_string();
    }
    let agy_cmd = find_in_path("agy")
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| "agy".to_string());
    let models = discover_antigravity_models(&agy_cmd);
    let clean_base = split_effort_suffix(base_model_id, base_model_id).0;
    let Some(entry) = models.into_iter().find(|m| m.base_id == clean_base) else {
        return base_model_id.to_string();
    };
    if entry.efforts.is_empty() {
        return entry.base_id;
    }
    if let Some(wanted) = effort.map(|s| s.trim().to_ascii_lowercase())
        && let Some((_, wire)) = entry.efforts.iter().find(|(lvl, _)| lvl == &wanted)
    {
        return wire.clone();
    }
    entry
        .efforts
        .iter()
        .find(|(lvl, _)| lvl == "medium")
        .or_else(|| entry.efforts.first())
        .map(|(_, wire)| wire.clone())
        .unwrap_or(entry.base_id)
}

fn claude_code_models() -> Vec<(String, String)> {
    vec![
        ("default".to_string(), "Claude Code (Default)".to_string()),
        ("sonnet".to_string(), "Claude Sonnet".to_string()),
        ("opus".to_string(), "Claude Opus".to_string()),
        ("haiku".to_string(), "Claude Haiku".to_string()),
    ]
}

fn gemini_cli_models() -> Vec<(String, String)> {
    vec![
        ("default".to_string(), "Gemini CLI (Default)".to_string()),
        ("gemini-2.5-pro".to_string(), "Gemini 2.5 Pro".to_string()),
        ("gemini-2.5-flash".to_string(), "Gemini 2.5 Flash".to_string()),
    ]
}

/// Lists built-in primary CLIs, auto-detected known ACP CLIs installed on the
/// user's machine, and user-defined custom ACP CLIs.
pub fn list_agents(data_dir: &Path) -> Vec<AcpAgentStatus> {
    let agy_bin = find_in_path("agy");
    let agy_cmd = agy_bin
        .as_ref()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| "agy".to_string());

    let claude_bin = find_in_path("claude");
    let claude_acp_bin = find_in_path("claude-code-acp");
    let npx_bin = find_in_path("npx");

    let (claude_cmd, claude_args, claude_installed) = if let Some(ref bin) = claude_acp_bin {
        (bin.to_string_lossy().to_string(), Vec::new(), true)
    } else if let Some(ref npx) = npx_bin {
        (
            npx.to_string_lossy().to_string(),
            vec![
                "-y".to_string(),
                "@zed-industries/claude-code-acp".to_string(),
            ],
            claude_bin.is_some(),
        )
    } else {
        (
            "npx".to_string(),
            vec![
                "-y".to_string(),
                "@zed-industries/claude-code-acp".to_string(),
            ],
            false,
        )
    };
    let claude_login_cmd = claude_bin
        .as_ref()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| "claude".to_string());

    let gemini_bin = find_in_path("gemini");
    let gemini_cmd = gemini_bin
        .as_ref()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| "gemini".to_string());

    let enabled_ids = load_enabled_ids(data_dir);
    let is_enabled = |id: &str| enabled_ids.iter().any(|e| e == id);

    let mut agents = vec![
        AcpAgentStatus {
            id: "antigravity".to_string(),
            runtime: runtime_key("antigravity"),
            name: "Antigravity".to_string(),
            description: "Google Antigravity CLI (`agy acp`)".to_string(),
            command: agy_cmd.clone(),
            args: vec!["acp".to_string()],
            installed: agy_bin.is_some(),
            authenticated: detect_antigravity_auth(),
            login_command: agy_cmd,
            login_args: Vec::new(),
            hint: "Install Antigravity CLI (`agy`) and sign in with your Google account."
                .to_string(),
            custom: false,
            enabled: is_enabled("antigravity"),
        },
        AcpAgentStatus {
            id: "claude-code".to_string(),
            runtime: runtime_key("claude-code"),
            name: "Claude Code".to_string(),
            description: "Anthropic Claude Code CLI (`@zed-industries/claude-code-acp`)"
                .to_string(),
            command: claude_cmd,
            args: claude_args,
            installed: claude_installed,
            authenticated: detect_claude_auth(),
            login_command: claude_login_cmd,
            login_args: vec!["auth".to_string(), "login".to_string()],
            hint: "Install Claude Code (`npm i -g @anthropic-ai/claude-code`) and sign in."
                .to_string(),
            custom: false,
            enabled: is_enabled("claude-code"),
        },
        AcpAgentStatus {
            id: "gemini-cli".to_string(),
            runtime: runtime_key("gemini-cli"),
            name: "Gemini CLI".to_string(),
            description: "Google Gemini CLI (`gemini --acp`)".to_string(),
            command: gemini_cmd.clone(),
            args: vec!["--acp".to_string()],
            installed: gemini_bin.is_some(),
            authenticated: detect_gemini_auth(),
            login_command: gemini_cmd,
            login_args: Vec::new(),
            hint: "Install Gemini CLI (`npm i -g @google/gemini-cli`) and sign in with Google."
                .to_string(),
            custom: false,
            enabled: is_enabled("gemini-cli"),
        },
    ];

    // Auto-detected community/ecosystem ACP CLIs: shown only when actually installed
    // so users without them see zero extra clutter.
    if let Some(codex_bin) = find_in_path("codex") {
        let codex_acp = find_in_path("codex-acp");
        let (cmd, args) = if let Some(bin) = codex_acp {
            (bin.to_string_lossy().to_string(), Vec::new())
        } else if let Some(ref npx) = npx_bin {
            (
                npx.to_string_lossy().to_string(),
                vec!["-y".to_string(), "@zed-industries/codex-acp".to_string()],
            )
        } else {
            (
                "npx".to_string(),
                vec!["-y".to_string(), "@zed-industries/codex-acp".to_string()],
            )
        };
        agents.push(AcpAgentStatus {
            id: "codex".to_string(),
            runtime: runtime_key("codex"),
            name: "Codex CLI".to_string(),
            description: "OpenAI Codex CLI via ACP (`@zed-industries/codex-acp`)".to_string(),
            command: cmd,
            args,
            installed: true,
            authenticated: detect_codex_auth(),
            login_command: codex_bin.to_string_lossy().to_string(),
            login_args: vec!["login".to_string()],
            hint: "Sign in with `codex login`.".to_string(),
            custom: false,
            enabled: is_enabled("codex"),
        });
    }

    struct ExtraPreset {
        id: &'static str,
        name: &'static str,
        desc: &'static str,
        bins: &'static [&'static str],
        acp_args: &'static [&'static str],
    }

    let extra_presets: &[ExtraPreset] = &[
        ExtraPreset {
            id: "cursor-agent",
            name: "Cursor Agent",
            desc: "Cursor Agent CLI (`cursor-agent acp`)",
            bins: &["cursor-agent"],
            acp_args: &["acp"],
        },
        ExtraPreset {
            id: "opencode",
            name: "OpenCode",
            desc: "OpenCode CLI (`opencode acp`)",
            bins: &["opencode"],
            acp_args: &["acp"],
        },
        ExtraPreset {
            id: "qwen-code",
            name: "Qwen Code",
            desc: "Qwen Code CLI (`qwen --acp`)",
            bins: &["qwen", "qwen-code"],
            acp_args: &["--acp"],
        },
        ExtraPreset {
            id: "kimi-code",
            name: "Kimi Code",
            desc: "Kimi Code CLI (`kimi acp`)",
            bins: &["kimi", "kimi-cli"],
            acp_args: &["acp"],
        },
        ExtraPreset {
            id: "goose",
            name: "Goose",
            desc: "Goose CLI (`goose acp`)",
            bins: &["goose"],
            acp_args: &["acp"],
        },
    ];

    for preset in extra_presets {
        if let Some(found) = preset.bins.iter().find_map(|b| find_in_path(b)) {
            let cmd = found.to_string_lossy().to_string();
            agents.push(AcpAgentStatus {
                id: preset.id.to_string(),
                runtime: runtime_key(preset.id),
                name: preset.name.to_string(),
                description: preset.desc.to_string(),
                command: cmd.clone(),
                args: preset.acp_args.iter().map(|s| (*s).to_string()).collect(),
                installed: true,
                authenticated: true,
                login_command: cmd,
                login_args: Vec::new(),
                hint: format!(
                    "Run `{}` in a terminal to authenticate if needed.",
                    preset.bins[0]
                ),
                custom: false,
                enabled: is_enabled(preset.id),
            });
        }
    }

    // User-configured custom ACP CLIs from `<data_dir>/acp-custom.json`.
    for custom in load_custom_agents(data_dir) {
        let resolved = find_in_path(&custom.command);
        let installed = resolved.is_some();
        let cmd = resolved
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| custom.command.clone());
        let login_cmd = custom
            .login_command
            .as_deref()
            .and_then(|c| find_in_path(c).map(|p| p.to_string_lossy().to_string()))
            .or_else(|| custom.login_command.clone())
            .unwrap_or_else(|| cmd.clone());
        let arg_preview = if custom.args.is_empty() {
            custom.command.clone()
        } else {
            format!("{} {}", custom.command, custom.args.join(" "))
        };
        agents.push(AcpAgentStatus {
            id: custom.id.clone(),
            runtime: runtime_key(&custom.id),
            name: custom.name,
            description: format!("Custom ACP CLI (`{arg_preview}`)"),
            command: cmd,
            args: custom.args,
            installed,
            authenticated: installed,
            login_command: login_cmd,
            login_args: custom.login_args,
            hint: format!("Executable `{}` not found on PATH.", custom.command),
            custom: true,
            enabled: is_enabled(&custom.id),
        });
    }

    agents
}

fn to_model_info(id: String, name: String) -> ModelInfo {
    ModelInfo {
        id,
        name,
        context_window: None,
        max_tokens: None,
        input_cost: None,
        output_cost: None,
        modalities: vec!["text".to_string(), "image".to_string()],
        enabled: true,
        hidden: false,
        source: MODEL_SOURCE_CATALOG.to_string(),
    }
}

/// Projects only installed AND explicitly enabled ACP CLIs into
/// `ProviderModels` rows for Nabla's normal Provider -> Model picker.
pub fn provider_models(data_dir: &Path) -> Vec<ProviderModels> {
    list_agents(data_dir)
        .into_iter()
        .filter(|a| a.installed && a.enabled)
        .map(|a| {
            let pairs: Vec<(String, String)> = match a.id.as_str() {
                "antigravity" => discover_antigravity_models(&a.command)
                    .into_iter()
                    .map(|g| (g.base_id, g.label))
                    .collect(),
                "claude-code" => claude_code_models(),
                "gemini-cli" => gemini_cli_models(),
                _ => vec![("default".to_string(), format!("{} (Default)", a.name))],
            };
            ProviderModels {
                provider: a.runtime,
                name: a.name,
                models: pairs
                    .into_iter()
                    .map(|(id, name)| to_model_info(id, name))
                    .collect(),
            }
        })
        .collect()
}

/// Resolves any ACP agent id (built-in, auto-detected, or custom).
pub fn resolve_launch(data_dir: &Path, agent_id: &str) -> Result<AcpLaunchSpec, String> {
    let agents = list_agents(data_dir);
    let agent = agents
        .into_iter()
        .find(|a| a.id == agent_id)
        .ok_or_else(|| format!("Unknown ACP agent: {agent_id}"))?;
    if !agent.installed {
        return Err(format!("{} is not installed. {}", agent.name, agent.hint));
    }
    Ok(AcpLaunchSpec {
        id: agent.id,
        name: agent.name,
        command: agent.command,
        args: agent.args,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_key_round_trips() {
        let key = runtime_key("antigravity");
        assert_eq!(key, "acp:antigravity");
        assert_eq!(acp_agent_id(&key), Some("antigravity"));
        assert_eq!(acp_agent_id("openai"), None);
        assert_eq!(acp_agent_id("acp:"), None);
    }

    #[test]
    fn catalog_contains_primary_agents_and_persists_custom_cli() {
        let dir = std::env::temp_dir().join("nabla-acp-catalog-test");
        let _ = fs::remove_dir_all(&dir);
        let agents = list_agents(&dir);
        assert!(agents.iter().any(|a| a.id == "antigravity"));
        assert!(agents.iter().any(|a| a.id == "claude-code"));
        assert!(agents.iter().any(|a| a.id == "gemini-cli"));
        // Auto-detected CLIs are never enabled automatically.
        assert!(agents.iter().all(|a| !a.enabled));
        assert!(provider_models(&dir).is_empty());

        let after_enable = set_agent_enabled(&dir, "antigravity", true).unwrap();
        assert!(after_enable.iter().find(|a| a.id == "antigravity").unwrap().enabled);

        let after_save = save_custom_agent(
            &dir,
            CustomAcpAgentDraft {
                name: "Z.ai Code".into(),
                command: "zai".into(),
                args: vec!["acp".into()],
                login_command: None,
                login_args: Vec::new(),
            },
        )
        .unwrap();
        let added = after_save.iter().find(|a| a.id == "custom-z-ai-code").unwrap();
        assert!(added.custom);
        assert_eq!(added.args, vec!["acp"]);

        let after_del = delete_custom_agent(&dir, "custom-z-ai-code").unwrap();
        assert!(!after_del.iter().any(|a| a.id == "custom-z-ai-code"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn groups_antigravity_effort_variants_into_single_model() {
        let grouped = group_antigravity_models(vec![
            ("gemini-4-neon-high".into(), "Gemini 4 Neon (High)".into()),
            ("gemini-4-neon-low".into(), "Gemini 4 Neon (Low)".into()),
            ("gemini-4-neon-xhigh".into(), "Gemini 4 Neon (XHigh)".into()),
            ("gemini-4-neon-medium".into(), "Gemini 4 Neon (Medium)".into()),
            ("gemini-pro-orchestrator-neon".into(), "Orchestrator (Neon)".into()),
        ]);
        assert_eq!(grouped.len(), 2);
        assert_eq!(grouped[0].base_id, "gemini-4-neon");
        assert_eq!(grouped[0].label, "Gemini 4 Neon");
        let levels: Vec<&str> = grouped[0].efforts.iter().map(|(l, _)| l.as_str()).collect();
        assert_eq!(levels, vec!["low", "medium", "high", "xhigh"]);
        assert_eq!(grouped[1].base_id, "gemini-pro-orchestrator-neon");
        assert!(grouped[1].efforts.is_empty());
    }

    #[test]
    fn detects_antigravity_auth_in_linux_snap_common_dir() {
        let home = std::env::temp_dir().join("nabla-acp-snap-auth-test");
        let _ = fs::remove_dir_all(&home);
        assert!(!has_antigravity_auth_in(&home));

        let snap_agy_dir = home
            .join("snap")
            .join("antigravity-cli")
            .join("common")
            .join(".gemini")
            .join("antigravity-cli");
        fs::create_dir_all(&snap_agy_dir).unwrap();
        // Empty dir without oauth token is not considered signed in.
        assert!(!has_antigravity_auth_in(&home));

        fs::write(snap_agy_dir.join("antigravity-oauth-token"), "ya29.test-token").unwrap();
        assert!(has_antigravity_auth_in(&home));

        let _ = fs::remove_dir_all(&home);
    }
}
