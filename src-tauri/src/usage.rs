//! CodexBar-style provider quota & local token/cost usage aggregator.
//!
//! Combines:
//! 1. Live subscription quota windows (5h / 7d) for authenticated accounts:
//!    - OpenAI/ChatGPT via Nabla's `auth.json` (`openai-codex`) or `~/.codex/auth.json`
//!    - Claude Code via `~/.claude/.credentials.json`, Nabla's `auth.json`, or macOS Keychain
//!    - Antigravity via local language server Connect-RPC probe (`RetrieveUserQuotaSummary` /
//!      `GetUserStatus`), falling back to local Nabla usage tracking
//! 2. Declarative custom provider usage endpoints & native presets (e.g. NaN Builders,
//!    OpenRouter, DeepSeek) stored in `<data_dir>/provider-usage.json`. All HTTP calls
//!    and credentials stay strictly in Rust — secrets never cross IPC to the webview.
//! 3. Exact local token & cost usage aggregated from Nabla's session JSONL files
//!    across all providers and models (today, 7d, 30d).

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

const QUOTA_TIMEOUT: Duration = Duration::from_secs(8);
const AGY_CLI_TIMEOUT: Duration = Duration::from_secs(25);
const BILLING_TIMEOUT: Duration = Duration::from_secs(2);
const USAGE_CONFIG_FILE: &str = "provider-usage.json";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaWindow {
    /// Short label like `"5h"`, `"7d"`, `"Créditos"`, or model family name.
    pub label: String,
    /// Percentage of the quota window consumed (`0.0..=100.0`).
    pub used_percent: f64,
    /// Optional ISO-8601 reset timestamp.
    pub reset_at: Option<String>,
    /// Optional seconds remaining until reset.
    pub reset_in_secs: Option<i64>,
    /// Optional human-readable detail (e.g. `"$4.20 / $10.00"` or `"120 / 500 cr"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderQuotaSnapshot {
    pub provider_id: String,
    pub name: String,
    pub plan: Option<String>,
    pub windows: Vec<QuotaWindow>,
    /// `"ok"` | `"local_only"` | `"unavailable"` | `"error"`
    pub status: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalUsageRow {
    pub provider: String,
    pub model: String,
    pub today_tokens: u64,
    pub week_tokens: u64,
    pub month_tokens: u64,
    pub today_cost: f64,
    pub week_cost: f64,
    pub month_cost: f64,
    pub turns: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageSnapshot {
    pub quotas: Vec<ProviderQuotaSnapshot>,
    pub local: Vec<LocalUsageRow>,
    pub total_today_tokens: u64,
    pub total_week_tokens: u64,
    pub total_today_cost: f64,
    pub total_week_cost: f64,
    pub fetched_at: u64,
}

// ---------------------------------------------------------------------------
// Declarative custom provider usage configuration (Option A)
// ---------------------------------------------------------------------------

fn default_true() -> bool {
    true
}
fn default_preset() -> String {
    "declarative".to_string()
}
fn default_method() -> String {
    "GET".to_string()
}
fn default_auth_mode() -> String {
    "providerApiKey".to_string()
}
fn default_unit() -> String {
    "percent".to_string()
}

/// On-disk config for one provider's usage endpoint (`<data_dir>/provider-usage.json`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderUsageConfig {
    pub provider_id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// `"nan"` | `"declarative"`
    #[serde(default = "default_preset")]
    pub preset: String,
    #[serde(default)]
    pub url: String,
    /// `"GET"` | `"POST"`
    #[serde(default = "default_method")]
    pub method: String,
    /// `"providerApiKey"` | `"bearer"` | `"cookie"` | `"none"`
    #[serde(default = "default_auth_mode")]
    pub auth_mode: String,
    /// Optional secret (Cookie header or custom Bearer token). Never returned over IPC.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub referer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub used_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remaining_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reset_at_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_path: Option<String>,
    /// `"percent"` | `"usd"` | `"credits"` | `"tokens"`
    #[serde(default = "default_unit")]
    pub unit: String,
}

/// Safe projection sent to the webview — secrets never leave Rust.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderUsageConfigSummary {
    pub provider_id: String,
    pub name: String,
    pub enabled: bool,
    pub preset: String,
    pub url: String,
    pub method: String,
    pub auth_mode: String,
    pub has_secret: bool,
    pub origin: Option<String>,
    pub referer: Option<String>,
    pub window_label: Option<String>,
    pub used_path: Option<String>,
    pub remaining_path: Option<String>,
    pub limit_path: Option<String>,
    pub reset_at_path: Option<String>,
    pub plan_path: Option<String>,
    pub unit: String,
}

/// Draft payload sent by the webview to save or test a provider's usage config.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderUsageConfigDraft {
    pub provider_id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_preset")]
    pub preset: String,
    #[serde(default)]
    pub url: String,
    #[serde(default = "default_method")]
    pub method: String,
    #[serde(default = "default_auth_mode")]
    pub auth_mode: String,
    /// When `None` or empty, preserves the previously stored secret unless `clear_secret` is true.
    #[serde(default)]
    pub secret: Option<String>,
    #[serde(default)]
    pub clear_secret: bool,
    #[serde(default)]
    pub origin: Option<String>,
    #[serde(default)]
    pub referer: Option<String>,
    #[serde(default)]
    pub window_label: Option<String>,
    #[serde(default)]
    pub used_path: Option<String>,
    #[serde(default)]
    pub remaining_path: Option<String>,
    #[serde(default)]
    pub limit_path: Option<String>,
    #[serde(default)]
    pub reset_at_path: Option<String>,
    #[serde(default)]
    pub plan_path: Option<String>,
    #[serde(default = "default_unit")]
    pub unit: String,
}

impl From<&ProviderUsageConfig> for ProviderUsageConfigSummary {
    fn from(c: &ProviderUsageConfig) -> Self {
        Self {
            provider_id: c.provider_id.clone(),
            name: c.name.clone(),
            enabled: c.enabled,
            preset: c.preset.clone(),
            url: c.url.clone(),
            method: c.method.clone(),
            auth_mode: c.auth_mode.clone(),
            has_secret: c
                .secret
                .as_deref()
                .map(|s| !s.trim().is_empty())
                .unwrap_or(false),
            origin: c.origin.clone(),
            referer: c.referer.clone(),
            window_label: c.window_label.clone(),
            used_path: c.used_path.clone(),
            remaining_path: c.remaining_path.clone(),
            limit_path: c.limit_path.clone(),
            reset_at_path: c.reset_at_path.clone(),
            plan_path: c.plan_path.clone(),
            unit: c.unit.clone(),
        }
    }
}

