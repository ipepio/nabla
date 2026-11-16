// Providers over pi's own configuration file.
// Discovery and saving both target `~/.pi/agent/models.json`: the file pi
// already reads, so a provider added here needs no extension and no second
// store (S3). The api key lives in that plain-text file (S4, known debt) and
// never crosses to the webview: `ProviderSummary` carries only `has_api_key`.

use std::path::{Path, PathBuf};

pub const DEFAULT_API: &str = "openai-completions";

/// Discovery is a form action, not a background job: it must fail fast enough to
/// tell the human their url or key is wrong (S2).
pub const MODELS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSummary {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub api: String,
    pub model_count: usize,
    pub has_api_key: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelOption {
    pub id: String,
    pub name: String,
}

/// S2/S3 of `models-by-provider.md`: one provider with the models pi would use,
/// in the order the file declares them. This is the read the grouped selector
/// and the per-model editor both build on.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderModels {
    pub provider: String,
    pub name: String,
    pub models: Vec<ModelInfo>,
}

/// S3: one model of a provider, as the editor reads it. Pi's own field names
/// (`contextWindow`, `maxTokens`, `cost.input`, `cost.output`, `input`) are
/// mapped onto flat camelCase fields; `enabled` is Nabla's mark, not pi's, and
/// defaults to `true` when the file does not carry it.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub context_window: Option<u64>,
    pub max_tokens: Option<u64>,
    pub input_cost: Option<f64>,
    pub output_cost: Option<f64>,
    /// Input modalities: `"text"` and `"image"`, pi's `input` array.
    pub modalities: Vec<String>,
    pub enabled: bool,
}

/// S3: what the editor writes for one model. An absent number means "keep what
/// the file has", so a form that only edits the cost never wipes a context
/// window; an absent `enabled` means enabled, the same default the read side
/// applies.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderModelDraft {
    pub provider: String,
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub context_window: Option<u64>,
    #[serde(default)]
    pub max_tokens: Option<u64>,
    #[serde(default)]
    pub input_cost: Option<f64>,
    #[serde(default)]
    pub output_cost: Option<f64>,
    #[serde(default)]
    pub modalities: Vec<String>,
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
}

/// The flag is enabled unless the payload says otherwise (L3/S3).
fn enabled_by_default() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderDraft {
    pub id: String,
    #[serde(default)]
    pub name: String,
    pub base_url: String,
    #[serde(default)]
    pub api: String,
    pub api_key: String,
    #[serde(default)]
    pub models: Vec<ModelOption>,
}

#[derive(Debug)]
pub enum ProvidersError {
    Io(String),
    Parse(String),
    InvalidId(String),
    /// S3 of `models-by-provider.md`: a model id the file must not receive.
    InvalidModelId(String),
    InvalidBaseUrl(String),
    EmptyApiKey,
    /// S2: a remote provider with nothing to authenticate with. Only discovery
    /// can hit this, and a provider on the machine itself is exempt (see
    /// `is_local_endpoint`).
    MissingApiKey(String),
    /// A provider entry the file does not have. Only `set_provider_key` can hit
    /// this: a key belongs to an entry that exists, and nothing invents providers.
    UnknownProvider(String),
    Http(String),
}

impl std::fmt::Display for ProvidersError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProvidersError::Io(m) => write!(f, "providers io error: {m}"),
            ProvidersError::Parse(m) => write!(f, "providers parse error: {m}"),
            ProvidersError::InvalidId(id) => write!(
                f,
                "provider id must match ^[a-z0-9-]{{1,32}}$: {id:?}"
            ),
            ProvidersError::InvalidModelId(id) => write!(
                f,
                "model id must be 1-{MODEL_ID_MAX} chars of [a-z0-9-._/:] and contain a \
                 letter or digit: {id:?}"
            ),
            ProvidersError::InvalidBaseUrl(url) => {
                write!(f, "provider base url must be http(s): {url:?}")
            }
            ProvidersError::EmptyApiKey => write!(f, "provider api key must not be empty"),
            ProvidersError::MissingApiKey(id) => {
                write!(f, "provider {id} has no api key configured")
            }
            ProvidersError::UnknownProvider(id) => write!(f, "unknown provider: {id}"),
            ProvidersError::Http(m) => write!(f, "providers http error: {m}"),
        }
    }
}

impl std::error::Error for ProvidersError {}

pub type ProvidersResult<T> = Result<T, ProvidersError>;

/// Path of pi's provider configuration inside a home directory.
pub fn models_path(home_dir: &Path) -> PathBuf {
    home_dir.join(".pi").join("agent").join("models.json")
}

/// Reads every provider pi knows, without the api keys.
///
/// The file is parsed as a `serde_json::Value` (not a typed struct) so keys
/// Nabla does not model — `modelOverrides`, `timeoutSeconds`, top-level settings
/// — survive a later write untouched (S3). A missing file reads as no providers:
/// a fresh install has simply never added one.
pub fn read_providers(path: &Path) -> ProvidersResult<Vec<ProviderSummary>> {
    let (root, _) = read_root(path)?;
    let Some(providers) = root.get("providers").and_then(|v| v.as_object()) else {
        return Ok(Vec::new());
    };
    Ok(providers
        .iter()
        .map(|(id, entry)| summary_of(id, entry))
        .collect())
}

/// S3: writes one provider into `providers.<id>` — backup, merge, atomic write.
///
/// Nothing outside that entry is touched: every other top-level key and every
/// other provider stays as it was, including their `modelOverrides`. Unknown
/// keys inside the provider being written are kept too, because pi owns them
/// (`timeoutSeconds`, per-model overrides).
pub fn save_provider(path: &Path, draft: &ProviderDraft) -> ProvidersResult<ProviderSummary> {
    let id = draft.id.trim();
    validate_id(id)?;
    let base_url = draft.base_url.trim();
    validate_base_url(base_url)?;
    let api_key = draft.api_key.trim();
    if api_key.is_empty() {
        return Err(ProvidersError::EmptyApiKey);
    }

    let (mut root, existed) = read_root(path)?;
    let root_object = root.as_object_mut().ok_or_else(|| {
        ProvidersError::Parse(format!("{}: root is not a JSON object", path.display()))
    })?;

    // `providers` is created when absent; a non-object there is a broken file.
    let providers_value = root_object
        .entry("providers")
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
    let providers = providers_value.as_object_mut().ok_or_else(|| {
        ProvidersError::Parse(format!("{}: `providers` is not a JSON object", path.display()))
    })?;

    // Merge into the existing entry instead of replacing it, so keys pi owns
    // inside the provider are not dropped by an update.
    let mut entry = match providers.get(id) {
        Some(serde_json::Value::Object(existing)) => existing.clone(),
        _ => serde_json::Map::new(),
    };
    // An empty `api` in the draft means "keep what the file has"; only a file
    // without one falls back to the default (frozen contract).
    let api = if draft.api.trim().is_empty() {
        entry
            .get("api")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .unwrap_or(DEFAULT_API)
            .to_string()
    } else {
        draft.api.trim().to_string()
    };
    // An unnamed provider is listed by its id, so the file gets the same fallback.
    let name = if draft.name.trim().is_empty() {
        id.to_string()
    } else {
        draft.name.trim().to_string()
    };

    entry.insert("name".to_string(), serde_json::Value::String(name.clone()));
    entry.insert(
        "baseUrl".to_string(),
        serde_json::Value::String(base_url.to_string()),
    );
    entry.insert("api".to_string(), serde_json::Value::String(api.clone()));
    entry.insert(
        "apiKey".to_string(),
        serde_json::Value::String(api_key.to_string()),
    );
    entry.insert(
        "models".to_string(),
        serde_json::to_value(&draft.models).map_err(|e| ProvidersError::Parse(e.to_string()))?,
    );
    providers.insert(id.to_string(), serde_json::Value::Object(entry));

    // The backup is taken before the rewrite, and only when there was a file.
    backup_before_rewrite(path, existed)?;

    write_root(path, &root)?;

    Ok(ProviderSummary {
        id: id.to_string(),
        name,
        base_url: base_url.to_string(),
        api,
        model_count: draft.models.len(),
        // Validated non-empty above, so this is always true on this path.
        has_api_key: true,
    })
}

/// S3: removes `providers.<provider_id>` and nothing else — same backup and
/// atomic write as every other writer of this file.
///
/// An id the file does not have is a no-op (`false`), never an error: deleting is
/// idempotent, exactly like listing never invents an entry. Nothing is rewritten
/// and no backup is taken for a change that did not happen, so the file stays
/// byte-identical. The id shape is validated first, so a malformed id can never
/// reach the file.
pub fn delete_provider(path: &Path, provider_id: &str) -> ProvidersResult<bool> {
    let id = provider_id.trim();
    validate_id(id)?;

    let (mut root, existed) = read_root(path)?;
    let removed = match root.get_mut("providers").and_then(|v| v.as_object_mut()) {
        Some(providers) => providers.remove(id).is_some(),
        // No `providers` object (or one that is not an object) holds nothing
        // to remove.
        None => false,
    };
    if !removed {
        return Ok(false);
    }

    backup_before_rewrite(path, existed)?;
    write_root(path, &root)?;
    Ok(true)
}

/// S2/S3 of `models-by-provider.md`: every provider with its models, the models
/// in the order the file declares them.
///
/// A provider without a `models` array is listed with an empty vector (the
/// editor still shows the provider, the selector simply offers nothing) and the
/// api key stays in the file: `ModelInfo` has no field for it (S4).
///
/// Providers come back sorted by id: serde_json's default map is a `BTreeMap`, so
/// the file's provider order is not kept across the read (that would need the
/// `preserve_order` feature, a Cargo change outside this task). The models array
/// is a JSON array and does keep file order, which is the order the editor and
/// the selector show.
pub fn read_provider_models(path: &Path) -> ProvidersResult<Vec<ProviderModels>> {
    let (root, _) = read_root(path)?;
    let Some(providers) = root.get("providers").and_then(|v| v.as_object()) else {
        return Ok(Vec::new());
    };
    Ok(providers
        .iter()
        .map(|(id, entry)| ProviderModels {
            provider: id.clone(),
            name: provider_name_of(id, entry),
            models: entry
                .get("models")
                .and_then(|v| v.as_array())
                .map(|models| models.iter().filter_map(model_info_of).collect())
                .unwrap_or_default(),
        })
        .collect())
}

