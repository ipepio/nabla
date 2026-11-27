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

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelOption {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_cost: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_cost: Option<f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modalities: Vec<String>,
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

/// `ModelInfo.source` for an entry of `~/.pi/agent/models.json`: the file the
/// human edits, so the UI may offer to change it.
pub const MODEL_SOURCE_CONFIGURED: &str = "configured";

/// `ModelInfo.source` for an entry of `~/.pi/agent/models-store.json`: pi's cached
/// catalog for an already signed-in provider. Read-only — pi owns that file and
/// rewrites it on its own schedule, so nothing here edits or deletes it.
pub const MODEL_SOURCE_CATALOG: &str = "catalog";

/// S3: one model of a provider, as the editor reads it. Pi's own field names
/// (`contextWindow`, `maxTokens`, `cost.input`, `cost.output`, `input`) are
/// mapped onto flat camelCase fields; `enabled` is Nabla's mark, not pi's, and
/// defaults to `true` when the file does not carry it.
///
/// `source` says which of pi's two files the entry came from
/// (`MODEL_SOURCE_CONFIGURED` or `MODEL_SOURCE_CATALOG`), which is what decides
/// whether the UI may edit it.
///
/// `hidden` is Nabla's own mark, from `Settings::hidden_models`: the catalogue
/// itself is read-only, so hiding is a preference stored here (keyed on
/// `"<provider>/<modelId>"`) and never an edit of pi's files.
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
    /// Whether the human hid this model from the selector. A hidden model is
    /// still listed and still described in full: only the choice disappears.
    #[serde(default)]
    pub hidden: bool,
    /// `"configured"` or `"catalog"`; see `MODEL_SOURCE_CONFIGURED`.
    pub source: String,
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
            ProvidersError::InvalidId(id) => {
                write!(f, "provider id must match ^[a-z0-9-]{{1,32}}$: {id:?}")
            }
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

/// Path of pi's cached provider catalog inside a home directory.
///
/// pi writes it after a successful sign-in or catalog refresh: it is the only
/// place the models of a signed-in provider (openai, openai-codex) exist. Nabla
/// reads it and never writes it.
pub fn models_store_path(home_dir: &Path) -> PathBuf {
    home_dir.join(".pi").join("agent").join("models-store.json")
}

/// Path of pi's credentials file inside a home directory.
///
/// pi writes it when the human signs in to a provider (an oauth token object or a
/// provider api key, one entry per provider id). It is the evidence that a
/// catalog-only provider is usable, and Nabla only ever reads it: no token is
/// returned, logged or written from here.
pub fn auth_path(home_dir: &Path) -> PathBuf {
    home_dir.join(".pi").join("agent").join("auth.json")
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
        ProvidersError::Parse(format!(
            "{}: `providers` is not a JSON object",
            path.display()
        ))
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
    let model_entries: Vec<serde_json::Value> = draft
        .models
        .iter()
        .map(model_option_to_pi_json)
        .collect();
    entry.insert("models".to_string(), serde_json::Value::Array(model_entries));
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
/// This is the configured half only; `read_provider_models_with_catalog` is what
/// the command layer calls when pi's cached catalog should be listed too.
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

/// The union of what pi has configured and what pi has cached, limited to the
/// providers that are usable: a provider whose models only live in the catalog
/// (a signed-in openai or openai-codex) still lists them, and the models of
/// `models.json` are listed exactly as before.
///
/// Usability of a catalog-only provider (S1 of `model-visibility.md`):
/// `models-store.json` caches the catalog of **every** provider pi has ever
/// fetched, signed in or not (openai-codex is cached today with no credentials),
/// so the cache alone is not evidence of a usable provider. A provider only the
/// catalog carries is listed only when `auth.json` has an entry for it whose
/// value is a non-empty object; a provider `models.json` already lists is
/// exposed regardless, because that file is the human's own configuration. A
/// cached-but-unauthenticated provider does not appear at all — not even as an
/// empty provider.
///
/// Merge rules:
/// - a provider in both files is listed once, with its configured models first
///   and then the catalog models whose id the configured list does not already
///   have — an id is never duplicated and the configured entry is the one that
///   is kept when both files carry the same id;
/// - a provider only the catalog has is appended with the provider id as its
///   name (the catalog carries no display name) and every entry marked
///   `MODEL_SOURCE_CATALOG`;
/// - only conversation models of the catalog are listed (`type: "chat"`, or no
///   `type` at all in an older cache); a `type: "classifier"` entry is not a
///   choice the selector can offer.
///
/// The catalog and the credentials file are read-only and never a reason to fail:
/// a missing, unreadable or corrupt `models-store.json` contributes no catalog
/// entries, and a missing or corrupt `auth.json` authenticates nothing, while the
/// configured providers are still listed. A broken `models.json` stays a parse
/// error, exactly like `read_provider_models` reports it.
pub fn read_provider_models_with_catalog(
    models: &Path,
    store: &Path,
    auth: &Path,
) -> ProvidersResult<Vec<ProviderModels>> {
    let mut providers = read_provider_models(models)?;
    let Some(catalog) = read_catalog(store) else {
        return Ok(providers);
    };
    let credentials = read_credentials(auth);

    // serde_json's default map is a `BTreeMap`, so the catalog providers are
    // appended in id order: the listing is deterministic across reads.
    for (provider_id, entry) in catalog {
        let catalog_models = catalog_models_of(entry.get("models"));
        if catalog_models.is_empty() {
            continue;
        }
        match providers
            .iter_mut()
            .find(|provider| provider.provider == provider_id)
        {
            Some(configured) => {
                for model in catalog_models {
                    if configured.models.iter().any(|known| known.id == model.id) {
                        continue;
                    }
                    configured.models.push(model);
                }
            }
            None => {
                // A cached catalogue without credentials is not a provider the
                // human can use, so it is not listed at all.
                if !is_authenticated(&credentials, &provider_id) {
                    continue;
                }
                providers.push(ProviderModels {
                    provider: provider_id.clone(),
                    // pi's store has no display name for a provider, and inventing
                    // one ("OpenAI Codex") would be a label pi never uses. The id is
                    // the same fallback `models.json` gets for an unnamed entry.
                    name: provider_id,
                    models: catalog_models,
                });
            }
        }
    }
    Ok(providers)
}

/// S2 of `model-visibility.md`: applies the human's hidden list to a listing.
///
/// This is Nabla's own preference, applied on read: a model whose
/// `"<provider>/<modelId>"` pair (see `settings::hidden_model_key`) is in the
/// list is marked `hidden`, for a configured and a catalog entry alike. Nothing
/// is removed or edited: the entry keeps every field it had, and a pair for a
/// model that is no longer listed marks nothing.
pub fn apply_hidden_models(providers: &mut [ProviderModels], hidden: &[String]) {
    if hidden.is_empty() {
        return;
    }
    for provider in providers.iter_mut() {
        for model in provider.models.iter_mut() {
            model.hidden = hidden.iter().any(|entry| {
                entry == &crate::settings::hidden_model_key(&provider.provider, &model.id)
            });
        }
    }
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
    if !entry
        .get("models")
        .map(serde_json::Value::is_array)
        .unwrap_or(false)
    {
        match entry.get("models") {
            None | Some(serde_json::Value::Null) => {
                entry.insert("models".to_string(), serde_json::Value::Array(Vec::new()));
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
        ProvidersError::Parse(format!(
            "{}: {}: {message}",
            path.display(),
            model_ref(provider, id)
        ))
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
    let serialized =
        serde_json::to_string_pretty(root).map_err(|e| ProvidersError::Parse(e.to_string()))?;
    write_atomic(path, &serialized)
}

/// Reads `path` as a JSON object, tolerating a missing file. Returns the value
/// plus whether the file existed (which decides the backup).
fn read_root(path: &Path) -> ProvidersResult<(serde_json::Value, bool)> {
    match std::fs::read_to_string(path) {
        Ok(raw) => {
            let value: serde_json::Value = serde_json::from_str(&raw)
                .map_err(|e| ProvidersError::Parse(format!("{}: {e}", path.display())))?;
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
            c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '.' | '_' | '/' | ':')
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
    model_info_of_source(model, MODEL_SOURCE_CONFIGURED)
}

/// The same mapping for both files Nabla reads: a configured entry of
/// `models.json` and a cached entry of `models-store.json` carry the same field
/// names (`contextWindow`, `maxTokens`, `cost`, `input`), so one mapper serves
/// both and the only difference is where the entry came from.
///
/// A catalog entry is always enabled: pi owns it, so there is nothing on this
/// side to disable.
fn model_info_of_source(model: &serde_json::Value, source: &str) -> Option<ModelInfo> {
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
    // S3/L3: Nabla's own mark, enabled unless the configured file says otherwise.
    let enabled = source == MODEL_SOURCE_CATALOG
        || model
            .get("enabled")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true);
    Some(ModelInfo {
        id,
        name,
        context_window: model_u64(model, "contextWindow"),
        max_tokens: model_u64(model, "maxTokens"),
        input_cost: model_cost(model, "input"),
        output_cost: model_cost(model, "output"),
        modalities: model_modalities(model),
        enabled,
        // Neither file knows about hiding: it is Nabla's own mark, applied by
        // `apply_hidden_models` from the settings once the listing is built.
        hidden: false,
        source: source.to_string(),
    })
}

/// pi's cached catalog as a JSON object, or `None` for every failure a read of it
/// may hit (missing file, unreadable file, corrupt JSON, a root that is not an
/// object). The catalog is a cache pi rebuilds by itself, so none of those is an
/// error here: they only mean there is nothing extra to list.
fn read_catalog(store: &Path) -> Option<serde_json::Map<String, serde_json::Value>> {
    let raw = std::fs::read_to_string(store).ok()?;
    match serde_json::from_str::<serde_json::Value>(&raw).ok()? {
        serde_json::Value::Object(catalog) => Some(catalog),
        _ => None,
    }
}

/// pi's credentials file as a JSON object, tolerating every failure a read of it
/// may hit.
///
/// Only the **shape** of an entry is ever inspected here (`is_authenticated`); no
/// token, key or expiry leaves this function. A missing, unreadable or corrupt
/// `auth.json` reads as "nothing is signed in", which is the safe direction: it
/// hides catalog-only providers instead of inventing usable ones.
fn read_credentials(auth: &Path) -> serde_json::Map<String, serde_json::Value> {
    let Some(raw) = std::fs::read_to_string(auth).ok() else {
        return serde_json::Map::new();
    };
    match serde_json::from_str::<serde_json::Value>(&raw) {
        Ok(serde_json::Value::Object(entries)) => entries,
        _ => serde_json::Map::new(),
    }
}

/// Whether `auth.json` holds usable credentials for one provider: an entry whose
/// value is a non-empty object.
///
/// pi writes one object per signed-in provider (oauth tokens, or an api key) and
/// removes the entry on sign-out. An empty object, a scalar or an array is not a
/// sign-in, so it never makes a cached catalog usable.
fn is_authenticated(
    credentials: &serde_json::Map<String, serde_json::Value>,
    provider_id: &str,
) -> bool {
    match credentials.get(provider_id) {
        Some(serde_json::Value::Object(entry)) => !entry.is_empty(),
        _ => false,
    }
}

/// The conversation models of one cached provider, mapped onto the contract.
///
/// `models` is the raw `models` field of the entry: a missing array, an entry
/// that is not an array and an entry that is not an object all contribute
/// nothing instead of failing the read.
fn catalog_models_of(models: Option<&serde_json::Value>) -> Vec<ModelInfo> {
    models
        .and_then(serde_json::Value::as_array)
        .map(|models| {
            models
                .iter()
                .filter(|model| is_chat_entry(model))
                .filter_map(|model| model_info_of_source(model, MODEL_SOURCE_CATALOG))
                .collect()
        })
        .unwrap_or_default()
}

/// Whether a cached entry is a conversation model. pi's store also carries
/// non-chat kinds (`type: "classifier"`), which are not choices the selector can
/// offer; a cache without a `type` field predates the distinction and is kept.
fn is_chat_entry(model: &serde_json::Value) -> bool {
    match model.get("type") {
        None | Some(serde_json::Value::Null) => true,
        Some(serde_json::Value::String(kind)) => kind.trim() == "chat",
        Some(_) => false,
    }
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
pub async fn discover_models(base_url: &str, api_key: &str) -> ProvidersResult<Vec<ModelOption>> {
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
fn model_option_to_pi_json(opt: &ModelOption) -> serde_json::Value {
    let mut obj = serde_json::Map::new();
    obj.insert("id".to_string(), serde_json::json!(opt.id));
    obj.insert("name".to_string(), serde_json::json!(opt.name));
    if let Some(ctx) = opt.context_window {
        obj.insert("contextWindow".to_string(), serde_json::json!(ctx));
    }
    if let Some(max_tok) = opt.max_tokens {
        obj.insert("maxTokens".to_string(), serde_json::json!(max_tok));
    }
    if !opt.modalities.is_empty() {
        obj.insert("input".to_string(), serde_json::json!(opt.modalities));
    }
    if opt.input_cost.is_some() || opt.output_cost.is_some() {
        let mut cost = serde_json::Map::new();
        if let Some(ic) = opt.input_cost {
            cost.insert("input".to_string(), serde_json::json!(ic));
        }
        if let Some(oc) = opt.output_cost {
            cost.insert("output".to_string(), serde_json::json!(oc));
        }
        obj.insert("cost".to_string(), serde_json::Value::Object(cost));
    }
    serde_json::Value::Object(obj)
}

fn json_positive_u64(v: &serde_json::Value) -> Option<u64> {
    v.as_u64()
        .or_else(|| v.as_str().and_then(|s| s.trim().parse::<u64>().ok()))
        .filter(|&n| n > 0)
}

fn json_non_neg_f64(v: &serde_json::Value) -> Option<f64> {
    v.as_f64()
        .or_else(|| v.as_str().and_then(|s| s.trim().parse::<f64>().ok()))
        .filter(|n| n.is_finite() && *n >= 0.0)
}

fn parse_discovered_context_window(entry: &serde_json::Value) -> Option<u64> {
    for key in [
        "contextWindow",
        "context_window",
        "context_length",
        "max_model_len",
        "max_context_length",
        "input_token_limit",
    ] {
        if let Some(n) = entry.get(key).and_then(json_positive_u64) {
            return Some(n);
        }
    }
    if let Some(n) = entry
        .get("top_provider")
        .and_then(|tp| tp.get("context_length"))
        .and_then(json_positive_u64)
    {
        return Some(n);
    }
    if let Some(n) = entry
        .get("limits")
        .and_then(|l| l.get("context").or_else(|| l.get("input")))
        .and_then(json_positive_u64)
    {
        return Some(n);
    }
    None
}

fn parse_discovered_max_tokens(entry: &serde_json::Value) -> Option<u64> {
    for key in [
        "maxTokens",
        "max_tokens",
        "max_output_tokens",
        "max_completion_tokens",
        "output_token_limit",
    ] {
        if let Some(n) = entry.get(key).and_then(json_positive_u64) {
            return Some(n);
        }
    }
    if let Some(n) = entry
        .get("top_provider")
        .and_then(|tp| {
            tp.get("max_completion_tokens")
                .or_else(|| tp.get("max_output_tokens"))
        })
        .and_then(json_positive_u64)
    {
        return Some(n);
    }
    if let Some(n) = entry
        .get("limits")
        .and_then(|l| l.get("output").or_else(|| l.get("max_tokens")))
        .and_then(json_positive_u64)
    {
        return Some(n);
    }
    None
}

fn parse_discovered_modalities(entry: &serde_json::Value) -> Vec<String> {
    let mut has_text = false;
    let mut has_image = false;

    let mut absorb_list = |arr: &[serde_json::Value]| {
        for item in arr {
            if let Some(raw) = item.as_str() {
                match raw.trim().to_ascii_lowercase().as_str() {
                    "text" | "chat" => has_text = true,
                    "image" | "vision" => has_image = true,
                    _ => {}
                }
            }
        }
    };

    for arr in [
        entry.get("input").and_then(|v| v.as_array()),
        entry.get("modalities").and_then(|v| v.as_array()),
        entry.get("input_modalities").and_then(|v| v.as_array()),
        entry
            .get("architecture")
            .and_then(|a| a.get("input_modalities"))
            .and_then(|v| v.as_array()),
    ]
    .into_iter()
    .flatten()
    {
        absorb_list(arr);
    }

    if let Some(modality_str) = entry
        .get("architecture")
        .and_then(|a| a.get("modality"))
        .and_then(|v| v.as_str())
    {
        let input_side = modality_str
            .split("->")
            .next()
            .unwrap_or(modality_str)
            .to_ascii_lowercase();
        if input_side.contains("text") {
            has_text = true;
        }
        if input_side.contains("image") || input_side.contains("vision") {
            has_image = true;
        }
    }

    if let Some(caps) = entry.get("capabilities").and_then(|v| v.as_object()) {
        if caps.get("vision").and_then(|v| v.as_bool()) == Some(true)
            || caps.get("image").and_then(|v| v.as_bool()) == Some(true)
        {
            has_image = true;
            has_text = true;
        }
        if caps.get("chat").and_then(|v| v.as_bool()) == Some(true)
            || caps.get("text").and_then(|v| v.as_bool()) == Some(true)
        {
            has_text = true;
        }
    }
    if entry.get("supports_vision").and_then(|v| v.as_bool()) == Some(true)
        || entry.get("vision").and_then(|v| v.as_bool()) == Some(true)
    {
        has_image = true;
        has_text = true;
    }

    let mut out = Vec::new();
    if has_text {
        out.push("text".to_string());
    }
    if has_image {
        out.push("image".to_string());
    }
    out
}

fn parse_discovered_costs(entry: &serde_json::Value) -> (Option<f64>, Option<f64>) {
    // 1. Native per-million-token `cost: { input, output }`.
    if let Some(cost) = entry.get("cost").and_then(|v| v.as_object()) {
        let input = cost.get("input").and_then(json_non_neg_f64);
        let output = cost.get("output").and_then(json_non_neg_f64);
        if input.is_some() || output.is_some() {
            return (input, output);
        }
    }
    // 2. OpenRouter / OpenAI-compatible `pricing: { prompt, completion }` (USD per single token).
    if let Some(pricing) = entry.get("pricing").and_then(|v| v.as_object()) {
        let scale = |v: f64| ((v * 1_000_000.0) * 10_000.0).round() / 10_000.0;
        let input = pricing.get("prompt").and_then(json_non_neg_f64).map(scale);
        let output = pricing
            .get("completion")
            .and_then(json_non_neg_f64)
            .map(scale);
        if input.is_some() || output.is_some() {
            return (input, output);
        }
    }
    (None, None)
}

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
            let (input_cost, output_cost) = parse_discovered_costs(entry);
            Some(ModelOption {
                id: id.to_string(),
                name: name.to_string(),
                context_window: parse_discovered_context_window(entry),
                max_tokens: parse_discovered_max_tokens(entry),
                input_cost,
                output_cost,
                modalities: parse_discovered_modalities(entry),
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

        /// pi's cached catalog, next to `models.json` exactly as pi writes it.
        fn store_path(&self) -> PathBuf {
            self.0.join("models-store.json")
        }

        fn write_store(&self, contents: &str) {
            std::fs::write(self.store_path(), contents).expect("write models-store.json");
        }

        /// pi's credentials file, next to `models.json` exactly as pi writes it.
        fn auth_path(&self) -> PathBuf {
            self.0.join("auth.json")
        }

        fn write_auth(&self, contents: &str) {
            std::fs::write(self.auth_path(), contents).expect("write auth.json");
        }

        /// Every file name in the scratch directory, sorted.
        fn entries(&self) -> Vec<String> {
            let mut names: Vec<String> = std::fs::read_dir(&self.0)
                .expect("read dir")
                .map(|entry| {
                    entry
                        .expect("entry")
                        .file_name()
                        .to_string_lossy()
                        .to_string()
                })
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
                ..Default::default()
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
        assert!(
            !json.contains("apiKey"),
            "no apiKey field in the DTO: {json}"
        );
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
        assert_eq!(
            provider["baseUrl"],
            serde_json::json!("https://api.example.com/v1")
        );
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
        assert_eq!(
            provider["baseUrl"],
            serde_json::json!("https://api.example.com/v1")
        );
    }

    #[test]
    fn an_explicit_api_is_written_over_the_existing_one() {
        let dir = TempModelsDir::new("explicit_api");
        dir.write(r#"{"providers":{"acme":{"baseUrl":"https://old.test","api":"openai-completions","apiKey":"old"}}}"#);
        let mut d = draft("acme");
        d.api = "openai-responses".to_string();

        save_provider(&dir.path(), &d).expect("save provider");

        let written: serde_json::Value = serde_json::from_str(&dir.read()).expect("parse");
        assert_eq!(
            written["providers"]["acme"]["api"],
            serde_json::json!("openai-responses")
        );
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
        for url in [
            "",
            "api.example.com/v1",
            "ftp://x.test",
            "file:///etc/passwd",
        ] {
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
            let err =
                set_provider_key(&dir.path(), "keep", key).expect_err("an empty key is rejected");
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
            nan.models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
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
                hidden: false,
                source: MODEL_SOURCE_CONFIGURED.to_string(),
            }
        );
        assert_eq!(nan.models[1].id, "glm5.3");
        assert_eq!(nan.models[1].context_window, Some(200000));
        assert_eq!(nan.models[1].max_tokens, None);
        assert_eq!(nan.models[1].input_cost, Some(1.0));
        assert_eq!(nan.models[1].output_cost, Some(2.0));

        // A provider without a `models` array is listed with an empty list.
        let empty = providers
            .iter()
            .find(|p| p.provider == "empty")
            .expect("empty");
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
                hidden: false,
                source: MODEL_SOURCE_CONFIGURED.to_string(),
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

    // ── the cached catalog (`models-store.json`) ────────────────────────────

    /// `models.json` with one provider, so every catalog test starts from a
    /// configured provider that must survive the union untouched.
    const CONFIGURED_FILE: &str = r#"{
      "providers": {
        "keep": {
          "name": "Keep",
          "baseUrl": "https://keep.test/v1",
          "apiKey": "sk-keep-secret",
          "models": [
            { "id": "alpha", "name": "Alpha (configured)", "contextWindow": 8192 }
          ]
        }
      }
    }"#;

    /// The union read: the configured file, the catalog and pi's credentials
    /// file next to them.
    fn union(dir: &TempModelsDir) -> Vec<ProviderModels> {
        read_provider_models_with_catalog(&dir.path(), &dir.store_path(), &dir.auth_path())
            .expect("the union never fails for a broken catalog")
    }

    /// The model ids of one listed provider, in payload order.
    fn listed_ids(providers: &[ProviderModels], provider: &str) -> Vec<String> {
        providers
            .iter()
            .find(|p| p.provider == provider)
            .unwrap_or_else(|| panic!("provider {provider} is not listed"))
            .models
            .iter()
            .map(|m| m.id.clone())
            .collect()
    }

    /// The provider ids of a listing, in payload order.
    fn listed_providers(providers: &[ProviderModels]) -> Vec<&str> {
        providers.iter().map(|p| p.provider.as_str()).collect()
    }

    /// pi's credentials file with one signed-in provider: the per-provider object
    /// pi writes. Only its presence as a non-empty object matters here, so no real
    /// token appears in the fixture.
    const AUTH_OPENAI: &str = r#"{"openai":{"type":"oauth","access":"token-value","refresh":"token-value","expires":1794769407}}"#;

    /// The signed-in openai-codex entry the tests authenticate against the same
    /// way pi writes it, plus the two chat models the cache carries.
    const AUTH_OPENAI_CODEX: &str = r#"{"openai-codex":{"type":"oauth","access":"token-value","refresh":"token-value","expires":1794769407}}"#;

    /// A cached openai-codex catalogue, exactly the shape pi caches whether or
    /// not the human ever signed in to it.
    const CODEX_STORE: &str = r#"{
      "openai-codex": {
        "models": [
          { "id": "gpt-5.6-luna", "name": "GPT-5.6 Luna", "type": "chat" },
          { "id": "gpt-5.6-sol", "name": "GPT-5.6 Sol", "type": "chat" }
        ],
        "lastModified": 1794769407,
        "etag": "etag-value",
        "checkedAt": 1794769407
      }
    }"#;

    // A provider pi only has in the catalog (a signed-in openai-codex) is listed
    // with every chat model, marked read-only, and the pane still lists the
    // configured provider exactly as before.
    #[test]
    fn a_catalog_only_provider_is_listed_with_source_catalog() {
        let dir = TempModelsDir::new("catalog_only");
        dir.write(CONFIGURED_FILE);
        // The human signed in to openai-codex: pi's credentials file carries the
        // entry, which is what makes the cached catalog usable.
        dir.write_auth(AUTH_OPENAI_CODEX);
        dir.write_store(
            r#"{
              "openai-codex": {
                "models": [
                  { "id": "gpt-5.6-luna", "name": "GPT-5.6 Luna", "type": "chat" },
                  { "id": "gpt-5.6-sol", "name": "GPT-5.6 Sol", "type": "chat" }
                ],
                "lastModified": 1794769407,
                "etag": "etag-value",
                "checkedAt": 1794769407
              }
            }"#,
        );

        let providers = union(&dir);

        assert_eq!(
            providers
                .iter()
                .map(|p| p.provider.as_str())
                .collect::<Vec<_>>(),
            vec!["keep", "openai-codex"],
            "configured providers come first, catalog-only ones after them"
        );
        let codex = providers
            .iter()
            .find(|p| p.provider == "openai-codex")
            .expect("openai-codex is listed");
        assert_eq!(
            codex.name, "openai-codex",
            "pi's store carries no display name, so the id is the label"
        );
        assert_eq!(
            codex
                .models
                .iter()
                .map(|m| m.id.as_str())
                .collect::<Vec<_>>(),
            vec!["gpt-5.6-luna", "gpt-5.6-sol"]
        );
        assert!(
            codex
                .models
                .iter()
                .all(|m| m.source == MODEL_SOURCE_CATALOG),
            "catalog entries are read-only: pi owns that file"
        );

        // The configured provider is untouched, with its own source.
        let keep = providers
            .iter()
            .find(|p| p.provider == "keep")
            .expect("keep");
        assert_eq!(keep.name, "Keep");
        assert_eq!(keep.models.len(), 1);
        assert_eq!(keep.models[0].source, MODEL_SOURCE_CONFIGURED);

        // S4: neither the key nor the base url of either file reaches the payload.
        let json = serde_json::to_string(&providers).expect("serialize");
        assert!(!json.contains("sk-keep-secret"), "no api key: {json}");
        assert!(!json.contains("apiKey"), "no apiKey field: {json}");
        assert!(!json.contains("baseUrl"), "no baseUrl field: {json}");
        assert!(!json.contains("keep.test"), "no base url value: {json}");
    }

    // A provider present in both files is listed once: configured models first,
    // then the catalog models whose id is not configured. The configured entry
    // wins for an id both files carry.
    #[test]
    fn a_provider_in_both_files_merges_without_duplicating_an_id() {
        let dir = TempModelsDir::new("catalog_merge");
        dir.write(CONFIGURED_FILE);
        dir.write_store(
            r#"{
              "keep": {
                "models": [
                  { "id": "alpha", "name": "Alpha (catalog)", "contextWindow": 999, "type": "chat" },
                  { "id": "beta", "name": "Beta", "type": "chat" },
                  { "id": "gamma", "name": "Gamma", "type": "chat" }
                ]
              }
            }"#,
        );
        let store_before = std::fs::read_to_string(dir.store_path()).expect("read store");

        let providers = union(&dir);

        assert_eq!(providers.len(), 1, "the provider is listed exactly once");
        assert_eq!(
            listed_ids(&providers, "keep"),
            vec!["alpha", "beta", "gamma"],
            "configured models first, then the catalog ones it does not have"
        );
        let models = &providers[0].models;
        // The configured entry is the one kept for the shared id.
        assert_eq!(models[0].name, "Alpha (configured)");
        assert_eq!(models[0].source, MODEL_SOURCE_CONFIGURED);
        assert_eq!(models[0].context_window, Some(8192));
        assert_eq!(models[1].source, MODEL_SOURCE_CATALOG);
        assert_eq!(models[2].name, "Gamma");

        // The catalog is read-only: the union never rewrites it.
        assert_eq!(
            std::fs::read_to_string(dir.store_path()).expect("read store"),
            store_before
        );
    }

    // Only conversation entries are listed: pi's store also carries non-chat
    // kinds, which the selector cannot offer.
    #[test]
    fn non_chat_catalog_entries_are_skipped() {
        let dir = TempModelsDir::new("catalog_types");
        dir.write(CONFIGURED_FILE);
        dir.write_auth(AUTH_OPENAI);
        dir.write_store(
            r#"{
              "openai": {
                "models": [
                  { "id": "chat-latest", "name": "Chat Latest", "type": "chat" },
                  { "id": "omni-moderation", "type": "classifier" },
                  { "id": "no-type", "name": "No Type" },
                  { "name": "no id at all", "type": "chat" },
                  { "id": "", "type": "chat" }
                ]
              }
            }"#,
        );

        assert_eq!(
            listed_ids(&union(&dir), "openai"),
            vec!["chat-latest", "no-type"],
            "a non-chat kind is skipped, and an entry without an id is not a choice"
        );
    }

    // The catalog is mapped by the same mapper as the configured file, so a
    // signed-in model carries its real window, cost and modalities. It is always
    // enabled: pi owns the entry.
    #[test]
    fn a_catalog_model_maps_pi_fields_onto_the_contract() {
        let dir = TempModelsDir::new("catalog_mapping");
        dir.write_auth(AUTH_OPENAI);
        dir.write_store(
            r#"{
              "openai": {
                "models": [
                  {
                    "id": "chat-latest",
                    "name": "Chat Latest",
                    "api": "openai-responses",
                    "provider": "openai",
                    "baseUrl": "https://api.openai.com/v1",
                    "reasoning": true,
                    "input": ["text", "image"],
                    "cost": { "input": 5, "output": 30, "cacheRead": 0.5, "cacheWrite": 0 },
                    "contextWindow": 400000,
                    "maxTokens": 128000,
                    "enabled": false,
                    "type": "chat"
                  }
                ]
              }
            }"#,
        );

        let providers = union(&dir);
        assert_eq!(providers.len(), 1, "no configured file, only the catalog");
        assert_eq!(
            providers[0].models,
            vec![ModelInfo {
                id: "chat-latest".to_string(),
                name: "Chat Latest".to_string(),
                context_window: Some(400000),
                max_tokens: Some(128000),
                input_cost: Some(5.0),
                output_cost: Some(30.0),
                modalities: vec!["text".to_string(), "image".to_string()],
                enabled: true,
                hidden: false,
                source: MODEL_SOURCE_CATALOG.to_string(),
            }]
        );

        // CamelCase on the wire, and the read-only marker is part of it.
        let json = serde_json::to_value(&providers).expect("serialize");
        let model = &json[0]["models"][0];
        assert_eq!(model["source"], serde_json::json!("catalog"));
        assert_eq!(model["contextWindow"], serde_json::json!(400000));
        assert_eq!(model["maxTokens"], serde_json::json!(128000));
        assert_eq!(model["inputCost"], serde_json::json!(5.0));
        assert_eq!(model["outputCost"], serde_json::json!(30.0));
        assert_eq!(model["modalities"], serde_json::json!(["text", "image"]));
        assert_eq!(model["enabled"], serde_json::json!(true));
        assert!(model.get("baseUrl").is_none(), "no base url: {model}");
        assert!(model.get("apiKey").is_none(), "no api key: {model}");
    }

    // A fresh install has never been signed in: the store does not exist, and
    // the configured listing still works.
    #[test]
    fn a_missing_catalog_file_lists_the_configured_providers_only() {
        let dir = TempModelsDir::new("catalog_missing");
        dir.write(CONFIGURED_FILE);

        let providers = union(&dir);

        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].provider, "keep");
        assert_eq!(listed_ids(&providers, "keep"), vec!["alpha"]);
        assert!(!dir.store_path().exists(), "nothing is created");
    }

    // A corrupt cache is a cache pi rebuilds by itself: it contributes no
    // catalog entries and never fails the command.
    #[test]
    fn a_corrupt_catalog_file_lists_the_configured_providers_only() {
        let dir = TempModelsDir::new("catalog_corrupt");
        dir.write(CONFIGURED_FILE);
        let configured_before = dir.read();

        for contents in [
            "{ not json",
            "",
            "[]",
            r#""just a string""#,
            r#"{"keep":{"models":"not an array"}}"#,
            r#"{"keep":[1,2,3]}"#,
            r#"{"openai":{"models":[42,"nope",null]}}"#,
        ] {
            dir.write_store(contents);
            let providers = union(&dir);
            assert_eq!(
                providers
                    .iter()
                    .map(|p| p.provider.as_str())
                    .collect::<Vec<_>>(),
                vec!["keep"],
                "only the configured provider is listed for {contents:?}"
            );
            assert_eq!(listed_ids(&providers, "keep"), vec!["alpha"]);
            assert_eq!(
                dir.read(),
                configured_before,
                "models.json is not rewritten"
            );
            assert_eq!(
                std::fs::read_to_string(dir.store_path()).expect("read store"),
                contents,
                "the catalog is never rewritten"
            );
        }
    }

    // The tolerance is for the catalog only: a broken configured file is still a
    // parse error, exactly like the configured-only read reports it.
    #[test]
    fn a_corrupt_configured_file_is_still_a_parse_error() {
        let dir = TempModelsDir::new("catalog_corrupt_models");
        dir.write("{ not json");
        dir.write_store(r#"{"openai":{"models":[{"id":"chat-latest"}]}}"#);

        assert!(matches!(
            read_provider_models_with_catalog(&dir.path(), &dir.store_path(), &dir.auth_path()),
            Err(ProvidersError::Parse(_))
        ));
    }

    // The path resolver targets pi's own cache, next to models.json.
    #[test]
    fn models_store_path_is_pis_agent_models_store_json() {
        assert_eq!(
            models_store_path(Path::new("/home/u")),
            PathBuf::from("/home/u/.pi/agent/models-store.json")
        );
    }

    // The credentials file is pi's own too, next to models.json.
    #[test]
    fn auth_path_is_pis_agent_auth_json() {
        assert_eq!(
            auth_path(Path::new("/home/u")),
            PathBuf::from("/home/u/.pi/agent/auth.json")
        );
    }

    // ── usability of a cached provider ──────────────────────────────────────

    // S2 of `model-visibility.md`: the catalog caches every provider pi has ever
    // seen, signed in or not, so a cached entry alone is not evidence of a usable
    // provider. openai-codex is exactly that case today: cached, never signed in.
    #[test]
    fn a_catalog_provider_without_an_auth_entry_is_not_exposed() {
        let dir = TempModelsDir::new("catalog_unauthenticated");
        dir.write(CONFIGURED_FILE);
        dir.write_store(CODEX_STORE);

        // No auth.json at all: a fresh install has nothing signed in.
        assert_eq!(
            listed_providers(&union(&dir)),
            vec!["keep"],
            "a cached catalogue without credentials is not a provider"
        );

        // An auth.json naming another provider does not authenticate this one.
        dir.write_auth(AUTH_OPENAI);
        assert_eq!(
            listed_providers(&union(&dir)),
            vec!["keep"],
            "openai is signed in, openai-codex is not"
        );

        // An entry that is not a non-empty object carries no credentials.
        dir.write_auth(r#"{"openai-codex":{},"openai":null,"other":"value","more":[1]}"#);
        assert_eq!(
            listed_providers(&union(&dir)),
            vec!["keep"],
            "an empty or non-object entry is not a sign-in, and the provider is not even \
             listed empty"
        );
    }

    #[test]
    fn a_catalog_provider_with_an_auth_entry_is_exposed() {
        let dir = TempModelsDir::new("catalog_authenticated");
        dir.write(CONFIGURED_FILE);
        dir.write_store(CODEX_STORE);
        dir.write_auth(AUTH_OPENAI_CODEX);

        let providers = union(&dir);
        assert_eq!(listed_providers(&providers), vec!["keep", "openai-codex"]);
        assert_eq!(
            listed_ids(&providers, "openai-codex"),
            vec!["gpt-5.6-luna", "gpt-5.6-sol"]
        );
        let codex = providers
            .iter()
            .find(|p| p.provider == "openai-codex")
            .expect("openai-codex is listed");
        assert!(
            codex
                .models
                .iter()
                .all(|m| m.source == MODEL_SOURCE_CATALOG),
            "a catalog entry stays read-only: pi owns that file"
        );
    }

    // A provider the human configured authenticates itself: models.json is the
    // evidence, so no auth entry is needed for its cached catalog models.
    #[test]
    fn a_catalog_provider_configured_in_models_json_is_exposed_without_auth() {
        let dir = TempModelsDir::new("catalog_configured_no_auth");
        dir.write(CONFIGURED_FILE);
        dir.write_store(
            r#"{"keep":{"models":[
                {"id":"alpha","name":"Alpha (catalog)"},
                {"id":"beta","name":"Beta","type":"chat"}
            ]}}"#,
        );

        let providers = union(&dir);
        assert_eq!(listed_providers(&providers), vec!["keep"]);
        assert_eq!(
            listed_ids(&providers, "keep"),
            vec!["alpha", "beta"],
            "the merged catalog models are listed without any credentials"
        );
    }

    // The credentials file is a cache of its own: a broken one never fails the
    // listing, it only means no catalogue-only provider is exposed.
    #[test]
    fn a_corrupt_auth_file_lists_the_configured_providers_only() {
        let dir = TempModelsDir::new("auth_corrupt");
        dir.write(CONFIGURED_FILE);
        dir.write_store(CODEX_STORE);

        for contents in [
            "{ not json",
            "",
            "[]",
            r#""just a string""#,
            r#"{"openai-codex":42}"#,
            r#"{"openai-codex":[]}"#,
        ] {
            dir.write_auth(contents);
            assert_eq!(
                listed_providers(&union(&dir)),
                vec!["keep"],
                "only the configured provider survives {contents:?}"
            );
        }
    }

    // ── hidden models ───────────────────────────────────────────────────────

    // S2 of `model-visibility.md`: the hidden list is Nabla's own preference and
    // is applied to the listing. It marks the matching `<provider>/<modelId>`
    // pair for a configured and a catalog model alike, marks nothing else, and
    // never removes an entry: a hidden model is still described in full.
    #[test]
    fn hidden_models_mark_only_the_matching_entries() {
        let dir = TempModelsDir::new("hidden_marks");
        dir.write(PI_FILE);
        dir.write_store(CODEX_STORE);
        dir.write_auth(AUTH_OPENAI_CODEX);

        let mut providers = union(&dir);
        let hidden = vec![
            crate::settings::hidden_model_key("nan", "qwen3.6"),
            crate::settings::hidden_model_key("openai-codex", "gpt-5.6-sol"),
            // A pair for a model that is not listed marks nothing.
            crate::settings::hidden_model_key("nan", "ghost"),
        ];
        apply_hidden_models(&mut providers, &hidden);

        let nan = providers.iter().find(|p| p.provider == "nan").expect("nan");
        assert_eq!(
            listed_ids(&providers, "nan").len(),
            2,
            "the hidden model is still listed"
        );
        assert!(nan.models[0].hidden, "the configured model is hidden");
        assert!(!nan.models[1].hidden, "its sibling stays visible");
        assert_eq!(
            nan.models[0].source, MODEL_SOURCE_CONFIGURED,
            "hiding is a mark, not an edit"
        );

        let codex = providers
            .iter()
            .find(|p| p.provider == "openai-codex")
            .expect("openai-codex");
        assert!(!codex.models[0].hidden);
        assert!(codex.models[1].hidden, "a catalog model is hideable too");
        assert_eq!(codex.models[1].source, MODEL_SOURCE_CATALOG);

        let empty = providers
            .iter()
            .find(|p| p.provider == "empty")
            .expect("empty");
        assert!(
            empty.models.is_empty(),
            "a provider without models is untouched"
        );

        // The flag travels in the payload the UI reads.
        let json = serde_json::to_value(&providers).expect("serialize");
        let nan = json
            .as_array()
            .expect("array")
            .iter()
            .find(|p| p["provider"] == serde_json::json!("nan"))
            .expect("nan in the payload");
        assert_eq!(nan["models"][0]["hidden"], serde_json::json!(true));
        assert_eq!(nan["models"][1]["hidden"], serde_json::json!(false));
    }

    // An empty hidden list leaves every model visible: the default of a fresh
    // install, and of every settings file written before the preference existed.
    #[test]
    fn an_empty_hidden_list_leaves_every_model_visible() {
        let dir = TempModelsDir::new("hidden_empty");
        dir.write(PI_FILE);

        let mut providers = union(&dir);
        apply_hidden_models(&mut providers, &[]);

        assert!(
            providers
                .iter()
                .flat_map(|p| p.models.iter())
                .all(|m| !m.hidden),
            "nothing is hidden by default"
        );
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
                hidden: false,
                source: MODEL_SOURCE_CONFIGURED.to_string(),
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
        assert_eq!(
            root["providers"]["nan"]["timeoutSeconds"],
            serde_json::json!(300)
        );
        assert_eq!(
            root["providers"]["nan"]["modelOverrides"]["qwen3.6"]["contextWindow"],
            serde_json::json!(4096)
        );
        assert_eq!(
            root["providers"]["nan"]["apiKey"],
            serde_json::json!("sk-nan-secret")
        );
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
        assert_eq!(
            model_ids(&root, "nan"),
            vec!["qwen3.6"],
            "nothing is dropped"
        );
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
        assert_eq!(
            model_ids(&root, "nan"),
            vec!["glm5.3"],
            "only one model is gone"
        );
        let original: serde_json::Value = serde_json::from_str(PI_FILE).expect("parse fixture");
        assert_eq!(
            root["providers"]["nan"]["models"][0], original["providers"]["nan"]["models"][1],
            "the surviving model is untouched"
        );
        assert_eq!(root["providers"]["empty"], original["providers"]["empty"]);
        assert_eq!(
            root["providers"]["nan"]["apiKey"],
            serde_json::json!("sk-nan-secret")
        );
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
        assert_eq!(
            missing.entries(),
            Vec::<String>::new(),
            "nothing is written"
        );
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
            assert!(
                listed_ids.contains(&id),
                "{id:?} is missing: {listed_ids:?}"
            );
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
                hidden: true,
                source: MODEL_SOURCE_CONFIGURED.to_string(),
            }],
        };
        let json = serde_json::to_value(&provider).expect("serialize");
        assert_eq!(json["provider"], serde_json::json!("nan"));
        assert_eq!(json["name"], serde_json::json!("NaN"));
        let model = &json["models"][0];
        assert_eq!(model["id"], serde_json::json!("qwen3.6"));
        assert_eq!(
            model["source"],
            serde_json::json!("configured"),
            "every model says which file it came from"
        );
        assert_eq!(model["contextWindow"], serde_json::json!(131072));
        assert_eq!(model["maxTokens"], serde_json::json!(8192));
        assert_eq!(
            model["hidden"],
            serde_json::json!(true),
            "the hidden mark is part of the payload"
        );
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
                    ..Default::default()
                },
                ModelOption {
                    id: "qwen3.6-35b".to_string(),
                    name: "Qwen3.6 35B".to_string(),
                    ..Default::default()
                },
                ModelOption {
                    id: "nameless".to_string(),
                    name: "nameless".to_string(),
                    ..Default::default()
                },
            ]
        );
    }

    #[test]
    fn parse_models_response_extracts_capabilities_and_pricing() {
        let body = r#"{
          "data": [
            {
              "id": "anthropic/claude-sonnet",
              "name": "Claude Sonnet",
              "context_length": 200000,
              "top_provider": { "max_completion_tokens": 16384 },
              "architecture": { "input_modalities": ["text", "image"] },
              "pricing": { "prompt": "0.000003", "completion": "0.000015" }
            },
            {
              "id": "local-vlm",
              "context_window": 32768,
              "max_output_tokens": 4096,
              "capabilities": { "chat": true, "vision": true }
            }
          ]
        }"#;
        let models = parse_models_response(body);
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].context_window, Some(200000));
        assert_eq!(models[0].max_tokens, Some(16384));
        assert_eq!(models[0].modalities, vec!["text", "image"]);
        assert_eq!(models[0].input_cost, Some(3.0));
        assert_eq!(models[0].output_cost, Some(15.0));

        assert_eq!(models[1].context_window, Some(32768));
        assert_eq!(models[1].max_tokens, Some(4096));
        assert_eq!(models[1].modalities, vec!["text", "image"]);
        assert_eq!(models[1].input_cost, None);
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
        assert_eq!(
            missing.entries(),
            Vec::<String>::new(),
            "nothing is written"
        );
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
            ..Default::default()
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
        assert_eq!(
            json["baseUrl"],
            serde_json::json!("https://api.example.com")
        );
        assert_eq!(json["modelCount"], serde_json::json!(3));
        assert_eq!(json["hasApiKey"], serde_json::json!(true));

        let option = ModelOption {
            id: "m".to_string(),
            name: "M".to_string(),
            ..Default::default()
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