fn clean_opt(v: Option<String>) -> Option<String> {
    v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn config_file_path(data_dir: &Path) -> PathBuf {
    data_dir.join(USAGE_CONFIG_FILE)
}

fn load_configs_map(data_dir: &Path) -> BTreeMap<String, ProviderUsageConfig> {
    let path = config_file_path(data_dir);
    let Ok(raw) = fs::read_to_string(path) else {
        return BTreeMap::new();
    };
    serde_json::from_str::<BTreeMap<String, ProviderUsageConfig>>(&raw).unwrap_or_default()
}

fn write_configs_map(
    data_dir: &Path,
    map: &BTreeMap<String, ProviderUsageConfig>,
) -> Result<(), String> {
    fs::create_dir_all(data_dir).map_err(|e| format!("cannot create data dir: {e}"))?;
    let path = config_file_path(data_dir);
    let tmp = data_dir.join(format!("{USAGE_CONFIG_FILE}.tmp"));
    let body = serde_json::to_string_pretty(map)
        .map_err(|e| format!("cannot serialize usage config: {e}"))?;
    fs::write(&tmp, body).map_err(|e| format!("cannot write usage config: {e}"))?;
    fs::rename(&tmp, &path).map_err(|e| format!("cannot save usage config: {e}"))?;
    Ok(())
}

pub fn list_provider_usage_configs(data_dir: &Path) -> Vec<ProviderUsageConfigSummary> {
    load_configs_map(data_dir)
        .values()
        .map(ProviderUsageConfigSummary::from)
        .collect()
}

fn validate_and_resolve_draft(
    data_dir: &Path,
    draft: ProviderUsageConfigDraft,
) -> Result<ProviderUsageConfig, String> {
    let provider_id = draft.provider_id.trim().to_string();
    if provider_id.is_empty() {
        return Err("Provider ID is required.".to_string());
    }
    let preset = match draft.preset.trim() {
        "nan" => "nan".to_string(),
        "" | "declarative" => "declarative".to_string(),
        other => return Err(format!("Unsupported usage preset: {other}")),
    };
    let method = match draft.method.trim().to_uppercase().as_str() {
        "" | "GET" => "GET".to_string(),
        "POST" => "POST".to_string(),
        other => return Err(format!("Unsupported HTTP method: {other} (only GET or POST)")),
    };
    let auth_mode = match draft.auth_mode.trim() {
        "" | "providerApiKey" => "providerApiKey".to_string(),
        "bearer" => "bearer".to_string(),
        "cookie" => "cookie".to_string(),
        "none" => "none".to_string(),
        other => return Err(format!("Unsupported auth mode: {other}")),
    };
    let unit = match draft.unit.trim() {
        "" | "percent" => "percent".to_string(),
        "usd" => "usd".to_string(),
        "credits" => "credits".to_string(),
        "tokens" => "tokens".to_string(),
        other => return Err(format!("Unsupported unit: {other}")),
    };
    let url = draft.url.trim().to_string();
    if preset == "declarative" {
        if !url.starts_with("https://") {
            return Err("Usage endpoint URL must start with https://".to_string());
        }
        let host = url
            .trim_start_matches("https://")
            .split('/')
            .next()
            .unwrap_or("")
            .split(':')
            .next()
            .unwrap_or("")
            .to_lowercase();
        if host.is_empty() || host == "localhost" || host == "127.0.0.1" || host == "::1" {
            return Err("Usage endpoint URL must target a remote HTTPS host.".to_string());
        }
    }

    let existing = load_configs_map(data_dir).remove(&provider_id);
    let secret = if draft.clear_secret {
        None
    } else if let Some(s) = clean_opt(draft.secret) {
        Some(s)
    } else {
        existing.and_then(|e| e.secret)
    };

    let name = if draft.name.trim().is_empty() {
        provider_id.clone()
    } else {
        draft.name.trim().to_string()
    };

    Ok(ProviderUsageConfig {
        provider_id,
        name,
        enabled: draft.enabled,
        preset,
        url,
        method,
        auth_mode,
        secret,
        origin: clean_opt(draft.origin),
        referer: clean_opt(draft.referer),
        window_label: clean_opt(draft.window_label),
        used_path: clean_opt(draft.used_path),
        remaining_path: clean_opt(draft.remaining_path),
        limit_path: clean_opt(draft.limit_path),
        reset_at_path: clean_opt(draft.reset_at_path),
        plan_path: clean_opt(draft.plan_path),
        unit,
    })
}

pub fn save_provider_usage_config(
    data_dir: &Path,
    draft: ProviderUsageConfigDraft,
) -> Result<ProviderUsageConfigSummary, String> {
    let config = validate_and_resolve_draft(data_dir, draft)?;
    let mut map = load_configs_map(data_dir);
    let summary = ProviderUsageConfigSummary::from(&config);
    map.insert(config.provider_id.clone(), config);
    write_configs_map(data_dir, &map)?;
    Ok(summary)
}

pub fn delete_provider_usage_config(data_dir: &Path, provider_id: &str) -> Result<bool, String> {
    let mut map = load_configs_map(data_dir);
    let removed = map.remove(provider_id.trim()).is_some();
    if removed {
        write_configs_map(data_dir, &map)?;
    }
    Ok(removed)
}

pub async fn test_provider_usage_config(
    data_dir: &Path,
    pi_agent_dir: &Path,
    draft: ProviderUsageConfigDraft,
) -> Result<ProviderQuotaSnapshot, String> {
    let config = validate_and_resolve_draft(data_dir, draft)?;
    let client = reqwest::Client::builder()
        .timeout(QUOTA_TIMEOUT)
        .build()
        .map_err(|e| format!("HTTP client error: {e}"))?;
    Ok(fetch_custom_provider_quota(&client, pi_agent_dir, &config).await)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn clamp_percent(v: f64) -> f64 {
    if !v.is_finite() {
        return 0.0;
    }
    (v.clamp(0.0, 100.0) * 10.0).round() / 10.0
}

/// Pure dot-path lookup over JSON (supports object keys and array indices like `balance_infos.0.total_balance`).
pub fn json_dot_path<'a>(root: &'a Value, path: &str) -> Option<&'a Value> {
    let trimmed = path.trim().trim_start_matches("$.");
    if trimmed.is_empty() {
        return None;
    }
    let mut cur = root;
    for seg in trimmed.split('.') {
        if seg.is_empty() {
            continue;
        }
        if let Some(obj) = cur.as_object() {
            cur = obj.get(seg)?;
        } else {
            let arr = cur.as_array()?;
            let idx: usize = seg.parse().ok()?;
            cur = arr.get(idx)?;
        }
    }
    Some(cur)
}

fn value_as_f64(v: &Value) -> Option<f64> {
    v.as_f64()
        .or_else(|| v.as_str().and_then(|s| s.trim().parse::<f64>().ok()))
        .filter(|n| n.is_finite())
}

fn format_unit_amount(val: f64, unit: &str) -> String {
    match unit {
        "usd" => format!("${val:.2}"),
        "credits" => {
            if (val - val.round()).abs() < 0.05 {
                format!("{:.0} cr", val)
            } else {
                format!("{val:.1} cr")
            }
        }
        "tokens" => {
            if val >= 1_000_000.0 {
                format!("{:.1}M tok", val / 1_000_000.0)
            } else if val >= 1_000.0 {
                format!("{:.1}k tok", val / 1_000.0)
            } else {
                format!("{val:.0} tok")
            }
        }
        _ => format!("{val:.0}%"),
    }
}