/// S3: writes one model inside `providers.<provider>.models` — backup, merge,
/// atomic write.
///
/// Only that one entry changes: an id the provider already lists is merged into
/// (every field the draft does not cover survives — `reasoning`, sampling
/// params, the costs it did not set), a new id is appended in file order, and
/// every other model, provider and top-level key stays as it was. The provider
/// itself must already exist: nothing invents a provider (`UnknownProvider`),
/// exactly like `set_provider_key`.
pub fn update_provider_model(
    path: &Path,
    draft: &ProviderModelDraft,
) -> ProvidersResult<ModelInfo> {
    let provider = draft.provider.trim();
    validate_id(provider)?;
    let id = draft.id.trim();
    validate_model_id(id)?;

    let (mut root, existed) = read_root(path)?;
    let entry = root
        .get_mut("providers")
        .and_then(|v| v.as_object_mut())
        .and_then(|providers| providers.get_mut(provider))
        .and_then(|v| v.as_object_mut())
        .ok_or_else(|| ProvidersError::UnknownProvider(provider.to_string()))?;

    // A provider the file lists without models gets the array it needs; anything
    // other than an array there is a file Nabla must not guess at.
    if !entry.get("models").map(serde_json::Value::is_array).unwrap_or(false) {
        match entry.get("models") {
            None | Some(serde_json::Value::Null) => {
                entry.insert(
                    "models".to_string(),
                    serde_json::Value::Array(Vec::new()),
                );
            }
            Some(_) => {
                return Err(ProvidersError::Parse(format!(
                    "{}: `providers.{provider}.models` is not a JSON array",
                    path.display()
                )));
            }
        }
    }
    let models = entry
        .get_mut("models")
        .and_then(|v| v.as_array_mut())
        .ok_or_else(|| {
            ProvidersError::Parse(format!(
                "{}: `providers.{provider}.models` is not a JSON array",
                path.display()
            ))
        })?;

    let index = models
        .iter()
        .position(|model| model_id_of(model) == Some(id));
    // Merge into the entry the file already has instead of replacing it, so the
    // keys pi owns inside a model are not dropped by an edit (S3).
    let mut model = match index {
        Some(position) => match &models[position] {
            serde_json::Value::Object(map) => serde_json::Value::Object(map.clone()),
            _ => {
                return Err(ProvidersError::Parse(format!(
                    "{}: `providers.{provider}.models[{position}]` is not a JSON object",
                    path.display()
                )));
            }
        },
        None => serde_json::Value::Object(serde_json::Map::new()),
    };
    let object = model
        .as_object_mut()
        .expect("the entry was built as a JSON object above");
    merge_model_draft(object, id, draft).map_err(|message| {
        ProvidersError::Parse(format!("{}: {}: {message}", path.display(), model_ref(provider, id)))
    })?;

    // The answer is the entry as it now stands, so the editor refreshes from what
    // was written instead of from what was sent.
    let info = model_info_of(&model).ok_or_else(|| {
        ProvidersError::Parse(format!(
            "{}: {} could not be read back",
            path.display(),
            model_ref(provider, id)
        ))
    })?;

    match index {
        Some(position) => models[position] = model,
        None => models.push(model),
    }

    backup_before_rewrite(path, existed)?;
    write_root(path, &root)?;
    Ok(info)
}

/// `providers.<provider>.models[<id>]`, the path an error message names.
fn model_ref(provider: &str, id: &str) -> String {
    format!("`providers.{provider}.models[{id}]`")
}

/// S3: merges the draft into one model entry, writing only the fields the draft
/// covers. Everything else the entry already has — `reasoning`, sampling params,
/// the costs Nabla does not model, the numeric fields the draft left as `None` —
/// survives untouched.
///
/// The error is the offending field's name only: the caller owns the file path
/// and builds the message, so the wording never drifts between call sites.
fn merge_model_draft(
    object: &mut serde_json::Map<String, serde_json::Value>,
    id: &str,
    draft: &ProviderModelDraft,
) -> Result<(), &'static str> {
    object.insert("id".to_string(), serde_json::Value::String(id.to_string()));
    // A blank name keeps the one the file has (and falls back to the id, exactly
    // like every other listing), so an edit that only touches the cost never
    // drops the label.
    let name = {
        let trimmed = draft.name.trim();
        if trimmed.is_empty() {
            object
                .get("name")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .unwrap_or(id)
                .to_string()
        } else {
            trimmed.to_string()
        }
    };
    object.insert("name".to_string(), serde_json::Value::String(name));

    if let Some(value) = draft.context_window {
        object.insert("contextWindow".to_string(), serde_json::json!(value));
    }
    if let Some(value) = draft.max_tokens {
        object.insert("maxTokens".to_string(), serde_json::json!(value));
    }
    // The costs live in pi's nested `cost` object, which may already carry values
    // Nabla does not model (`cacheRead`, `cacheWrite`): merge, never replace.
    if draft.input_cost.is_some() || draft.output_cost.is_some() {
        let mut cost = match object.get("cost") {
            None | Some(serde_json::Value::Null) => serde_json::Map::new(),
            Some(serde_json::Value::Object(existing)) => existing.clone(),
            Some(_) => return Err("`cost` is not a JSON object"),
        };
        if let Some(value) = draft.input_cost {
            cost.insert("input".to_string(), serde_json::json!(value));
        }
        if let Some(value) = draft.output_cost {
            cost.insert("output".to_string(), serde_json::json!(value));
        }
        object.insert("cost".to_string(), serde_json::Value::Object(cost));
    }
    // Modalities are pi's `input` array. An empty list means "the form did not
    // send them", so the file keeps what it has instead of losing every modality.
    let modalities: Vec<&str> = draft
        .modalities
        .iter()
        .map(|m| m.trim())
        .filter(|m| !m.is_empty())
        .collect();
    if !modalities.is_empty() {
        object.insert("input".to_string(), serde_json::json!(modalities));
    }
    // L3: disabling is a mark in the file, never a delete.
    object.insert(
        "enabled".to_string(),
        serde_json::Value::Bool(draft.enabled),
    );
    Ok(())
}

/// S3: removes one model from one provider's array — same backup and atomic
/// write as every other writer of this file.
///
/// A provider or model the file does not have is a no-op (`false`), never an
/// error: deleting is idempotent and nothing is rewritten or backed up for a
/// change that did not happen. Both ids are validated first, so a malformed id
/// can never reach the file.
pub fn remove_provider_model(
    path: &Path,
    provider_id: &str,
    model_id: &str,
) -> ProvidersResult<bool> {
    let provider = provider_id.trim();
    validate_id(provider)?;
    let id = model_id.trim();
    validate_model_id(id)?;

    let (mut root, existed) = read_root(path)?;
    let removed = root
        .get_mut("providers")
        .and_then(|v| v.as_object_mut())
        .and_then(|providers| providers.get_mut(provider))
        .and_then(|v| v.as_object_mut())
        .and_then(|entry| entry.get_mut("models"))
        .and_then(|v| v.as_array_mut())
        .map(|models| {
            let before = models.len();
            models.retain(|model| model_id_of(model) != Some(id));
            models.len() != before
        })
        .unwrap_or(false);
    if !removed {
        return Ok(false);
    }

    backup_before_rewrite(path, existed)?;
    write_root(path, &root)?;
    Ok(true)
}

/// S4: rewrites `providers.<provider_id>.apiKey` in place, in plain text (the
/// user's own decision, with the keychain noted as debt).
///
/// Nothing else is created or touched: the entry must already exist (an unknown
/// id is an error, not an invented provider), and every other key of the entry,
/// of the other providers and of the file survives. The answer is the same
/// `ProviderSummary` `list_providers` would report, so the pane refreshes from it
/// without ever seeing the key.
pub fn set_provider_key(
    path: &Path,
    provider_id: &str,
    api_key: &str,
) -> ProvidersResult<ProviderSummary> {
    let id = provider_id.trim();
    validate_id(id)?;
    let api_key = api_key.trim();
    if api_key.is_empty() {
        return Err(ProvidersError::EmptyApiKey);
    }

    let (mut root, existed) = read_root(path)?;
    let entry = root
        .get_mut("providers")
        .and_then(|v| v.as_object_mut())
        .and_then(|providers| providers.get_mut(id))
        .and_then(|v| v.as_object_mut())
        .ok_or_else(|| ProvidersError::UnknownProvider(id.to_string()))?;
    entry.insert(
        "apiKey".to_string(),
        serde_json::Value::String(api_key.to_string()),
    );
    // The summary is built from the entry as it now stands, copy taken so the
    // borrow of `root` ends before the file is rewritten.
    let summary = summary_of(id, &serde_json::Value::Object(entry.clone()));

    backup_before_rewrite(path, existed)?;
    write_root(path, &root)?;
    Ok(summary)
}

/// S3: copies the file next to itself before a rewrite. One helper for every
/// writer, so no path can forget it; a file that did not exist needs no backup.
fn backup_before_rewrite(path: &Path, existed: bool) -> ProvidersResult<()> {
    if !existed {
        return Ok(());
    }
    let backup = backup_path(path)?;
    std::fs::copy(path, &backup)
        .map_err(|e| ProvidersError::Io(format!("backup {}: {e}", backup.display())))?;
    Ok(())
}

/// Serializes and atomically writes the whole root. One helper, so every writer
/// keeps the same formatting and the same crash safety.
fn write_root(path: &Path, root: &serde_json::Value) -> ProvidersResult<()> {
    let serialized = serde_json::to_string_pretty(root)
        .map_err(|e| ProvidersError::Parse(e.to_string()))?;
    write_atomic(path, &serialized)
}

/// Reads `path` as a JSON object, tolerating a missing file. Returns the value
/// plus whether the file existed (which decides the backup).
fn read_root(path: &Path) -> ProvidersResult<(serde_json::Value, bool)> {
    match std::fs::read_to_string(path) {
        Ok(raw) => {
            let value: serde_json::Value = serde_json::from_str(&raw).map_err(|e| {
                ProvidersError::Parse(format!("{}: {e}", path.display()))
            })?;
            if !value.is_object() {
                return Err(ProvidersError::Parse(format!(
                    "{}: root is not a JSON object",
                    path.display()
                )));
            }
            Ok((value, true))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok((serde_json::json!({}), false)),
        Err(e) => Err(ProvidersError::Io(format!("{}: {e}", path.display()))),
    }
}

/// S3: id `[a-z0-9-]{1,32}`.
fn validate_id(id: &str) -> ProvidersResult<()> {
    let valid = !id.is_empty()
        && id.len() <= 32
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if valid {
        Ok(())
    } else {
        Err(ProvidersError::InvalidId(id.to_string()))
    }
}

/// Longest model id accepted; pi's own ids (`deepseek-v4-flash`) are far shorter.
pub const MODEL_ID_MAX: usize = 64;

/// S3: a model id follows the provider discipline — lowercase, bounded, no
/// whitespace — but must admit the separators pi's real ids carry (`qwen3.6`,
/// `qwen3:8b`, `meta-llama/llama-3.2-1b`); rejecting a dot would make most of the
/// models a provider already lists impossible to edit.
fn validate_model_id(id: &str) -> ProvidersResult<()> {
    let valid = !id.is_empty()
        && id.len() <= MODEL_ID_MAX
        && id.chars().any(|c| c.is_ascii_alphanumeric())
        && id.chars().all(|c| {
            c.is_ascii_lowercase()
                || c.is_ascii_digit()
                || matches!(c, '-' | '.' | '_' | '/' | ':')
        });
    if valid {
        Ok(())
    } else {
        Err(ProvidersError::InvalidModelId(id.to_string()))
    }
}

/// S3: the base url is an http(s) endpoint; discovery appends `/models` to it.
fn validate_base_url(base_url: &str) -> ProvidersResult<()> {
    let host = base_url
        .strip_prefix("http://")
        .or_else(|| base_url.strip_prefix("https://"));
    match host {
        Some(rest) if !rest.is_empty() => Ok(()),
        _ => Err(ProvidersError::InvalidBaseUrl(base_url.to_string())),
    }
}

/// `models.json.bak-<millis>` next to the file it copies, so a broken rewrite is
/// always recoverable without leaving the directory.
fn backup_path(path: &Path) -> ProvidersResult<PathBuf> {
    let name = path
        .file_name()
        .ok_or_else(|| ProvidersError::Io(format!("{}: no file name", path.display())))?
        .to_string_lossy()
        .to_string();
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| ProvidersError::Io(e.to_string()))?
        .as_millis();
    Ok(path.with_file_name(format!("{name}.bak-{millis}")))
}