fn read_provider_api_key(pi_agent_dir: &Path, provider_id: &str) -> Option<String> {
    let raw = fs::read_to_string(pi_agent_dir.join("models.json")).ok()?;
    let val: Value = serde_json::from_str(&raw).ok()?;
    val.get("providers")
        .and_then(|p| p.get(provider_id))
        .and_then(|entry| entry.get("apiKey"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

// ---------------------------------------------------------------------------
// Declarative & NaN Builders quota fetchers
// ---------------------------------------------------------------------------

pub fn parse_declarative_usage_response(
    config: &ProviderUsageConfig,
    body: &str,
) -> Result<ProviderQuotaSnapshot, String> {
    let val: Value =
        serde_json::from_str(body).map_err(|e| format!("invalid JSON response: {e}"))?;
    let used = config
        .used_path
        .as_deref()
        .and_then(|p| json_dot_path(&val, p))
        .and_then(value_as_f64);
    let remaining = config
        .remaining_path
        .as_deref()
        .and_then(|p| json_dot_path(&val, p))
        .and_then(value_as_f64);
    let limit = config
        .limit_path
        .as_deref()
        .and_then(|p| json_dot_path(&val, p))
        .and_then(value_as_f64);
    let reset_at = config
        .reset_at_path
        .as_deref()
        .and_then(|p| json_dot_path(&val, p))
        .and_then(|v| v.as_str().map(str::to_string));
    let plan = config
        .plan_path
        .as_deref()
        .and_then(|p| json_dot_path(&val, p))
        .and_then(|v| v.as_str().map(str::to_string));

    let (used_percent, detail) = match (used, remaining, limit) {
        (Some(u), _, Some(lim)) if lim > 0.0 => (
            clamp_percent((u / lim) * 100.0),
            Some(format!(
                "{} / {}",
                format_unit_amount(u, &config.unit),
                format_unit_amount(lim, &config.unit)
            )),
        ),
        (_, Some(rem), Some(lim)) if lim > 0.0 => (
            clamp_percent(((lim - rem) / lim) * 100.0),
            Some(format!(
                "{} left / {}",
                format_unit_amount(rem, &config.unit),
                format_unit_amount(lim, &config.unit)
            )),
        ),
        (Some(u), Some(rem), None) if (u + rem) > 0.0 => {
            let total = u + rem;
            (
                clamp_percent((u / total) * 100.0),
                Some(format!(
                    "{} / {}",
                    format_unit_amount(u, &config.unit),
                    format_unit_amount(total, &config.unit)
                )),
            )
        }
        (Some(u), None, None) if config.unit == "percent" => {
            let pct = if u > 0.0 && u <= 1.0 { u * 100.0 } else { u };
            (clamp_percent(pct), None)
        }
        (None, Some(rem), None) if config.unit == "percent" => {
            let pct = if rem > 0.0 && rem <= 1.0 {
                (1.0 - rem) * 100.0
            } else {
                100.0 - rem
            };
            (clamp_percent(pct), None)
        }
        (None, Some(rem), None) => (
            0.0,
            Some(format!("{} remaining", format_unit_amount(rem, &config.unit))),
        ),
        (Some(u), None, None) => (
            0.0,
            Some(format!("{} used", format_unit_amount(u, &config.unit))),
        ),
        _ => {
            return Err("None of the configured JSON paths matched numeric values.".to_string());
        }
    };

    let label = config
        .window_label
        .clone()
        .unwrap_or_else(|| "Usage".to_string());

    Ok(ProviderQuotaSnapshot {
        provider_id: config.provider_id.clone(),
        name: config.name.clone(),
        plan,
        windows: vec![QuotaWindow {
            label,
            used_percent,
            reset_at,
            reset_in_secs: None,
            detail,
        }],
        status: "ok".to_string(),
        error: None,
    })
}

/// Parses NaN Builders `/api/usage/credits` response.
pub fn parse_nan_credits_response(
    provider_id: &str,
    name: &str,
    body: &str,
    plan: Option<String>,
) -> Option<ProviderQuotaSnapshot> {
    let val: Value = serde_json::from_str(body).ok()?;
    let allowance = val
        .get("allowanceCredits")
        .or_else(|| val.get("allowance_credits"))
        .and_then(value_as_f64);
    let used = val
        .get("usedCredits")
        .or_else(|| val.get("used_credits"))
        .and_then(value_as_f64);
    let remaining = val
        .get("remainingCredits")
        .or_else(|| val.get("remaining_credits"))
        .and_then(value_as_f64);
    let pack = val
        .get("packCredits")
        .or_else(|| val.get("pack_credits"))
        .and_then(value_as_f64)
        .unwrap_or(0.0);
    let pack_expires = val
        .get("packExpiresAt")
        .or_else(|| val.get("pack_expires_at"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let reset_at = val
        .get("periodEnd")
        .or_else(|| val.get("period_end"))
        .or_else(|| val.get("resetsAt"))
        .and_then(Value::as_str)
        .map(str::to_string);

    let mut windows = Vec::new();
    if let Some(lim) = allowance
        && lim > 0.0
    {
        let u = used.unwrap_or_else(|| remaining.map(|r| (lim - r).max(0.0)).unwrap_or(0.0));
        windows.push(QuotaWindow {
            label: "Créditos".to_string(),
            used_percent: clamp_percent((u / lim) * 100.0),
            reset_at,
            reset_in_secs: None,
            detail: Some(format!(
                "{} / {}",
                format_unit_amount(u, "credits"),
                format_unit_amount(lim, "credits")
            )),
        });
    }
    if pack > 0.0 {
        windows.push(QuotaWindow {
            label: "+ Pack créditos".to_string(),
            used_percent: 0.0,
            reset_at: pack_expires,
            reset_in_secs: None,
            detail: Some(format!("{} left", format_unit_amount(pack, "credits"))),
        });
    }
    if windows.is_empty() {
        return None;
    }

    let frozen = val.get("frozen").and_then(Value::as_bool).unwrap_or(false);
    let frontier_paused = val
        .get("frontierPaused")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let note = if frozen {
        Some("Account credits frozen.".to_string())
    } else if frontier_paused {
        Some("Frontier models paused.".to_string())
    } else {
        None
    };

    Some(ProviderQuotaSnapshot {
        provider_id: provider_id.to_string(),
        name: name.to_string(),
        plan,
        windows,
        status: "ok".to_string(),
        error: note,
    })
}

/// Parses NaN Builders `/api/usage/quota` legacy per-model quota response.
pub fn parse_nan_legacy_quota_response(
    provider_id: &str,
    name: &str,
    body: &str,
    plan: Option<String>,
) -> Option<ProviderQuotaSnapshot> {
    let val: Value = serde_json::from_str(body).ok()?;
    let models = val.get("models").and_then(Value::as_array)?;
    let mut windows = Vec::new();
    for item in models {
        let Some(model) = item.get("model").and_then(Value::as_str) else {
            continue;
        };
        let cap = item.get("cap").and_then(value_as_f64).unwrap_or(0.0);
        if cap <= 0.0 {
            continue;
        }
        let used = item.get("tokensUsed").and_then(value_as_f64).unwrap_or(0.0);
        let reset_at = item
            .get("periodEnd")
            .and_then(Value::as_str)
            .map(str::to_string);
        windows.push(QuotaWindow {
            label: model.to_string(),
            used_percent: clamp_percent((used / cap) * 100.0),
            reset_at,
            reset_in_secs: None,
            detail: Some(format!(
                "{} / {}",
                format_unit_amount(used, "tokens"),
                format_unit_amount(cap, "tokens")
            )),
        });
    }
    if windows.is_empty() {
        return None;
    }
    Some(ProviderQuotaSnapshot {
        provider_id: provider_id.to_string(),
        name: name.to_string(),
        plan,
        windows,
        status: "ok".to_string(),
        error: None,
    })
}

async fn fetch_nan_quota(
    client: &reqwest::Client,
    config: &ProviderUsageConfig,
) -> ProviderQuotaSnapshot {
    let Some(cookie) = config
        .secret
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        return ProviderQuotaSnapshot {
            provider_id: config.provider_id.clone(),
            name: config.name.clone(),
            plan: None,
            windows: Vec::new(),
            status: "unavailable".to_string(),
            error: Some(
                "Paste your cloud.nan.builders Cookie header in Settings → Providers → Edit."
                    .to_string(),
            ),
        };
    };

    let build_req = |url: &str, timeout: Duration| {
        client
            .get(url)
            .timeout(timeout)
            .header("Cookie", cookie)
            .header("Origin", "https://cloud.nan.builders")
            .header("Referer", "https://cloud.nan.builders/")
            .header("Accept", "application/json")
    };

    // Best-effort billing tier check (2s deadline).
    let plan = match build_req("https://cloud-api.nan.builders/api/billing", BILLING_TIMEOUT)
        .send()
        .await
    {
        Ok(r) if r.status().is_success() => r.text().await.ok().and_then(|b| {
            let v: Value = serde_json::from_str(&b).ok()?;
            let premium = v
                .get("subscription")
                .and_then(|s| s.get("premium"))
                .and_then(Value::as_bool);
            match premium {
                Some(true) => Some("Plus".to_string()),
                Some(false) => Some("Free".to_string()),
                None => None,
            }
        }),
        _ => None,
    };

    // 1. Try modern credit billing endpoint first.
    let mut last_err = None;
    match build_req("https://cloud-api.nan.builders/api/usage/credits", QUOTA_TIMEOUT)
        .send()
        .await
    {
        Ok(r) if r.status().is_success() => {
            if let Ok(body) = r.text().await
                && let Some(snap) =
                    parse_nan_credits_response(&config.provider_id, &config.name, &body, plan.clone())
            {
                return snap;
            }
        }
        Ok(r) => {
            last_err = Some(format!("HTTP {}", r.status().as_u16()));
        }
        Err(e) => {
            last_err = Some(e.to_string());
        }
    }

    // 2. Fallback to legacy per-model quota endpoint.
    match build_req("https://cloud-api.nan.builders/api/usage/quota", QUOTA_TIMEOUT)
        .send()
        .await
    {
        Ok(r) if r.status().is_success() => {
            if let Ok(body) = r.text().await
                && let Some(snap) =
                    parse_nan_legacy_quota_response(&config.provider_id, &config.name, &body, plan)
            {
                return snap;
            }
            ProviderQuotaSnapshot {
                provider_id: config.provider_id.clone(),
                name: config.name.clone(),
                plan: None,
                windows: Vec::new(),
                status: "error".to_string(),
                error: Some("NaN returned an unrecognized usage payload.".to_string()),
            }
        }
        Ok(r) => ProviderQuotaSnapshot {
            provider_id: config.provider_id.clone(),
            name: config.name.clone(),
            plan: None,
            windows: Vec::new(),
            status: "unavailable".to_string(),
            error: Some(format!("HTTP {} (check your session cookie)", r.status().as_u16())),
        },
        Err(e) => ProviderQuotaSnapshot {
            provider_id: config.provider_id.clone(),
            name: config.name.clone(),
            plan: None,
            windows: Vec::new(),
            status: "error".to_string(),
            error: last_err.or_else(|| Some(e.to_string())),
        },
    }
}

async fn fetch_custom_provider_quota(
    client: &reqwest::Client,
    pi_agent_dir: &Path,
    config: &ProviderUsageConfig,
) -> ProviderQuotaSnapshot {
    if config.preset == "nan" {
        return fetch_nan_quota(client, config).await;
    }

    let mut req = if config.method == "POST" {
        client.post(&config.url)
    } else {
        client.get(&config.url)
    };
    req = req.header("Accept", "application/json");

    match config.auth_mode.as_str() {
        "providerApiKey" => {
            let Some(key) = read_provider_api_key(pi_agent_dir, &config.provider_id) else {
                return ProviderQuotaSnapshot {
                    provider_id: config.provider_id.clone(),
                    name: config.name.clone(),
                    plan: None,
                    windows: Vec::new(),
                    status: "unavailable".to_string(),
                    error: Some("Provider API key is not set.".to_string()),
                };
            };
            req = req.header("Authorization", format!("Bearer {key}"));
        }
        "bearer" => {
            let Some(tok) = config
                .secret
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            else {
                return ProviderQuotaSnapshot {
                    provider_id: config.provider_id.clone(),
                    name: config.name.clone(),
                    plan: None,
                    windows: Vec::new(),
                    status: "unavailable".to_string(),
                    error: Some("Bearer token is not configured.".to_string()),
                };
            };
            req = req.header("Authorization", format!("Bearer {tok}"));
        }
        "cookie" => {
            let Some(cookie) = config
                .secret
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            else {
                return ProviderQuotaSnapshot {
                    provider_id: config.provider_id.clone(),
                    name: config.name.clone(),
                    plan: None,
                    windows: Vec::new(),
                    status: "unavailable".to_string(),
                    error: Some("Session cookie is not configured.".to_string()),
                };
            };
            req = req.header("Cookie", cookie);
        }
        _ => {}
    }

    if let Some(origin) = config.origin.as_deref() {
        req = req.header("Origin", origin);
    }
    if let Some(referer) = config.referer.as_deref() {
        req = req.header("Referer", referer);
    }

    match req.send().await {
        Ok(r) if r.status().is_success() => match r.text().await {
            Ok(body) => match parse_declarative_usage_response(config, &body) {
                Ok(snap) => snap,
                Err(err) => ProviderQuotaSnapshot {
                    provider_id: config.provider_id.clone(),
                    name: config.name.clone(),
                    plan: None,
                    windows: Vec::new(),
                    status: "error".to_string(),
                    error: Some(err),
                },
            },
            Err(e) => ProviderQuotaSnapshot {
                provider_id: config.provider_id.clone(),
                name: config.name.clone(),
                plan: None,
                windows: Vec::new(),
                status: "error".to_string(),
                error: Some(e.to_string()),
            },
        },
        Ok(r) => ProviderQuotaSnapshot {
            provider_id: config.provider_id.clone(),
            name: config.name.clone(),
            plan: None,
            windows: Vec::new(),
            status: "unavailable".to_string(),
            error: Some(format!("HTTP {}", r.status().as_u16())),
        },
        Err(e) => ProviderQuotaSnapshot {
            provider_id: config.provider_id.clone(),
            name: config.name.clone(),
            plan: None,
            windows: Vec::new(),
            status: "error".to_string(),
            error: Some(e.to_string()),
        },
    }
}

// ---------------------------------------------------------------------------
// Built-in providers: Claude, OpenAI/Codex, Antigravity (local probe)
// ---------------------------------------------------------------------------

/// Parses Claude Code's `GET https://api.anthropic.com/api/oauth/usage` JSON.
pub fn parse_claude_usage_response(body: &str) -> Result<ProviderQuotaSnapshot, String> {
    let val: Value =
        serde_json::from_str(body).map_err(|e| format!("invalid Claude usage JSON: {e}"))?;
    let mut windows = Vec::new();
    for (key, label) in [("five_hour", "5h"), ("seven_day", "7d")] {
        if let Some(obj) = val.get(key).and_then(Value::as_object)
            && let Some(util) = obj.get("utilization").and_then(Value::as_f64)
        {
            let pct = if util <= 1.0 && util > 0.0 {
                util * 100.0
            } else {
                util
            };
            let reset_at = obj
                .get("resets_at")
                .and_then(Value::as_str)
                .map(str::to_string);
            windows.push(QuotaWindow {
                label: label.to_string(),
                used_percent: clamp_percent(pct),
                reset_at,
                reset_in_secs: None,
                detail: None,
            });
        }
    }
    Ok(ProviderQuotaSnapshot {
        provider_id: "claude".to_string(),
        name: "Claude".to_string(),
        plan: val
            .get("plan")
            .and_then(Value::as_str)
            .map(str::to_string),
        windows,
        status: "ok".to_string(),
        error: None,
    })
}

/// Parses OpenAI / ChatGPT Codex's `GET https://chatgpt.com/backend-api/wham/usage` JSON.
pub fn parse_openai_usage_response(body: &str) -> Result<ProviderQuotaSnapshot, String> {
    let val: Value =
        serde_json::from_str(body).map_err(|e| format!("invalid OpenAI usage JSON: {e}"))?;
    let mut windows = Vec::new();
    if let Some(rl) = val.get("rate_limit").and_then(Value::as_object) {
        for (key, label) in [("primary_window", "5h"), ("secondary_window", "7d")] {
            if let Some(win) = rl.get(key).and_then(Value::as_object)
                && let Some(pct) = win.get("used_percent").and_then(Value::as_f64)
            {
                let reset_in = win.get("reset_after_seconds").and_then(Value::as_i64);
                windows.push(QuotaWindow {
                    label: label.to_string(),
                    used_percent: clamp_percent(pct),
                    reset_at: None,
                    reset_in_secs: reset_in,
                    detail: None,
                });
            }
        }
    }
    let plan = val
        .get("plan_type")
        .and_then(Value::as_str)
        .map(str::to_string);
    Ok(ProviderQuotaSnapshot {
        provider_id: "openai".to_string(),
        name: "ChatGPT / Codex".to_string(),
        plan,
        windows,
        status: "ok".to_string(),
        error: None,
    })
}

/// Parses official `agy -p /usage --output-format json` stdout.
pub fn parse_antigravity_quota_response(body: &str) -> Option<ProviderQuotaSnapshot> {
    // Try full body first (handles both compact and pretty JSON); if `agy` prefixed
    // log lines on stdout, fall back to the last single-line JSON object.
    let val: Value = serde_json::from_str(body.trim()).ok().or_else(|| {
        let line = body
            .lines()
            .rev()
            .map(str::trim)
            .find(|l| l.starts_with('{') && l.ends_with('}'))?;
        serde_json::from_str(line).ok()
    })?;
    let groups = val
        .pointer("/command/data/groups")
        .or_else(|| val.get("groups"))
        .and_then(Value::as_array)?;

    let mut windows: Vec<QuotaWindow> = Vec::new();
    for group in groups {
        let group_name = group
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("Antigravity");
        let short_group = if group_name.to_lowercase().contains("gemini") {
            "Gemini"
        } else if group_name.to_lowercase().contains("claude")
            || group_name.to_lowercase().contains("gpt")
        {
            "Claude / GPT"
        } else {
            group_name
        };

        let Some(buckets) = group.get("buckets").and_then(Value::as_array) else {
            continue;
        };
        for bucket in buckets {
            let Some(rem) = bucket
                .get("remaining_fraction")
                .or_else(|| bucket.get("remainingFraction"))
                .and_then(value_as_f64)
            else {
                continue;
            };
            let win_tag = match bucket.get("window").and_then(Value::as_str) {
                Some("5h") => "5h",
                Some("weekly") => "7d",
                Some(other) if !other.is_empty() => other,
                _ => bucket
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("Quota"),
            };
            let reset_at = bucket
                .get("reset_time")
                .or_else(|| bucket.get("resetTime"))
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty() && !s.starts_with("1970-01-01"))
                .map(str::to_string);
            windows.push(QuotaWindow {
                label: format!("{short_group} · {win_tag}"),
                used_percent: clamp_percent((1.0 - rem) * 100.0),
                reset_at,
                reset_in_secs: None,
                detail: None,
            });
        }
    }

    if windows.is_empty() {
        return None;
    }

    Some(ProviderQuotaSnapshot {
        provider_id: "antigravity".to_string(),
        name: "Antigravity".to_string(),
        plan: Some("CLI".to_string()),
        windows,
        status: "ok".to_string(),
        error: None,
    })
}

async fn fetch_antigravity_quota(home_dir: &Path) -> Option<ProviderQuotaSnapshot> {
    let agy_bin = crate::acp::catalog::find_in_path("agy")?;

    // Official non-interactive quota report (`agy -p /usage --output-format json`),
    // same strategy CodexBar uses for agy >= 1.2.2.
    let bin = agy_bin.clone();
    let task = tokio::task::spawn_blocking(move || {
        std::process::Command::new(&bin)
            .args(["-p", "/usage", "--output-format", "json"])
            .env("AGY_CLI_DISABLE_AUTO_UPDATE", "true")
            .current_dir(std::env::temp_dir())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .output()
    });

    if let Ok(Ok(Ok(out))) = tokio::time::timeout(AGY_CLI_TIMEOUT, task).await
        && out.status.success()
    {
        let stdout = String::from_utf8_lossy(&out.stdout);
        if let Some(snap) = parse_antigravity_quota_response(&stdout) {
            return Some(snap);
        }
    }

    // Fallback when `agy` is not signed in or `/usage` timed out.
    let has_auth = crate::acp::catalog::has_antigravity_auth_in(home_dir);
    Some(ProviderQuotaSnapshot {
        provider_id: "antigravity".to_string(),
        name: "Antigravity".to_string(),
        plan: if has_auth {
            Some("CLI".to_string())
        } else {
            None
        },
        windows: Vec::new(),
        status: "unavailable".to_string(),
        error: if has_auth {
            Some("Could not read live quota from `agy -p /usage` — tracking local Nabla usage below.".to_string())
        } else {
            Some("Sign in via Settings → Providers → Antigravity.".to_string())
        },
    })
}

fn extract_claude_token_from_json(raw: &str) -> Option<String> {
    let val: Value = serde_json::from_str(raw).ok()?;
    val.get("claudeAiOauth")
        .and_then(|o| o.get("accessToken"))
        .or_else(|| val.get("accessToken"))
        .or_else(|| val.get("access_token"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn read_claude_oauth_token(pi_agent_dir: &Path, home_dir: &Path) -> Option<String> {
    let path = home_dir.join(".claude").join(".credentials.json");
    if let Ok(raw) = fs::read_to_string(path)
        && let Some(tok) = extract_claude_token_from_json(&raw)
    {
        return Some(tok);
    }

    let nabla_auth = pi_agent_dir.join("auth.json");
    if let Ok(raw) = fs::read_to_string(nabla_auth)
        && let Ok(val) = serde_json::from_str::<Value>(&raw)
        && let Some(entry) = val.get("anthropic").and_then(Value::as_object)
        && entry.get("type").and_then(Value::as_str) == Some("oauth")
        && let Some(tok) = entry
            .get("access")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
    {
        return Some(tok.to_string());
    }

    #[cfg(target_os = "macos")]
    {
        if let Ok(out) = std::process::Command::new("security")
            .args([
                "find-generic-password",
                "-s",
                "Claude Code-credentials",
                "-w",
            ])
            .output()
            && out.status.success()
        {
            let raw = String::from_utf8_lossy(&out.stdout);
            if let Some(tok) = extract_claude_token_from_json(raw.trim()) {
                return Some(tok);
            }
        }
    }

    None
}

fn read_openai_oauth_token(
    pi_agent_dir: &Path,
    home_dir: &Path,
) -> Option<(String, Option<String>)> {
    let nabla_auth = pi_agent_dir.join("auth.json");
    if let Ok(raw) = fs::read_to_string(nabla_auth)
        && let Ok(val) = serde_json::from_str::<Value>(&raw)
        && let Some(entry) = val.get("openai-codex").and_then(Value::as_object)
        && entry.get("type").and_then(Value::as_str) == Some("oauth")
        && let Some(token) = entry
            .get("access")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
    {
        let account_id = entry
            .get("accountId")
            .and_then(Value::as_str)
            .map(str::to_string);
        return Some((token.to_string(), account_id));
    }

    let codex_auth = home_dir.join(".codex").join("auth.json");
    if let Ok(raw) = fs::read_to_string(codex_auth)
        && let Ok(val) = serde_json::from_str::<Value>(&raw)
        && let Some(token) = val
            .get("tokens")
            .and_then(|t| t.get("access_token"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
    {
        let account_id = val
            .get("tokens")
            .and_then(|t| t.get("account_id"))
            .and_then(Value::as_str)
            .map(str::to_string);
        return Some((token.to_string(), account_id));
    }
    None
}

async fn fetch_claude_quota(
    client: &reqwest::Client,
    pi_agent_dir: &Path,
    home_dir: &Path,
) -> Option<ProviderQuotaSnapshot> {
    let token = match read_claude_oauth_token(pi_agent_dir, home_dir) {
        Some(t) => t,
        None => {
            if crate::acp::catalog::find_in_path("claude").is_some() {
                return Some(ProviderQuotaSnapshot {
                    provider_id: "claude".to_string(),
                    name: "Claude".to_string(),
                    plan: None,
                    windows: Vec::new(),
                    status: "unavailable".to_string(),
                    error: Some(
                        "Sign in with `claude auth login` (or grant macOS Keychain access)."
                            .to_string(),
                    ),
                });
            }
            return None;
        }
    };
    let resp = client
        .get("https://api.anthropic.com/api/oauth/usage")
        .header("Authorization", format!("Bearer {token}"))
        .header("anthropic-beta", "oauth-2025-04-20")
        .header("Accept", "application/json")
        .send()
        .await;
    match resp {
        Ok(r) if r.status().is_success() => match r.text().await {
            Ok(body) => match parse_claude_usage_response(&body) {
                Ok(snap) if !snap.windows.is_empty() => Some(snap),
                Ok(_) => None,
                Err(err) => Some(ProviderQuotaSnapshot {
                    provider_id: "claude".to_string(),
                    name: "Claude".to_string(),
                    plan: None,
                    windows: Vec::new(),
                    status: "error".to_string(),
                    error: Some(err),
                }),
            },
            Err(e) => Some(ProviderQuotaSnapshot {
                provider_id: "claude".to_string(),
                name: "Claude".to_string(),
                plan: None,
                windows: Vec::new(),
                status: "error".to_string(),
                error: Some(e.to_string()),
            }),
        },
        Ok(r) => Some(ProviderQuotaSnapshot {
            provider_id: "claude".to_string(),
            name: "Claude".to_string(),
            plan: None,
            windows: Vec::new(),
            status: "unavailable".to_string(),
            error: Some(format!("HTTP {}", r.status().as_u16())),
        }),
        Err(e) => Some(ProviderQuotaSnapshot {
            provider_id: "claude".to_string(),
            name: "Claude".to_string(),
            plan: None,
            windows: Vec::new(),
            status: "error".to_string(),
            error: Some(e.to_string()),
        }),
    }
}

async fn fetch_openai_quota(
    client: &reqwest::Client,
    pi_agent_dir: &Path,
    home_dir: &Path,
) -> Option<ProviderQuotaSnapshot> {
    let (token, account_id) = read_openai_oauth_token(pi_agent_dir, home_dir)?;
    let mut req = client
        .get("https://chatgpt.com/backend-api/wham/usage")
        .header("Authorization", format!("Bearer {token}"))
        .header("Accept", "application/json");
    if let Some(acc) = account_id {
        req = req.header("ChatGPT-Account-Id", acc);
    }
    match req.send().await {
        Ok(r) if r.status().is_success() => match r.text().await {
            Ok(body) => match parse_openai_usage_response(&body) {
                Ok(snap) if !snap.windows.is_empty() => Some(snap),
                Ok(_) => None,
                Err(err) => Some(ProviderQuotaSnapshot {
                    provider_id: "openai".to_string(),
                    name: "ChatGPT / Codex".to_string(),
                    plan: None,
                    windows: Vec::new(),
                    status: "error".to_string(),
                    error: Some(err),
                }),
            },
            Err(e) => Some(ProviderQuotaSnapshot {
                provider_id: "openai".to_string(),
                name: "ChatGPT / Codex".to_string(),
                plan: None,
                windows: Vec::new(),
                status: "error".to_string(),
                error: Some(e.to_string()),
            }),
        },
        Ok(r) => Some(ProviderQuotaSnapshot {
            provider_id: "openai".to_string(),
            name: "ChatGPT / Codex".to_string(),
            plan: None,
            windows: Vec::new(),
            status: "unavailable".to_string(),
            error: Some(format!("HTTP {}", r.status().as_u16())),
        }),
        Err(e) => Some(ProviderQuotaSnapshot {
            provider_id: "openai".to_string(),
            name: "ChatGPT / Codex".to_string(),
            plan: None,
            windows: Vec::new(),
            status: "error".to_string(),
            error: Some(e.to_string()),
        }),
    }
}

// ---------------------------------------------------------------------------
// Local JSONL usage aggregator
// ---------------------------------------------------------------------------

fn collect_jsonl_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().and_then(|n| n.to_str()) == Some(".trash") {
                continue;
            }
            collect_jsonl_files(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            out.push(path);
        }
    }
}

fn entry_epoch_secs(entry: &Value, msg: &Value) -> Option<u64> {
    if let Some(ms) = msg.get("timestamp").and_then(Value::as_u64) {
        return Some(if ms > 10_000_000_000 { ms / 1000 } else { ms });
    }
    if let Some(s) = entry.get("timestamp").and_then(Value::as_str)
        && let Ok(n) = s.parse::<u64>()
    {
        return Some(if n > 10_000_000_000 { n / 1000 } else { n });
    }
    None
}

/// Aggregates local token & USD usage from all JSONL session files under `dirs`.
pub fn aggregate_local_usage(dirs: &[PathBuf], now: u64) -> Vec<LocalUsageRow> {
    let mut files = Vec::new();
    for dir in dirs {
        collect_jsonl_files(dir, &mut files);
    }

    let cutoff_day = now.saturating_sub(86_400);
    let cutoff_week = now.saturating_sub(7 * 86_400);
    let cutoff_month = now.saturating_sub(30 * 86_400);

    let mut by_key: HashMap<(String, String), LocalUsageRow> = HashMap::new();

    for path in files {
        let Ok(file) = fs::File::open(&path) else {
            continue;
        };
        let file_mtime = fs::metadata(&path)
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(now);
        if file_mtime < cutoff_month {
            continue;
        }
        for line in BufReader::new(file).lines().map_while(Result::ok) {
            let trimmed = line.trim();
            if trimmed.is_empty() || !trimmed.contains("\"usage\"") {
                continue;
            }
            let Ok(val) = serde_json::from_str::<Value>(trimmed) else {
                continue;
            };
            let msg = if val.get("type").and_then(Value::as_str) == Some("message") {
                match val.get("message") {
                    Some(m) => m,
                    None => continue,
                }
            } else {
                &val
            };
            if msg.get("role").and_then(Value::as_str) != Some("assistant") {
                continue;
            }
            let Some(usage) = msg.get("usage").and_then(Value::as_object) else {
                continue;
            };
            let tokens = usage
                .get("totalTokens")
                .and_then(Value::as_u64)
                .unwrap_or_else(|| {
                    usage.get("input").and_then(Value::as_u64).unwrap_or(0)
                        + usage.get("output").and_then(Value::as_u64).unwrap_or(0)
                        + usage.get("cacheRead").and_then(Value::as_u64).unwrap_or(0)
                        + usage.get("cacheWrite").and_then(Value::as_u64).unwrap_or(0)
                });
            let cost = usage
                .get("cost")
                .and_then(|c| c.get("total"))
                .and_then(Value::as_f64)
                .filter(|c| c.is_finite() && *c >= 0.0)
                .unwrap_or(0.0);
            if tokens == 0 && cost == 0.0 {
                continue;
            }
            let ts = entry_epoch_secs(&val, msg).unwrap_or(file_mtime);
            if ts < cutoff_month {
                continue;
            }
            let provider = msg
                .get("provider")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_string();
            let model = msg
                .get("model")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_string();

            let row = by_key
                .entry((provider.clone(), model.clone()))
                .or_insert_with(|| LocalUsageRow {
                    provider,
                    model,
                    today_tokens: 0,
                    week_tokens: 0,
                    month_tokens: 0,
                    today_cost: 0.0,
                    week_cost: 0.0,
                    month_cost: 0.0,
                    turns: 0,
                });
            row.turns += 1;
            row.month_tokens += tokens;
            row.month_cost += cost;
            if ts >= cutoff_week {
                row.week_tokens += tokens;
                row.week_cost += cost;
            }
            if ts >= cutoff_day {
                row.today_tokens += tokens;
                row.today_cost += cost;
            }
        }
    }

    let mut rows: Vec<LocalUsageRow> = by_key.into_values().collect();
    rows.sort_by(|a, b| {
        b.week_tokens
            .cmp(&a.week_tokens)
            .then_with(|| b.month_tokens.cmp(&a.month_tokens))
    });
    rows
}

/// Collects live subscription quotas + custom provider quotas + local token/cost usage.
pub async fn collect_usage_snapshot(
    data_dir: &Path,
    pi_agent_dir: &Path,
    home_dir: &Path,
) -> UsageSnapshot {
    let now = now_secs();
    let local_dirs = vec![data_dir.join("sessions"), pi_agent_dir.join("sessions")];
    let local = aggregate_local_usage(&local_dirs, now);
    let total_today_tokens: u64 = local.iter().map(|r| r.today_tokens).sum();
    let total_week_tokens: u64 = local.iter().map(|r| r.week_tokens).sum();
    let total_today_cost: f64 = local.iter().map(|r| r.today_cost).sum();
    let total_week_cost: f64 = local.iter().map(|r| r.week_cost).sum();

    let mut quotas = Vec::new();
    let custom_configs: Vec<ProviderUsageConfig> = load_configs_map(data_dir)
        .into_values()
        .filter(|c| c.enabled)
        .collect();

    if let Ok(client) = reqwest::Client::builder().timeout(QUOTA_TIMEOUT).build() {
        let (claude, openai, agy) = tokio::join!(
            fetch_claude_quota(&client, pi_agent_dir, home_dir),
            fetch_openai_quota(&client, pi_agent_dir, home_dir),
            fetch_antigravity_quota(home_dir),
        );
        if let Some(q) = openai {
            quotas.push(q);
        }
        if let Some(q) = claude {
            quotas.push(q);
        }
        if let Some(q) = agy {
            quotas.push(q);
        }

        for cfg in &custom_configs {
            quotas.push(fetch_custom_provider_quota(&client, pi_agent_dir, cfg).await);
        }
    }

    UsageSnapshot {
        quotas,
        local,
        total_today_tokens,
        total_week_tokens,
        total_today_cost,
        total_week_cost,
        fetched_at: now,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_claude_and_openai_quota_payloads() {
        let claude = parse_claude_usage_response(
            r#"{"five_hour":{"utilization":28.5,"resets_at":"2026-11-27T12:00:00Z"},"seven_day":{"utilization":64.0,"resets_at":"2026-12-01T00:00:00Z"}}"#,
        )
        .unwrap();
        assert_eq!(claude.windows.len(), 2);
        assert_eq!(claude.windows[0].label, "5h");
        assert_eq!(claude.windows[0].used_percent, 28.5);
        assert_eq!(claude.windows[1].label, "7d");
        assert_eq!(claude.windows[1].used_percent, 64.0);

        let openai = parse_openai_usage_response(
            r#"{"plan_type":"plus","rate_limit":{"primary_window":{"used_percent":19,"reset_after_seconds":5400},"secondary_window":{"used_percent":42,"reset_after_seconds":180000}}}"#,
        )
        .unwrap();
        assert_eq!(openai.plan.as_deref(), Some("plus"));
        assert_eq!(openai.windows.len(), 2);
        assert_eq!(openai.windows[0].used_percent, 19.0);
        assert_eq!(openai.windows[0].reset_in_secs, Some(5400));
    }

    #[test]
    fn parses_antigravity_cli_usage_payload() {
        let payload = r#"{
            "status": "SUCCESS",
            "command": {
                "name": "usage",
                "data": {
                    "groups": [
                        {
                            "name": "Gemini Models",
                            "buckets": [
                                { "id": "gemini-weekly", "window": "weekly", "remaining_fraction": 0.674, "reset_time": "2026-12-03T08:19:49Z" },
                                { "id": "gemini-5h", "window": "5h", "remaining_fraction": 0.64, "reset_time": "2026-11-27T12:33:15Z" }
                            ]
                        },
                        {
                            "name": "Claude and GPT models",
                            "buckets": [
                                { "id": "3p-weekly", "window": "weekly", "remaining_fraction": 1.0, "reset_time": "1970-01-01T00:00:00Z" }
                            ]
                        }
                    ]
                }
            }
        }"#;
        let snap = parse_antigravity_quota_response(payload).unwrap();
        assert_eq!(snap.windows.len(), 3);
        assert_eq!(snap.windows[0].label, "Gemini · 7d");
        assert_eq!(snap.windows[0].used_percent, 32.6);
        assert_eq!(snap.windows[0].reset_at.as_deref(), Some("2026-12-03T08:19:49Z"));
        assert_eq!(snap.windows[1].label, "Gemini · 5h");
        assert_eq!(snap.windows[1].used_percent, 36.0);
        assert_eq!(snap.windows[2].label, "Claude / GPT · 7d");
        assert_eq!(snap.windows[2].used_percent, 0.0);
        assert_eq!(snap.windows[2].reset_at, None);
    }

    #[test]
    fn parses_declarative_and_nan_payloads() {
        let cfg = ProviderUsageConfig {
            provider_id: "openrouter".into(),
            name: "OpenRouter".into(),
            enabled: true,
            preset: "declarative".into(),
            url: "https://openrouter.ai/api/v1/auth/key".into(),
            method: "GET".into(),
            auth_mode: "providerApiKey".into(),
            secret: None,
            origin: None,
            referer: None,
            window_label: Some("Credits".into()),
            used_path: Some("data.usage".into()),
            remaining_path: None,
            limit_path: Some("data.limit".into()),
            reset_at_path: None,
            plan_path: Some("data.label".into()),
            unit: "usd".into(),
        };
        let snap = parse_declarative_usage_response(
            &cfg,
            r#"{"data":{"label":"Pro","usage":4.5,"limit":10.0}}"#,
        )
        .unwrap();
        assert_eq!(snap.plan.as_deref(), Some("Pro"));
        assert_eq!(snap.windows[0].used_percent, 45.0);
        assert_eq!(snap.windows[0].detail.as_deref(), Some("$4.50 / $10.00"));

        let nan = parse_nan_credits_response(
            "nan",
            "NaN Builders",
            r#"{"allowanceCredits":500,"usedCredits":125,"packCredits":50,"periodEnd":"2026-12-01T00:00:00Z"}"#,
            Some("Plus".into()),
        )
        .unwrap();
        assert_eq!(nan.plan.as_deref(), Some("Plus"));
        assert_eq!(nan.windows.len(), 2);
        assert_eq!(nan.windows[0].used_percent, 25.0);
        assert_eq!(nan.windows[0].detail.as_deref(), Some("125 cr / 500 cr"));
        assert_eq!(nan.windows[1].label, "+ Pack créditos");
    }

    #[test]
    fn rejects_non_https_or_localhost_declarative_urls() {
        let dir = std::env::temp_dir().join("nabla-usage-sec-test");
        let _ = fs::remove_dir_all(&dir);
        let base_draft = ProviderUsageConfigDraft {
            provider_id: "custom".into(),
            name: "Custom".into(),
            enabled: true,
            preset: "declarative".into(),
            url: "http://example.com/usage".into(),
            method: "GET".into(),
            auth_mode: "none".into(),
            secret: None,
            clear_secret: false,
            origin: None,
            referer: None,
            window_label: None,
            used_path: Some("used".into()),
            remaining_path: None,
            limit_path: None,
            reset_at_path: None,
            plan_path: None,
            unit: "percent".into(),
        };
        assert!(save_provider_usage_config(&dir, base_draft.clone()).is_err());
        let mut local_draft = base_draft;
        local_draft.url = "https://127.0.0.1:8080/usage".into();
        assert!(save_provider_usage_config(&dir, local_draft).is_err());
    }

    #[test]
    fn aggregates_local_session_jsonl_usage() {
        let dir = std::env::temp_dir().join("nabla-usage-test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let now = now_secs();
        let sample = format!(
            "{{\"type\":\"message\",\"message\":{{\"role\":\"assistant\",\"provider\":\"openai\",\"model\":\"gpt-5\",\"usage\":{{\"totalTokens\":1500,\"cost\":{{\"total\":0.012}}}},\"timestamp\":{}}}}}\n",
            now * 1000
        );
        fs::write(dir.join("s1.jsonl"), sample).unwrap();

        let rows = aggregate_local_usage(&[dir.clone()], now);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].provider, "openai");
        assert_eq!(rows[0].today_tokens, 1500);
        assert!((rows[0].today_cost - 0.012).abs() < 1e-6);
        let _ = fs::remove_dir_all(&dir);
    }
}