/// S3: writes through a temporary file in the same directory and renames it, so
/// a crash mid-write never leaves a half-written `models.json`.
fn write_atomic(path: &Path, contents: &str) -> ProvidersResult<()> {
    let dir = path
        .parent()
        .ok_or_else(|| ProvidersError::Io(format!("{}: no parent directory", path.display())))?;
    std::fs::create_dir_all(dir)
        .map_err(|e| ProvidersError::Io(format!("{}: {e}", dir.display())))?;
    let name = path
        .file_name()
        .ok_or_else(|| ProvidersError::Io(format!("{}: no file name", path.display())))?
        .to_string_lossy()
        .to_string();
    let tmp = path.with_file_name(format!("{name}.tmp"));
    std::fs::write(&tmp, contents)
        .map_err(|e| ProvidersError::Io(format!("{}: {e}", tmp.display())))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        // Never leave the temporary file behind when the rename fails.
        let _ = std::fs::remove_file(&tmp);
        ProvidersError::Io(format!("{}: {e}", path.display()))
    })
}

/// Display name of a provider entry; an unnamed provider is listed by its id.
fn provider_name_of(id: &str, entry: &serde_json::Value) -> String {
    entry
        .get("name")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .unwrap_or(id)
        .to_string()
}

/// The `id` of a model entry, trimmed; `None` when it has none, which is a model
/// the UI cannot select or edit.
fn model_id_of(model: &serde_json::Value) -> Option<&str> {
    model
        .get("id")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|id| !id.is_empty())
}

/// A whole-number field of a model entry. pi writes integers; a float that names
/// an integer (`131072.0`) is read as one instead of being dropped.
fn model_u64(model: &serde_json::Value, field: &str) -> Option<u64> {
    match model.get(field)? {
        serde_json::Value::Number(number) => number.as_u64().or_else(|| {
            let value = number.as_f64()?;
            let integral = value.is_finite()
                && value >= 0.0
                && value.fract() == 0.0
                && value <= u64::MAX as f64;
            integral.then_some(value as u64)
        }),
        _ => None,
    }
}

/// A per-token cost out of pi's nested `cost` object.
fn model_cost(model: &serde_json::Value, field: &str) -> Option<f64> {
    model
        .get("cost")
        .and_then(|cost| cost.get(field))
        .and_then(serde_json::Value::as_f64)
}

/// pi's `input` array; a model that declares none is text-only, which is pi's
/// own default.
fn model_modalities(model: &serde_json::Value) -> Vec<String> {
    match model.get("input") {
        Some(serde_json::Value::Array(entries)) => entries
            .iter()
            .filter_map(|entry| entry.as_str())
            .map(|entry| entry.trim().to_string())
            .filter(|entry| !entry.is_empty())
            .collect(),
        _ => vec!["text".to_string()],
    }
}

/// One model entry of pi's file as the contract's `ModelInfo`. `None` when the
/// entry is not an object or carries no id: nothing the UI could select.
fn model_info_of(model: &serde_json::Value) -> Option<ModelInfo> {
    if !model.is_object() {
        return None;
    }
    let id = model_id_of(model)?.to_string();
    let name = model
        .get("name")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .unwrap_or(&id)
        .to_string();
    Some(ModelInfo {
        id,
        name,
        context_window: model_u64(model, "contextWindow"),
        max_tokens: model_u64(model, "maxTokens"),
        input_cost: model_cost(model, "input"),
        output_cost: model_cost(model, "output"),
        modalities: model_modalities(model),
        // S3/L3: Nabla's own mark, enabled unless the file says otherwise.
        enabled: model
            .get("enabled")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true),
    })
}

fn summary_of(id: &str, entry: &serde_json::Value) -> ProviderSummary {
    let name = provider_name_of(id, entry);
    let base_url = entry
        .get("baseUrl")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let api = entry
        .get("api")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .unwrap_or(DEFAULT_API)
        .to_string();
    let model_count = entry
        .get("models")
        .and_then(|v| v.as_array())
        .map(|models| models.len())
        .unwrap_or(0);
    // The key itself never leaves this module (S4): only whether it exists.
    let has_api_key = entry
        .get("apiKey")
        .and_then(|v| v.as_str())
        .map(|key| !key.trim().is_empty())
        .unwrap_or(false);
    ProviderSummary {
        id: id.to_string(),
        name,
        base_url,
        api,
        model_count,
        has_api_key,
    }
}

/// S2: `GET {base_url}/models` from Rust, so the api key never reaches the
/// webview and CORS/CSP stay out of the problem.
pub async fn discover_models(
    base_url: &str,
    api_key: &str,
) -> ProvidersResult<Vec<ModelOption>> {
    let endpoint = models_endpoint(base_url);
    let client = reqwest::Client::builder()
        .timeout(MODELS_TIMEOUT)
        .build()
        .map_err(|e| ProvidersError::Http(e.to_string()))?;
    let mut request = client.get(&endpoint).header("Accept", "application/json");
    // A keyless local server gets no header at all; see `bearer_header`.
    if let Some(authorization) = bearer_header(api_key) {
        request = request.header("Authorization", authorization);
    }
    let response = request
        .send()
        .await
        .map_err(|e| ProvidersError::Http(format!("{endpoint}: {e}")))?;
    let status = response.status();
    if !status.is_success() {
        return Err(ProvidersError::Http(http_status_error(status.as_u16())));
    }
    let body = response
        .text()
        .await
        .map_err(|e| ProvidersError::Http(format!("{endpoint}: {e}")))?;
    Ok(parse_models_response(&body))
}

/// S2: the endpoint and the key of a provider that is **already** configured,
/// read from pi's own file server-side.
///
/// This is what lets the settings editor refresh the model list of an existing
/// provider without ever holding its key (S4): the webview sends an id, the key
/// goes from `models.json` into the request and nowhere else. It is never logged
/// and never part of an answer or an error.
///
/// The provider must already exist (`UnknownProvider`, nothing is invented), its
/// id and url are validated before any request (a malformed id or a non-http url
/// never reaches the network), and the key may only be missing when the server
/// is local (see `is_local_endpoint`). The answer is `(base_url, api_key)`, with
/// an empty key for the local case.
pub fn provider_discovery_credentials(
    path: &Path,
    provider_id: &str,
) -> ProvidersResult<(String, String)> {
    let id = provider_id.trim();
    validate_id(id)?;

    let (root, _) = read_root(path)?;
    let entry = root
        .get("providers")
        .and_then(|v| v.as_object())
        .and_then(|providers| providers.get(id))
        .and_then(|v| v.as_object())
        .ok_or_else(|| ProvidersError::UnknownProvider(id.to_string()))?;

    let base_url = entry
        .get("baseUrl")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .unwrap_or_default();
    validate_base_url(base_url)?;

    let api_key = entry
        .get("apiKey")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|key| !key.is_empty());
    match api_key {
        Some(key) => Ok((base_url.to_string(), key.to_string())),
        // A server on this machine is commonly started without a key.
        None if is_local_endpoint(base_url) => Ok((base_url.to_string(), String::new())),
        None => Err(ProvidersError::MissingApiKey(id.to_string())),
    }
}

/// S2: discovery for a provider the file already has, over the same HTTP path,
/// parser and timeout as the create form uses.
pub async fn discover_provider_models(
    path: &Path,
    provider_id: &str,
) -> ProvidersResult<Vec<ModelOption>> {
    let (base_url, api_key) = provider_discovery_credentials(path, provider_id)?;
    discover_models(&base_url, &api_key).await
}

/// The `Authorization` value of a discovery request, or `None` when there is no
/// key to send.
///
/// An empty key is only reachable for a local server, and stripping it there
/// means the request carries no `Authorization` header instead of the malformed
/// `Bearer ` (no token) that strict servers reject.
pub fn bearer_header(api_key: &str) -> Option<String> {
    let key = api_key.trim();
    if key.is_empty() {
        None
    } else {
        Some(format!("Bearer {key}"))
    }
}

/// S2: whether a base url points at the machine itself, the one case where a
/// provider may have no api key at all.
///
/// Loopback means the host is `localhost`, `127.0.0.1` or IPv6 `::1`, with or
/// without a port and with any path: llama.cpp, Ollama, LM Studio and a local
/// vLLM usually start with no auth, and requiring a key there would make a
/// perfectly working provider impossible to refresh. The comparison is on the
/// parsed host, never on a substring, so `localhost.evil.test` and
/// `127.0.0.1.example.com` stay remote and still need their key.
pub fn is_local_endpoint(base_url: &str) -> bool {
    matches!(
        url_host(base_url).as_deref(),
        Some("localhost") | Some("127.0.0.1") | Some("::1")
    )
}

/// Host of an http(s) url, lowercased: userinfo, port, path, query and fragment
/// are dropped, and an IPv6 literal keeps only what is inside the brackets.
fn url_host(base_url: &str) -> Option<String> {
    let lower = base_url.trim().to_ascii_lowercase();
    let rest = lower
        .strip_prefix("http://")
        .or_else(|| lower.strip_prefix("https://"))?;
    let authority = rest.split(['/', '?', '#']).next()?;
    // `user:pass@host` — the host is what follows the last `@`.
    let authority = authority.rsplit('@').next()?;
    let host = match authority.strip_prefix('[') {
        Some(after_bracket) => after_bracket.split(']').next()?,
        None => authority.split(':').next()?,
    };
    (!host.is_empty()).then(|| host.to_string())
}

/// `{base}/models` with trailing slashes removed from the base url.
pub fn models_endpoint(base_url: &str) -> String {
    format!("{}/models", base_url.trim().trim_end_matches('/'))
}

/// Maps an OpenAI-compatible `/models` body onto `[{ id, name }]`.
///
/// Entries without a usable `id` are skipped; a missing `name` falls back to the
/// id, which is what a plain OpenAI-compatible server usually returns.
pub fn parse_models_response(body: &str) -> Vec<ModelOption> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(body) else {
        return Vec::new();
    };
    let Some(data) = value.get("data").and_then(|d| d.as_array()) else {
        return Vec::new();
    };
    data.iter()
        .filter_map(|entry| {
            let id = entry.get("id")?.as_str()?.trim();
            if id.is_empty() {
                return None;
            }
            let name = entry
                .get("name")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .unwrap_or(id);
            Some(ModelOption {
                id: id.to_string(),
                name: name.to_string(),
            })
        })
        .collect()
}

/// Error text for a non-2xx answer, carrying the status so the form can show it.
pub fn http_status_error(status: u16) -> String {
    format!("models endpoint returned HTTP {status}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Scratch directory in the system temp dir (never the worktree), removed
    /// in teardown together with everything written inside it.
    struct TempModelsDir(PathBuf);

    impl TempModelsDir {
        fn new(tag: &str) -> Self {
            let mut path = std::env::temp_dir();
            path.push(format!(
                "nabla_providers_{}_{}_{tag}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&path).expect("create temp dir");
            TempModelsDir(path)
        }

        fn path(&self) -> PathBuf {
            self.0.join("models.json")
        }

        fn write(&self, contents: &str) {
            std::fs::write(self.path(), contents).expect("write fixture");
        }

        fn read(&self) -> String {
            std::fs::read_to_string(self.path()).expect("read models.json")
        }

        /// Every file name in the scratch directory, sorted.
        fn entries(&self) -> Vec<String> {
            let mut names: Vec<String> = std::fs::read_dir(&self.0)
                .expect("read dir")
                .map(|entry| entry.expect("entry").file_name().to_string_lossy().to_string())
                .collect();
            names.sort();
            names
        }
    }

    impl Drop for TempModelsDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn draft(id: &str) -> ProviderDraft {
        ProviderDraft {
            id: id.to_string(),
            name: format!("Provider {id}"),
            base_url: "https://api.example.com/v1".to_string(),
            api: String::new(),
            api_key: "sk-test".to_string(),
            models: vec![ModelOption {
                id: "model-a".to_string(),
                name: "Model A".to_string(),
            }],
        }
    }

    // ── path ────────────────────────────────────────────────────────────────

    // S3: the file is pi's own, resolved from the home directory.
    #[test]
    fn models_path_is_pis_agent_models_json() {
        assert_eq!(
            models_path(Path::new("/home/u")),
            PathBuf::from("/home/u/.pi/agent/models.json")
        );
    }

    // ── reading ─────────────────────────────────────────────────────────────

    #[test]
    fn missing_models_file_reads_as_no_providers() {
        let dir = TempModelsDir::new("missing");
        let providers = read_providers(&dir.path()).expect("a missing file is not an error");
        assert!(providers.is_empty());
    }

    // S1/S3/S4: the pane lists what pi knows, and the key is never handed back.
    #[test]
    fn two_providers_are_summarized_and_the_key_never_leaves() {
        let dir = TempModelsDir::new("two_providers");
        dir.write(
            r#"{
              "providers": {
                "openrouter": {
                  "name": "OpenRouter",
                  "baseUrl": "https://openrouter.ai/api/v1",
                  "api": "openai-completions",
                  "apiKey": "sk-secret-value",
                  "models": [
                    { "id": "a", "name": "A" },
                    { "id": "b", "name": "B" }
                  ]
                },
                "local": {
                  "baseUrl": "http://127.0.0.1:8080/v1",
                  "api": "openai-responses",
                  "apiKey": ""
                }
              }
            }"#,
        );

        let providers = read_providers(&dir.path()).expect("read providers");
        assert_eq!(providers.len(), 2);
        let openrouter = providers
            .iter()
            .find(|p| p.id == "openrouter")
            .expect("openrouter");
        assert_eq!(openrouter.name, "OpenRouter");
        assert_eq!(openrouter.base_url, "https://openrouter.ai/api/v1");
        assert_eq!(openrouter.api, "openai-completions");
        assert_eq!(openrouter.model_count, 2);
        assert!(openrouter.has_api_key);

        // An empty key is not a key, and a missing name falls back to the id.
        let local = providers.iter().find(|p| p.id == "local").expect("local");
        assert_eq!(local.name, "local");
        assert!(!local.has_api_key);
        assert_eq!(local.model_count, 0);
        assert_eq!(local.api, "openai-responses");

        // S4: the secret stays in the file and out of every payload.
        let json = serde_json::to_string(&providers).expect("serialize");
        assert!(
            !json.contains("sk-secret-value"),
            "the api key must never be serialized: {json}"
        );
        assert!(!json.contains("apiKey"), "no apiKey field in the DTO: {json}");
    }

    #[test]
    fn provider_without_api_uses_the_default_one() {
        let dir = TempModelsDir::new("default_api");
        dir.write(r#"{"providers":{"p":{"baseUrl":"https://x.test"}}}"#);
        let providers = read_providers(&dir.path()).expect("read providers");
        assert_eq!(providers[0].api, DEFAULT_API);
    }

    // S3: pi's file also holds keys Nabla does not model; reading must tolerate
    // them instead of rejecting the whole file.
    #[test]
    fn unknown_provider_keys_do_not_break_reading() {
        let dir = TempModelsDir::new("unknown_keys");
        dir.write(
            r#"{"theme":"dark","providers":{"p":{"baseUrl":"https://x.test","apiKey":"k","timeoutSeconds":300,"modelOverrides":{"a":{"x":1}}}}}"#,
        );
        let providers = read_providers(&dir.path()).expect("read providers");
        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].id, "p");
    }

    #[test]
    fn malformed_models_json_is_a_parse_error() {
        let dir = TempModelsDir::new("malformed");
        dir.write("{ not json");
        assert!(matches!(
            read_providers(&dir.path()),
            Err(ProvidersError::Parse(_))
        ));
    }

    // ── writing ─────────────────────────────────────────────────────────────

    // S3: a first provider creates the file (and its directory) from nothing.
    #[test]
    fn save_creates_the_file_and_its_directory_when_absent() {
        let dir = TempModelsDir::new("create");
        let path = dir.0.join("nested").join("models.json");

        let summary = save_provider(&path, &draft("acme")).expect("save provider");

        assert_eq!(summary.id, "acme");
        assert_eq!(summary.name, "Provider acme");
        assert_eq!(summary.base_url, "https://api.example.com/v1");
        assert_eq!(summary.api, DEFAULT_API, "empty api takes the default");
        assert_eq!(summary.model_count, 1);
        assert!(summary.has_api_key);

        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("read"))
                .expect("parse written file");
        let provider = &written["providers"]["acme"];
        assert_eq!(provider["baseUrl"], serde_json::json!("https://api.example.com/v1"));
        assert_eq!(provider["api"], serde_json::json!(DEFAULT_API));
        assert_eq!(provider["apiKey"], serde_json::json!("sk-test"));
        assert_eq!(provider["models"][0]["id"], serde_json::json!("model-a"));
        assert_eq!(provider["models"][0]["name"], serde_json::json!("Model A"));
    }

    // S3: "preservando cualquier otra clave del archivo" — the merge touches
    // `providers.<id>` and nothing else.
    #[test]
    fn save_merges_and_preserves_every_other_key() {
        let dir = TempModelsDir::new("merge");
        dir.write(
            r#"{
              "theme": "dark",
              "providers": {
                "existing": {
                  "baseUrl": "https://existing.test/v1",
                  "api": "openai-completions",
                  "apiKey": "sk-existing",
                  "modelOverrides": { "m": { "contextWindow": 4096 } },
                  "models": [{ "id": "m", "name": "M" }]
                }
              }
            }"#,
        );

        save_provider(&dir.path(), &draft("acme")).expect("save provider");

        let written: serde_json::Value = serde_json::from_str(&dir.read()).expect("parse");
        assert_eq!(written["theme"], serde_json::json!("dark"));
        assert_eq!(
            written["providers"]["existing"]["modelOverrides"]["m"]["contextWindow"],
            serde_json::json!(4096)
        );
        assert_eq!(
            written["providers"]["existing"]["apiKey"],
            serde_json::json!("sk-existing")
        );
        assert_eq!(
            written["providers"]["acme"]["baseUrl"],
            serde_json::json!("https://api.example.com/v1")
        );
    }

    // S3: updating one provider must not drop the keys pi owns inside it.
    #[test]
    fn updating_a_provider_keeps_its_unknown_keys_and_existing_api() {
        let dir = TempModelsDir::new("update");
        dir.write(
            r#"{"providers":{"acme":{"baseUrl":"https://old.test","api":"openai-responses","apiKey":"old","timeoutSeconds":300,"modelOverrides":{"m":{"x":1}}}}}"#,
        );

        save_provider(&dir.path(), &draft("acme")).expect("save provider");

        let written: serde_json::Value = serde_json::from_str(&dir.read()).expect("parse");
        let provider = &written["providers"]["acme"];
        // Empty api keeps the one already in the file.
        assert_eq!(provider["api"], serde_json::json!("openai-responses"));
        assert_eq!(provider["timeoutSeconds"], serde_json::json!(300));
        assert_eq!(provider["modelOverrides"]["m"]["x"], serde_json::json!(1));
        assert_eq!(provider["baseUrl"], serde_json::json!("https://api.example.com/v1"));
    }

    #[test]
    fn an_explicit_api_is_written_over_the_existing_one() {
        let dir = TempModelsDir::new("explicit_api");
        dir.write(r#"{"providers":{"acme":{"baseUrl":"https://old.test","api":"openai-completions","apiKey":"old"}}}"#);
        let mut d = draft("acme");
        d.api = "openai-responses".to_string();

        save_provider(&dir.path(), &d).expect("save provider");

        let written: serde_json::Value = serde_json::from_str(&dir.read()).expect("parse");
        assert_eq!(written["providers"]["acme"]["api"], serde_json::json!("openai-responses"));
    }

    // S3/S4: an existing file is backed up before it is rewritten.
    #[test]
    fn an_existing_file_is_backed_up_before_the_rewrite() {
        let dir = TempModelsDir::new("backup");
        let original = r#"{"providers":{"keep":{"baseUrl":"https://keep.test","apiKey":"k"}}}"#;
        dir.write(original);

        save_provider(&dir.path(), &draft("acme")).expect("save provider");

        let backups: Vec<String> = dir
            .entries()
            .into_iter()
            .filter(|name| name.starts_with("models.json.bak-"))
            .collect();
        assert_eq!(backups.len(), 1, "exactly one backup: {:?}", dir.entries());
        let stamp = backups[0]
            .trim_start_matches("models.json.bak-")
            .to_string();
        assert!(
            stamp.parse::<u128>().map(|ms| ms > 0).unwrap_or(false),
            "the backup name carries millis: {stamp}"
        );
        assert_eq!(
            std::fs::read_to_string(dir.0.join(&backups[0])).expect("read backup"),
            original,
            "the backup holds the pre-save contents"
        );
    }

    #[test]
    fn no_backup_is_created_for_a_file_that_did_not_exist() {
        let dir = TempModelsDir::new("no_backup");
        save_provider(&dir.path(), &draft("acme")).expect("save provider");
        assert_eq!(
            dir.entries(),
            vec!["models.json".to_string()],
            "only the file itself is written"
        );
    }

    // S3: "escritura atómica" leaves no temporary file behind.
    #[test]
    fn save_leaves_no_temporary_file_behind() {
        let dir = TempModelsDir::new("atomic");
        dir.write(r#"{"providers":{}}"#);
        save_provider(&dir.path(), &draft("acme")).expect("save provider");
        let leftovers: Vec<String> = dir
            .entries()
            .into_iter()
            .filter(|name| name.contains(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "left behind: {leftovers:?}");
    }

    // ── validation ──────────────────────────────────────────────────────────

    fn assert_rejected(d: &ProviderDraft) -> ProvidersError {
        let dir = TempModelsDir::new("rejected");
        let original = r#"{"providers":{"keep":{"baseUrl":"https://keep.test","apiKey":"k"}}}"#;
        dir.write(original);

        let err = save_provider(&dir.path(), d).expect_err("invalid draft must be rejected");

        // A rejected draft changes nothing: no write, no backup.
        assert_eq!(dir.read(), original);
        assert_eq!(dir.entries(), vec!["models.json".to_string()]);
        err
    }

    // S3: id `[a-z0-9-]{1,32}`.
    #[test]
    fn invalid_ids_are_rejected() {
        for id in ["", "Acme", "ac me", "ac_me", "acmé", &"a".repeat(33)] {
            let mut d = draft("acme");
            d.id = id.to_string();
            match assert_rejected(&d) {
                ProvidersError::InvalidId(v) => assert_eq!(v, id),
                other => panic!("expected InvalidId for {id:?}, got {other}"),
            }
        }
        // The boundary length is accepted.
        let dir = TempModelsDir::new("id_boundary");
        let mut d = draft("acme");
        d.id = "a".repeat(32);
        save_provider(&dir.path(), &d).expect("32 chars is a valid id");
    }

    // S3: base url must be http(s).
    #[test]
    fn non_http_base_urls_are_rejected() {
        for url in ["", "api.example.com/v1", "ftp://x.test", "file:///etc/passwd"] {
            let mut d = draft("acme");
            d.base_url = url.to_string();
            match assert_rejected(&d) {
                ProvidersError::InvalidBaseUrl(v) => assert_eq!(v, url),
                other => panic!("expected InvalidBaseUrl for {url:?}, got {other}"),
            }
        }
    }

    // S3/S4: an empty key is not a key.
    #[test]
    fn an_empty_api_key_is_rejected() {
        for key in ["", "   "] {
            let mut d = draft("acme");
            d.api_key = key.to_string();
            match assert_rejected(&d) {
                ProvidersError::EmptyApiKey => {}
                other => panic!("expected EmptyApiKey for {key:?}, got {other}"),
            }
        }
    }

    // ── removing and re-keying a provider (S3, S4) ──────────────────────────

    /// Two providers plus a top-level key Nabla does not model, and, inside one
    /// of them, keys pi owns (`timeoutSeconds`, `modelOverrides`).
    const TWO_PROVIDERS: &str = r#"{
      "theme": "dark",
      "providers": {
        "keep": {
          "name": "Keep",
          "baseUrl": "https://keep.test/v1",
          "api": "openai-completions",
          "apiKey": "sk-keep",
          "timeoutSeconds": 300,
          "modelOverrides": { "m": { "contextWindow": 4096 } },
          "models": [{ "id": "m", "name": "M" }]
        },
        "drop": {
          "name": "Drop",
          "baseUrl": "https://drop.test/v1",
          "api": "openai-completions",
          "apiKey": "sk-drop",
          "models": [{ "id": "d", "name": "D" }]
        }
      }
    }"#;

    /// The backups written next to the file, in name order.
    fn backups(dir: &TempModelsDir) -> Vec<String> {
        dir.entries()
            .into_iter()
            .filter(|name| name.starts_with("models.json.bak-"))
            .collect()
    }

    // S3: deleting one provider touches `providers.<id>` only — the other
    // provider (with the keys pi owns inside it), every top-level key and the
    // pre-delete backup are all exactly what the contract promises.
    #[test]
    fn delete_removes_only_that_provider_and_writes_a_backup() {
        let dir = TempModelsDir::new("delete_one");
        dir.write(TWO_PROVIDERS);

        assert!(delete_provider(&dir.path(), "drop").expect("delete provider"));

        let written: serde_json::Value = serde_json::from_str(&dir.read()).expect("parse");
        let providers = written["providers"].as_object().expect("providers object");
        assert_eq!(providers.len(), 1, "only the named provider is gone");
        assert!(providers.contains_key("keep"));
        let original: serde_json::Value =
            serde_json::from_str(TWO_PROVIDERS).expect("parse fixture");
        assert_eq!(
            written["providers"]["keep"], original["providers"]["keep"],
            "the other provider is untouched, unknown keys included"
        );
        assert_eq!(
            written["theme"],
            serde_json::json!("dark"),
            "top-level keys stay"
        );

        let written_backups = backups(&dir);
        assert_eq!(
            written_backups.len(),
            1,
            "exactly one backup: {:?}",
            dir.entries()
        );
        assert_eq!(
            std::fs::read_to_string(dir.0.join(&written_backups[0])).expect("read backup"),
            TWO_PROVIDERS,
            "the backup holds the pre-delete contents"
        );
        // S3: the atomic write leaves no temporary file behind.
        assert!(
            !dir.entries().iter().any(|name| name.contains(".tmp")),
            "left behind: {:?}",
            dir.entries()
        );
        // And pi would read exactly one provider next time.
        assert_eq!(read_providers(&dir.path()).expect("read").len(), 1);
    }

    // S3: an id the file does not have is a no-op — no rewrite, no backup, the
    // file byte-identical.
    #[test]
    fn delete_of_an_unknown_id_leaves_the_file_byte_identical() {
        let dir = TempModelsDir::new("delete_unknown");
        dir.write(TWO_PROVIDERS);
        let before = dir.read();

        assert!(!delete_provider(&dir.path(), "absent").expect("a no-op delete"));

        assert_eq!(dir.read(), before, "nothing is rewritten");
        assert_eq!(
            dir.entries(),
            vec!["models.json".to_string()],
            "no backup for a rewrite that did not happen"
        );
    }

    // A file that does not exist has nothing to delete, and deleting must not
    // create one.
    #[test]
    fn delete_of_a_missing_file_creates_nothing() {
        let dir = TempModelsDir::new("delete_missing");
        assert!(!delete_provider(&dir.path(), "absent").expect("no-op"));
        assert_eq!(dir.entries(), Vec::<String>::new(), "nothing is written");
    }

    // S3: the id shape is validated before the file is touched, so a malformed
    // id can never reach a write.
    #[test]
    fn delete_rejects_an_invalid_id_without_touching_the_file() {
        let dir = TempModelsDir::new("delete_bad_id");
        dir.write(TWO_PROVIDERS);
        let before = dir.read();
        for id in ["", "Drop", "drop me", "../drop", &"d".repeat(33)] {
            let err = delete_provider(&dir.path(), id).expect_err("a malformed id is rejected");
            match err {
                ProvidersError::InvalidId(value) => assert_eq!(value, id),
                other => panic!("expected InvalidId for {id:?}, got {other:?}"),
            }
            assert_eq!(dir.read(), before, "a rejected id changes nothing: {id:?}");
            assert_eq!(dir.entries(), vec!["models.json".to_string()]);
        }
    }

    // S4: the key of one provider is rewritten in place; every other value of the
    // file stays exactly as it was, and the summary reports the new state without
    // ever carrying the key.
    #[test]
    fn set_provider_key_updates_only_that_provider() {
        let dir = TempModelsDir::new("set_key");
        dir.write(TWO_PROVIDERS);

        let summary = set_provider_key(&dir.path(), "keep", "sk-new").expect("set key");

        assert_eq!(summary.id, "keep");
        assert_eq!(summary.name, "Keep");
        assert_eq!(summary.base_url, "https://keep.test/v1");
        assert_eq!(summary.model_count, 1);
        assert!(
            summary.has_api_key,
            "the answer reports the key it just wrote"
        );

        let written: serde_json::Value = serde_json::from_str(&dir.read()).expect("parse");
        assert_eq!(
            written["providers"]["keep"]["apiKey"],
            serde_json::json!("sk-new")
        );
        // Everything else survived: the keys pi owns inside the entry, the other
        // provider and the top-level settings.
        assert_eq!(
            written["providers"]["keep"]["timeoutSeconds"],
            serde_json::json!(300)
        );
        assert_eq!(
            written["providers"]["keep"]["modelOverrides"]["m"]["contextWindow"],
            serde_json::json!(4096)
        );
        assert_eq!(
            written["providers"]["keep"]["baseUrl"],
            serde_json::json!("https://keep.test/v1")
        );
        let original: serde_json::Value =
            serde_json::from_str(TWO_PROVIDERS).expect("parse fixture");
        assert_eq!(written["providers"]["drop"], original["providers"]["drop"]);
        assert_eq!(written["theme"], serde_json::json!("dark"));

        let written_backups = backups(&dir);
        assert_eq!(
            written_backups.len(),
            1,
            "the rewrite is backed up: {:?}",
            dir.entries()
        );
        assert_eq!(
            std::fs::read_to_string(dir.0.join(&written_backups[0])).expect("read backup"),
            TWO_PROVIDERS
        );

        // The pane reads the result without ever seeing the key.
        let providers = read_providers(&dir.path()).expect("read providers");
        assert!(providers.iter().all(|provider| provider.has_api_key));
        let json = serde_json::to_string(&providers).expect("serialize");
        assert!(
            !json.contains("sk-new"),
            "the key never reaches the payload: {json}"
        );
    }

    // S4: an empty key is not a key, and a rejected write changes nothing.
    #[test]
    fn set_provider_key_rejects_an_empty_key() {
        let dir = TempModelsDir::new("set_key_empty");
        dir.write(TWO_PROVIDERS);
        let before = dir.read();
        for key in ["", "   "] {
            let err = set_provider_key(&dir.path(), "keep", key)
                .expect_err("an empty key is rejected");
            match err {
                ProvidersError::EmptyApiKey => {}
                other => panic!("expected EmptyApiKey for {key:?}, got {other:?}"),
            }
            assert_eq!(dir.read(), before, "a rejected key changes nothing");
            assert_eq!(
                dir.entries(),
                vec!["models.json".to_string()],
                "no backup for a rejected write"
            );
        }
    }

    // S3/S4: a key belongs to an entry that exists; an id the file does not have
    // is an error, and no provider is invented.
    #[test]
    fn set_provider_key_rejects_an_unknown_provider() {
        let dir = TempModelsDir::new("set_key_unknown");
        dir.write(TWO_PROVIDERS);
        let before = dir.read();

        let err = set_provider_key(&dir.path(), "absent", "sk-new")
            .expect_err("an unknown provider cannot be keyed");
        match err {
            ProvidersError::UnknownProvider(id) => assert_eq!(id, "absent"),
            other => panic!("expected UnknownProvider, got {other:?}"),
        }
        assert_eq!(dir.read(), before, "the file is not rewritten");
        assert_eq!(
            dir.entries(),
            vec!["models.json".to_string()],
            "no provider is created"
        );

        // An entry that is not an object is not a provider to key either.
        dir.write(r#"{"providers":{"weird":"nope"}}"#);
        let err = set_provider_key(&dir.path(), "weird", "sk-new")
            .expect_err("a non-object entry is not a provider");
        match err {
            ProvidersError::UnknownProvider(id) => assert_eq!(id, "weird"),
            other => panic!("expected UnknownProvider, got {other:?}"),
        }
    }

    // S3: the id shape is validated before a key is written.
    #[test]
    fn set_provider_key_rejects_an_invalid_id() {
        let dir = TempModelsDir::new("set_key_bad_id");
        dir.write(TWO_PROVIDERS);
        let before = dir.read();
        for id in ["", "Keep", "keep me", &"k".repeat(33)] {
            let err = set_provider_key(&dir.path(), id, "sk-new")
                .expect_err("a malformed id is rejected");
            match err {
                ProvidersError::InvalidId(value) => assert_eq!(value, id),
                other => panic!("expected InvalidId for {id:?}, got {other:?}"),
            }
            assert_eq!(dir.read(), before, "a rejected id changes nothing");
            assert_eq!(dir.entries(), vec!["models.json".to_string()]);
        }
    }

    // ── models of one provider (S2, S3 of `models-by-provider.md`) ──────────

    /// A pi-shaped file: one provider whose models carry the fields pi
    /// understands, one provider with no `models` key at all.
    const PI_FILE: &str = r#"{
      "theme": "dark",
      "providers": {
        "nan": {
          "name": "NaN",
          "baseUrl": "https://nan.test/v1",
          "api": "openai-completions",
          "apiKey": "sk-nan-secret",
          "timeoutSeconds": 300,
          "modelOverrides": { "qwen3.6": { "contextWindow": 4096 } },
          "models": [
            {
              "id": "qwen3.6",
              "name": "Qwen 3.6",
              "contextWindow": 131072,
              "maxTokens": 8192,
              "cost": { "input": 0.5, "output": 1.5, "cacheRead": 0.1 },
              "input": ["text", "image"],
              "reasoning": true,
              "temperature": 0.7
            },
            {
              "id": "glm5.3",
              "name": "GLM 5.3",
              "contextWindow": 200000,
              "cost": { "input": 1.0, "output": 2.0 },
              "input": ["text"]
            }
          ]
        },
        "empty": {
          "name": "Empty",
          "baseUrl": "https://empty.test/v1",
          "apiKey": "sk-empty"
        }
      }
    }"#;

    /// A draft that changes nothing by itself: every optional field is `None`
    /// ("keep what the file has") and the model stays enabled.
    fn model_draft(provider: &str, id: &str) -> ProviderModelDraft {
        ProviderModelDraft {
            provider: provider.to_string(),
            id: id.to_string(),
            name: String::new(),
            context_window: None,
            max_tokens: None,
            input_cost: None,
            output_cost: None,
            modalities: Vec::new(),
            enabled: true,
        }
    }

    /// The file as parsed JSON, for asserting what a write produced.
    fn written(dir: &TempModelsDir) -> serde_json::Value {
        serde_json::from_str(&dir.read()).expect("parse written models.json")
    }

    /// The model ids of one provider, in file order.
    fn model_ids(root: &serde_json::Value, provider: &str) -> Vec<String> {
        root["providers"][provider]["models"]
            .as_array()
            .expect("models array")
            .iter()
            .map(|model| model["id"].as_str().expect("model id").to_string())
            .collect()
    }

    // S2/S3: every provider comes back with its models in file order, pi's field
    // names are mapped onto the contract, and the api key never leaves the file.
    #[test]
    fn provider_models_are_listed_with_pi_fields_mapped() {
        let dir = TempModelsDir::new("list_provider_models");
        dir.write(PI_FILE);

        let providers = read_provider_models(&dir.path()).expect("list provider models");
        assert_eq!(
            providers
                .iter()
                .map(|p| p.provider.as_str())
                .collect::<Vec<_>>(),
            vec!["empty", "nan"],
            "every provider of the file is listed"
        );

        let nan = providers.iter().find(|p| p.provider == "nan").expect("nan");
        assert_eq!(nan.name, "NaN");
        assert_eq!(
            nan.models
                .iter()
                .map(|m| m.id.as_str())
                .collect::<Vec<_>>(),
            vec!["qwen3.6", "glm5.3"],
            "models keep the order the file declares"
        );
        assert_eq!(
            nan.models[0],
            ModelInfo {
                id: "qwen3.6".to_string(),
                name: "Qwen 3.6".to_string(),
                context_window: Some(131072),
                max_tokens: Some(8192),
                input_cost: Some(0.5),
                output_cost: Some(1.5),
                modalities: vec!["text".to_string(), "image".to_string()],
                enabled: true,
            }
        );
        assert_eq!(nan.models[1].id, "glm5.3");
        assert_eq!(nan.models[1].context_window, Some(200000));
        assert_eq!(nan.models[1].max_tokens, None);
        assert_eq!(nan.models[1].input_cost, Some(1.0));
        assert_eq!(nan.models[1].output_cost, Some(2.0));

        // A provider without a `models` array is listed with an empty list.
        let empty = providers.iter().find(|p| p.provider == "empty").expect("empty");
        assert_eq!(empty.name, "Empty");
        assert!(empty.models.is_empty());

        // S4: the key is in the file and in none of the payloads.
        let json = serde_json::to_string(&providers).expect("serialize");
        assert!(!json.contains("sk-nan-secret"), "no api key: {json}");
        assert!(!json.contains("apiKey"), "no apiKey field: {json}");
    }

    // S3: a model that declares none of the editable fields still reads, with the
    // contract's defaults (enabled, text-only input, unnamed model = its id).
    #[test]
    fn a_bare_model_gets_the_documented_defaults() {
        let dir = TempModelsDir::new("bare_model");
        dir.write(r#"{"providers":{"p":{"models":[{"id":"bare"},{"id":"off","enabled":false}]}}}"#);

        let providers = read_provider_models(&dir.path()).expect("list");
        assert_eq!(
            providers[0].models[0],
            ModelInfo {
                id: "bare".to_string(),
                name: "bare".to_string(),
                context_window: None,
                max_tokens: None,
                input_cost: None,
                output_cost: None,
                modalities: vec!["text".to_string()],
                enabled: true,
            }
        );
        // The Nabla-side flag is read back when the file carries it.
        assert!(!providers[0].models[1].enabled);
    }

    #[test]
    fn a_missing_file_lists_no_provider_models() {
        let dir = TempModelsDir::new("list_missing");
        let providers = read_provider_models(&dir.path()).expect("a missing file is not an error");
        assert!(providers.is_empty());
    }

    // S3: a new model is appended to its provider and everything else — the other
    // models, the keys pi owns inside the provider, the other provider and the
    // top-level settings — survives byte for byte.
    #[test]
    fn update_creates_a_new_model_and_preserves_the_rest_of_the_provider() {
        let dir = TempModelsDir::new("update_create");
        dir.write(PI_FILE);

        let mut draft = model_draft("nan", "hy4");
        draft.name = "Hy4".to_string();
        draft.context_window = Some(32768);
        draft.max_tokens = Some(4096);
        draft.input_cost = Some(0.25);
        draft.output_cost = Some(0.75);
        draft.modalities = vec!["text".to_string(), "image".to_string()];

        let info = update_provider_model(&dir.path(), &draft).expect("update model");
        assert_eq!(
            info,
            ModelInfo {
                id: "hy4".to_string(),
                name: "Hy4".to_string(),
                context_window: Some(32768),
                max_tokens: Some(4096),
                input_cost: Some(0.25),
                output_cost: Some(0.75),
                modalities: vec!["text".to_string(), "image".to_string()],
                enabled: true,
            }
        );

        let root = written(&dir);
        assert_eq!(model_ids(&root, "nan"), vec!["qwen3.6", "glm5.3", "hy4"]);
        let created = &root["providers"]["nan"]["models"][2];
        assert_eq!(created["id"], serde_json::json!("hy4"));
        assert_eq!(created["name"], serde_json::json!("Hy4"));
        assert_eq!(created["contextWindow"], serde_json::json!(32768));
        assert_eq!(created["maxTokens"], serde_json::json!(4096));
        assert_eq!(created["cost"]["input"], serde_json::json!(0.25));
        assert_eq!(created["cost"]["output"], serde_json::json!(0.75));
        assert_eq!(created["input"], serde_json::json!(["text", "image"]));
        assert_eq!(created["enabled"], serde_json::json!(true));

        // The rest of the file is exactly what it was.
        let original: serde_json::Value = serde_json::from_str(PI_FILE).expect("parse fixture");
        let mut expected = original.clone();
        expected["providers"]["nan"]["models"]
            .as_array_mut()
            .expect("array")
            .push(created.clone());
        assert_eq!(root, expected, "only the new model entry was added");

        assert_eq!(backups(&dir).len(), 1, "the rewrite is backed up");
        assert_eq!(
            std::fs::read_to_string(dir.0.join(&backups(&dir)[0])).expect("read backup"),
            PI_FILE
        );
        assert!(
            !dir.entries().iter().any(|name| name.contains(".tmp")),
            "left behind: {:?}",
            dir.entries()
        );
    }

    // S3: "preserving every other field of that model" — an update writes only
    // the fields the draft covers; reasoning, sampling params, the other costs and
    // the untouched numeric fields stay as pi has them.
    #[test]
    fn update_preserves_unknown_model_fields_and_only_changes_what_it_covers() {
        let dir = TempModelsDir::new("update_merge");
        dir.write(PI_FILE);

        let mut draft = model_draft("nan", "qwen3.6");
        draft.name = "Qwen 3.6 Turbo".to_string();
        draft.context_window = Some(65536);
        draft.input_cost = Some(1.25);
        draft.modalities = vec!["text".to_string()];
        draft.enabled = false;

        let info = update_provider_model(&dir.path(), &draft).expect("update model");
        assert_eq!(info.name, "Qwen 3.6 Turbo");
        assert_eq!(info.context_window, Some(65536));
        // `None` means "keep": the file's values are reported back unchanged.
        assert_eq!(info.max_tokens, Some(8192));
        assert_eq!(info.output_cost, Some(1.5));
        assert!(!info.enabled);

        let root = written(&dir);
        let model = &root["providers"]["nan"]["models"][0];
        assert_eq!(model["id"], serde_json::json!("qwen3.6"));
        assert_eq!(model["name"], serde_json::json!("Qwen 3.6 Turbo"));
        assert_eq!(model["contextWindow"], serde_json::json!(65536));
        assert_eq!(model["maxTokens"], serde_json::json!(8192));
        assert_eq!(model["cost"]["input"], serde_json::json!(1.25));
        assert_eq!(model["cost"]["output"], serde_json::json!(1.5));
        assert_eq!(model["cost"]["cacheRead"], serde_json::json!(0.1));
        assert_eq!(model["input"], serde_json::json!(["text"]));
        // Fields Nabla does not model survive the merge.
        assert_eq!(model["reasoning"], serde_json::json!(true));
        assert_eq!(model["temperature"], serde_json::json!(0.7));
        // Nothing else in the file moved.
        let original: serde_json::Value = serde_json::from_str(PI_FILE).expect("parse fixture");
        assert_eq!(
            root["providers"]["nan"]["models"][1],
            original["providers"]["nan"]["models"][1]
        );
        assert_eq!(root["providers"]["nan"]["timeoutSeconds"], serde_json::json!(300));
        assert_eq!(
            root["providers"]["nan"]["modelOverrides"]["qwen3.6"]["contextWindow"],
            serde_json::json!(4096)
        );
        assert_eq!(root["providers"]["nan"]["apiKey"], serde_json::json!("sk-nan-secret"));
        assert_eq!(root["theme"], serde_json::json!("dark"));
    }

    // L3/S3: disabling is a mark, never a delete — the entry keeps its settings.
    #[test]
    fn update_writes_enabled_false_without_deleting_the_model() {
        let dir = TempModelsDir::new("disable_model");
        dir.write(r#"{"providers":{"nan":{"models":[{"id":"qwen3.6","name":"Qwen 3.6","contextWindow":131072}]}}}"#);

        let mut draft = model_draft("nan", "qwen3.6");
        draft.enabled = false;
        let info = update_provider_model(&dir.path(), &draft).expect("disable model");

        assert!(!info.enabled);
        let root = written(&dir);
        assert_eq!(model_ids(&root, "nan"), vec!["qwen3.6"], "nothing is dropped");
        let model = &root["providers"]["nan"]["models"][0];
        assert_eq!(model["enabled"], serde_json::json!(false));
        assert_eq!(model["name"], serde_json::json!("Qwen 3.6"));
        assert_eq!(model["contextWindow"], serde_json::json!(131072));

        // And the editor reads the flag back.
        let providers = read_provider_models(&dir.path()).expect("list");
        assert_eq!(providers[0].models.len(), 1);
        assert!(!providers[0].models[0].enabled);
    }

    // S3: removal drops one model from one provider, with the same backup and
    // atomic write as every other writer of this file.
    #[test]
    fn remove_drops_only_that_model_and_writes_a_backup() {
        let dir = TempModelsDir::new("remove_model");
        dir.write(PI_FILE);

        assert!(remove_provider_model(&dir.path(), "nan", "qwen3.6").expect("remove"));

        let root = written(&dir);
        assert_eq!(model_ids(&root, "nan"), vec!["glm5.3"], "only one model is gone");
        let original: serde_json::Value = serde_json::from_str(PI_FILE).expect("parse fixture");
        assert_eq!(
            root["providers"]["nan"]["models"][0],
            original["providers"]["nan"]["models"][1],
            "the surviving model is untouched"
        );
        assert_eq!(root["providers"]["empty"], original["providers"]["empty"]);
        assert_eq!(root["providers"]["nan"]["apiKey"], serde_json::json!("sk-nan-secret"));
        assert_eq!(root["theme"], serde_json::json!("dark"));

        let written_backups = backups(&dir);
        assert_eq!(written_backups.len(), 1);
        assert_eq!(
            std::fs::read_to_string(dir.0.join(&written_backups[0])).expect("read backup"),
            PI_FILE
        );
        assert!(
            !dir.entries().iter().any(|name| name.contains(".tmp")),
            "left behind: {:?}",
            dir.entries()
        );
    }

    // S3: removing what the file does not have is a no-op, never an error and
    // never a rewrite.
    #[test]
    fn remove_of_an_unknown_model_or_provider_is_a_no_op() {
        let dir = TempModelsDir::new("remove_unknown");
        dir.write(PI_FILE);
        let before = dir.read();

        assert!(!remove_provider_model(&dir.path(), "nan", "absent").expect("no-op"));
        assert!(!remove_provider_model(&dir.path(), "empty", "qwen3.6").expect("no-op"));
        assert!(!remove_provider_model(&dir.path(), "absent", "qwen3.6").expect("no-op"));

        assert_eq!(dir.read(), before, "nothing is rewritten");
        assert_eq!(dir.entries(), vec!["models.json".to_string()], "no backup");
    }

    // S3: "nunca tocar providers no presentes en models.json" — an update for a
    // provider the file does not have is an error, and no provider is invented.
    #[test]
    fn update_of_a_provider_absent_from_the_file_is_rejected() {
        let dir = TempModelsDir::new("update_unknown_provider");
        dir.write(PI_FILE);
        let before = dir.read();

        let err = update_provider_model(&dir.path(), &model_draft("absent", "qwen3.6"))
            .expect_err("an unknown provider cannot receive a model");
        match err {
            ProvidersError::UnknownProvider(id) => assert_eq!(id, "absent"),
            other => panic!("expected UnknownProvider, got {other:?}"),
        }
        assert_eq!(dir.read(), before, "the file is not rewritten");
        assert_eq!(dir.entries(), vec!["models.json".to_string()], "no backup");
        assert!(
            written(&dir)["providers"].get("absent").is_none(),
            "no provider is created"
        );

        // A missing file has no provider either, and none is created.
        let missing = TempModelsDir::new("update_missing_file");
        assert!(matches!(
            update_provider_model(&missing.path(), &model_draft("absent", "qwen3.6")),
            Err(ProvidersError::UnknownProvider(_))
        ));
        assert_eq!(missing.entries(), Vec::<String>::new(), "nothing is written");
    }

    // S3: both ids are validated before the file is touched, so a malformed id
    // can never reach a write.
    #[test]
    fn invalid_provider_and_model_ids_are_rejected_without_touching_the_file() {
        let dir = TempModelsDir::new("model_bad_id");
        dir.write(PI_FILE);
        let before = dir.read();

        for id in ["", "NaN", "na n", "nan/nested", &"n".repeat(33)] {
            let err = update_provider_model(&dir.path(), &model_draft(id, "qwen3.6"))
                .expect_err("a malformed provider id is rejected");
            match err {
                ProvidersError::InvalidId(value) => assert_eq!(value, id),
                other => panic!("expected InvalidId for {id:?}, got {other:?}"),
            }
        }

        for id in [
            "",
            "Qwen 3.6",
            "qwen 3.6",
            "qwen3.6; rm -rf /",
            "mo\u{301}del",
            &"q".repeat(65),
        ] {
            let err = update_provider_model(&dir.path(), &model_draft("nan", id))
                .expect_err("a malformed model id is rejected");
            match err {
                ProvidersError::InvalidModelId(value) => assert_eq!(value, id),
                other => panic!("expected InvalidModelId for {id:?}, got {other:?}"),
            }
            let err = remove_provider_model(&dir.path(), "nan", id)
                .expect_err("a malformed model id is rejected");
            match err {
                ProvidersError::InvalidModelId(value) => assert_eq!(value, id),
                other => panic!("expected InvalidModelId for {id:?}, got {other:?}"),
            }
        }

        assert_eq!(dir.read(), before, "a rejected id changes nothing");
        assert_eq!(dir.entries(), vec!["models.json".to_string()], "no backup");
    }

    // S3: the ids pi's own models carry (`qwen3.6`, `glm5.3`, `qwen3:8b`, a
    // namespaced `meta-llama/...`) must be editable, so the model id rule admits
    // the separators the provider id rule does not need.
    #[test]
    fn model_ids_pi_really_uses_are_accepted() {
        let dir = TempModelsDir::new("real_model_ids");
        dir.write(PI_FILE);
        let ids = [
            "qwen3.6",
            "glm5.3-flash",
            "deepseek-v4-pro",
            "qwen3:8b",
            "meta-llama/llama-3.2-1b",
            &"a".repeat(64),
        ];
        for id in ids {
            update_provider_model(&dir.path(), &model_draft("nan", id))
                .unwrap_or_else(|err| panic!("model id {id:?} must be accepted: {err}"));
        }

        let listed = read_provider_models(&dir.path()).expect("list");
        let nan = listed.iter().find(|p| p.provider == "nan").expect("nan");
        let listed_ids: Vec<&str> = nan.models.iter().map(|m| m.id.as_str()).collect();
        for id in ids {
            assert!(listed_ids.contains(&id), "{id:?} is missing: {listed_ids:?}");
        }
    }

    // S3: a surrounding whitespace is trimmed, exactly like a provider id, so
    // the same model written by hand and by the form is one entry.
    #[test]
    fn a_model_id_is_trimmed_before_it_is_written() {
        let dir = TempModelsDir::new("trim_model_id");
        dir.write(PI_FILE);

        let info = update_provider_model(&dir.path(), &model_draft("nan", "  qwen3.6\n"))
            .expect("surrounding whitespace is trimmed");

        assert_eq!(info.id, "qwen3.6");
        assert_eq!(model_ids(&written(&dir), "nan"), vec!["qwen3.6", "glm5.3"]);
    }

    // Frozen contract: camelCase keys, mirrored field by field in TS.
    #[test]
    fn provider_model_payloads_serialize_camel_case() {
        let provider = ProviderModels {
            provider: "nan".to_string(),
            name: "NaN".to_string(),
            models: vec![ModelInfo {
                id: "qwen3.6".to_string(),
                name: "Qwen 3.6".to_string(),
                context_window: Some(131072),
                max_tokens: Some(8192),
                input_cost: Some(0.5),
                output_cost: Some(1.5),
                modalities: vec!["text".to_string()],
                enabled: false,
            }],
        };
        let json = serde_json::to_value(&provider).expect("serialize");
        assert_eq!(json["provider"], serde_json::json!("nan"));
        assert_eq!(json["name"], serde_json::json!("NaN"));
        let model = &json["models"][0];
        assert_eq!(model["id"], serde_json::json!("qwen3.6"));
        assert_eq!(model["contextWindow"], serde_json::json!(131072));
        assert_eq!(model["maxTokens"], serde_json::json!(8192));
        assert_eq!(model["inputCost"], serde_json::json!(0.5));
        assert_eq!(model["outputCost"], serde_json::json!(1.5));
        assert_eq!(model["modalities"], serde_json::json!(["text"]));
        assert_eq!(model["enabled"], serde_json::json!(false));
        assert!(model.get("context_window").is_none(), "camelCase only");

        let draft: ProviderModelDraft = serde_json::from_value(serde_json::json!({
            "provider": "nan",
            "id": "qwen3.6",
            "name": "Qwen 3.6",
            "contextWindow": 131072,
            "maxTokens": 8192,
            "inputCost": 0.5,
            "outputCost": 1.5,
            "modalities": ["text", "image"],
            "enabled": false
        }))
        .expect("deserialize draft");
        assert_eq!(draft.provider, "nan");
        assert_eq!(draft.context_window, Some(131072));
        assert_eq!(draft.max_tokens, Some(8192));
        assert_eq!(draft.input_cost, Some(0.5));
        assert_eq!(draft.output_cost, Some(1.5));
        assert_eq!(draft.modalities, vec!["text", "image"]);
        assert!(!draft.enabled);

        // A payload that omits the optional halves still builds a draft: absent
        // numbers mean "keep" and an absent flag means "enabled", the same
        // default the read side applies.
        let minimal: ProviderModelDraft = serde_json::from_value(serde_json::json!({
            "provider": "nan",
            "id": "qwen3.6",
            "name": "Qwen 3.6"
        }))
        .expect("deserialize minimal draft");
        assert_eq!(minimal.context_window, None);
        assert_eq!(minimal.input_cost, None);
        assert!(minimal.modalities.is_empty());
        assert!(minimal.enabled);
    }

    // ── discovery ───────────────────────────────────────────────────────────

    // S2: `GET {baseUrl}/models`, tolerating a base url with trailing slashes.
    #[test]
    fn models_endpoint_appends_models_and_trims_trailing_slashes() {
        assert_eq!(
            models_endpoint("https://api.example.com/v1"),
            "https://api.example.com/v1/models"
        );
        assert_eq!(
            models_endpoint("https://api.example.com/v1///"),
            "https://api.example.com/v1/models"
        );
        assert_eq!(
            models_endpoint("https://api.example.com/v1/"),
            "https://api.example.com/v1/models"
        );
    }

    // S2: the OpenAI-compatible list maps onto `{ id, name }`; entries without
    // an id are skipped and a missing name falls back to the id.
    #[test]
    fn parse_models_response_maps_ids_and_names() {
        let body = r#"{
          "object": "list",
          "data": [
            { "id": "gpt-4o", "object": "model", "created": 1715367049, "owned_by": "system" },
            { "id": "qwen3.6-35b", "name": "Qwen3.6 35B", "object": "model" },
            { "id": "nameless" },
            { "object": "model" },
            { "id": 42 },
            { "id": "" },
            "not-an-object"
          ]
        }"#;

        let models = parse_models_response(body);

        assert_eq!(
            models,
            vec![
                ModelOption {
                    id: "gpt-4o".to_string(),
                    name: "gpt-4o".to_string(),
                },
                ModelOption {
                    id: "qwen3.6-35b".to_string(),
                    name: "Qwen3.6 35B".to_string(),
                },
                ModelOption {
                    id: "nameless".to_string(),
                    name: "nameless".to_string(),
                },
            ]
        );
    }

    #[test]
    fn a_body_that_is_not_a_model_list_parses_as_empty() {
        assert!(parse_models_response("not json").is_empty());
        assert!(parse_models_response(r#"{"error":{"message":"nope"}}"#).is_empty());
        assert!(parse_models_response(r#"{"data":"nope"}"#).is_empty());
    }

    // ── discovery of an already-configured provider (S2, S4) ────────────────

    /// A file with one remote provider (key padded with whitespace, as a human
    /// pasting it may leave it), one provider pointing at a local server with no
    /// key, one with a whitespace-only key and one with no `apiKey` key at all.
    const DISCOVERY_FILE: &str = r#"{
      "providers": {
        "nan": {
          "baseUrl": "https://nan.test/v1",
          "apiKey": "  sk-nan-secret  "
        },
        "local": {
          "baseUrl": "http://127.0.0.1:8080/v1",
          "apiKey": ""
        },
        "blank": {
          "baseUrl": "https://blank.test/v1",
          "apiKey": "   "
        },
        "nokey": {
          "baseUrl": "https://nokey.test/v1"
        }
      }
    }"#;

    // S2/S4: the endpoint and the key of an existing provider are read from pi's
    // own file, server-side, so the editor never has to hold the key to refresh
    // a model list.
    #[test]
    fn discovery_credentials_come_from_pis_file_trimmed() {
        let dir = TempModelsDir::new("discovery_credentials");
        dir.write(DISCOVERY_FILE);

        assert_eq!(
            provider_discovery_credentials(&dir.path(), "nan").expect("credentials"),
            (
                "https://nan.test/v1".to_string(),
                "sk-nan-secret".to_string()
            ),
            "the url and the key are read back trimmed"
        );

        // The id is trimmed exactly like every other reader and writer of the
        // file, so a stray newline from the UI is not an unknown provider.
        assert_eq!(
            provider_discovery_credentials(&dir.path(), " nan\n")
                .expect("a padded id still finds the provider")
                .0,
            "https://nan.test/v1"
        );
    }

    // S2: discovering with an id the file does not have is an error that names
    // it; nothing is invented and nothing is written.
    #[test]
    fn discovery_of_an_unknown_provider_is_rejected() {
        let dir = TempModelsDir::new("discovery_unknown");
        dir.write(DISCOVERY_FILE);
        let before = dir.read();

        let err = provider_discovery_credentials(&dir.path(), "absent")
            .expect_err("an unknown provider cannot be discovered");
        match &err {
            ProvidersError::UnknownProvider(id) => assert_eq!(id, "absent"),
            other => panic!("expected UnknownProvider, got {other:?}"),
        }
        assert_eq!(
            err.to_string(),
            "unknown provider: absent",
            "the editor shows this verbatim"
        );
        assert_eq!(dir.read(), before, "a failed lookup writes nothing");
        assert_eq!(dir.entries(), vec!["models.json".to_string()], "no backup");

        // An entry that is not an object is not a provider either.
        dir.write(r#"{"providers":{"weird":"nope"}}"#);
        let before = dir.read();
        match provider_discovery_credentials(&dir.path(), "weird").expect_err("not a provider") {
            ProvidersError::UnknownProvider(id) => assert_eq!(id, "weird"),
            other => panic!("expected UnknownProvider, got {other:?}"),
        }
        assert_eq!(dir.read(), before, "a failed lookup writes nothing");

        // A file that was never written has no provider, and none is created.
        let missing = TempModelsDir::new("discovery_missing_file");
        assert!(matches!(
            provider_discovery_credentials(&missing.path(), "absent"),
            Err(ProvidersError::UnknownProvider(_))
        ));
        assert_eq!(missing.entries(), Vec::<String>::new(), "nothing is written");
    }

    // S2/S4: a remote provider without a key cannot be discovered, and the error
    // names the provider without ever carrying the key it did not have.
    #[test]
    fn discovery_without_an_api_key_is_rejected() {
        let dir = TempModelsDir::new("discovery_no_key");
        dir.write(DISCOVERY_FILE);
        let before = dir.read();

        for id in ["blank", "nokey"] {
            let err = provider_discovery_credentials(&dir.path(), id)
                .expect_err("a remote provider needs a key");
            match &err {
                ProvidersError::MissingApiKey(value) => assert_eq!(value, id),
                other => panic!("expected MissingApiKey for {id:?}, got {other:?}"),
            }
            assert_eq!(
                err.to_string(),
                format!("provider {id} has no api key configured")
            );
        }

        assert_eq!(dir.read(), before, "a rejected lookup writes nothing");
        assert_eq!(dir.entries(), vec!["models.json".to_string()]);
    }

    // S2: a server on this machine commonly runs without a key (llama.cpp,
    // Ollama, LM Studio, a local vLLM), so an empty key is only an error for a
    // remote endpoint. The rule is the host, never a substring of the url.
    #[test]
    fn a_local_provider_needs_no_api_key() {
        let dir = TempModelsDir::new("discovery_local");
        dir.write(DISCOVERY_FILE);

        assert_eq!(
            provider_discovery_credentials(&dir.path(), "local")
                .expect("a local server needs no key"),
            ("http://127.0.0.1:8080/v1".to_string(), String::new()),
            "discovery proceeds with no key at all, not with a blank one"
        );

        for url in [
            "http://localhost:11434/v1",
            "https://localhost/v1",
            "http://127.0.0.1:1234/v1",
            "http://127.0.0.1",
            "http://[::1]:8080/v1",
            "HTTP://LOCALHOST:8080/v1",
        ] {
            assert!(is_local_endpoint(url), "{url} is loopback");
        }
        for url in [
            "https://api.example.com/v1",
            "http://192.168.1.10:8080/v1",
            "http://localhost.evil.test/v1",
            "http://127.0.0.1.example.com/v1",
            "http://notlocalhost/v1",
            "api.example.com/v1",
        ] {
            assert!(!is_local_endpoint(url), "{url} is remote");
        }
    }

    // S2/S3: both inputs the file supplies are validated before any request, so a
    // malformed id or a non-http url never reaches the network.
    #[test]
    fn discovery_rejects_a_malformed_id_or_a_non_http_base_url() {
        let dir = TempModelsDir::new("discovery_bad_input");
        dir.write(DISCOVERY_FILE);

        for id in ["", "NaN", "nan me", "nan/nested", &"n".repeat(33)] {
            match provider_discovery_credentials(&dir.path(), id)
                .expect_err("a malformed id is rejected")
            {
                ProvidersError::InvalidId(value) => assert_eq!(value, id),
                other => panic!("expected InvalidId for {id:?}, got {other:?}"),
            }
        }

        let urls = TempModelsDir::new("discovery_bad_url");
        urls.write(
            r#"{"providers":{
              "p":{"baseUrl":"ftp://x.test","apiKey":"k"},
              "q":{"apiKey":"k"},
              "r":{"baseUrl":"   ","apiKey":"k"},
              "s":{"baseUrl":"file:///etc/passwd","apiKey":"k"}
            }}"#,
        );
        for id in ["p", "q", "r", "s"] {
            match provider_discovery_credentials(&urls.path(), id)
                .expect_err("only http(s) is discoverable")
            {
                ProvidersError::InvalidBaseUrl(_) => {}
                other => panic!("expected InvalidBaseUrl for {id:?}, got {other:?}"),
            }
        }
    }

    // S4: the key stays inside Rust. Every error the editor can show is checked,
    // including the ones a failed discovery returns.
    #[test]
    fn the_api_key_never_appears_in_an_error_or_a_payload() {
        let dir = TempModelsDir::new("discovery_no_leak");
        dir.write(DISCOVERY_FILE);
        let secret = "sk-nan-secret";

        for id in ["absent", "blank", "nokey"] {
            let message = provider_discovery_credentials(&dir.path(), id)
                .expect_err("no credentials")
                .to_string();
            assert!(!message.contains(secret), "the key leaked: {message}");
        }

        // A successful lookup hands the key to the HTTP call and nothing else:
        // the payload the editor receives has no field for it.
        let (base_url, api_key) =
            provider_discovery_credentials(&dir.path(), "nan").expect("credentials");
        assert_eq!(api_key, secret);
        assert!(!base_url.contains(secret));
        let json = serde_json::to_string(&vec![ModelOption {
            id: "m".to_string(),
            name: "M".to_string(),
        }])
        .expect("serialize");
        assert!(!json.contains(secret));
        assert!(!json.contains("apiKey"), "no apiKey field: {json}");
    }

    // S2: a keyless local server must receive no `Authorization` header at all;
    // `Bearer ` with an empty token is a malformed header some servers reject.
    #[test]
    fn the_bearer_header_is_omitted_when_there_is_no_key() {
        assert_eq!(
            bearer_header("  sk-abc\n"),
            Some("Bearer sk-abc".to_string())
        );
        assert_eq!(bearer_header(""), None);
        assert_eq!(bearer_header("   "), None);
    }

    // S2: a non-2xx answer must surface the status.
    #[test]
    fn a_non_2xx_status_is_reported_with_its_code() {
        let message = http_status_error(401);
        assert!(message.contains("401"), "got: {message}");
        assert!(message.contains("models"), "got: {message}");
        assert_ne!(http_status_error(401), http_status_error(500));
    }

    // ── contract shape ──────────────────────────────────────────────────────

    // Frozen contract: camelCase keys, mirrored field by field in TS.
    #[test]
    fn summaries_and_models_serialize_camel_case() {
        let summary = ProviderSummary {
            id: "acme".to_string(),
            name: "Acme".to_string(),
            base_url: "https://api.example.com".to_string(),
            api: DEFAULT_API.to_string(),
            model_count: 3,
            has_api_key: true,
        };
        let json = serde_json::to_value(&summary).expect("serialize");
        assert_eq!(json["baseUrl"], serde_json::json!("https://api.example.com"));
        assert_eq!(json["modelCount"], serde_json::json!(3));
        assert_eq!(json["hasApiKey"], serde_json::json!(true));

        let option = ModelOption {
            id: "m".to_string(),
            name: "M".to_string(),
        };
        assert_eq!(
            serde_json::to_value(&option).expect("serialize"),
            serde_json::json!({ "id": "m", "name": "M" })
        );

        // The draft reads back from a camelCase payload.
        let draft: ProviderDraft = serde_json::from_value(serde_json::json!({
            "id": "acme",
            "name": "Acme",
            "baseUrl": "https://api.example.com",
            "api": "openai-completions",
            "apiKey": "sk-test",
            "models": [{ "id": "m", "name": "M" }]
        }))
        .expect("deserialize draft");
        assert_eq!(draft.base_url, "https://api.example.com");
        assert_eq!(draft.api_key, "sk-test");
        assert_eq!(draft.models.len(), 1);
    }
}
