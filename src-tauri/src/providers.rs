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
    InvalidBaseUrl(String),
    EmptyApiKey,
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
            ProvidersError::InvalidBaseUrl(url) => {
                write!(f, "provider base url must be http(s): {url:?}")
            }
            ProvidersError::EmptyApiKey => write!(f, "provider api key must not be empty"),
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
    if existed {
        let backup = backup_path(path)?;
        std::fs::copy(path, &backup).map_err(|e| {
            ProvidersError::Io(format!("backup {}: {e}", backup.display()))
        })?;
    }

    let serialized = serde_json::to_string_pretty(&root)
        .map_err(|e| ProvidersError::Parse(e.to_string()))?;
    write_atomic(path, &serialized)?;

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

fn summary_of(id: &str, entry: &serde_json::Value) -> ProviderSummary {
    let name = entry
        .get("name")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .unwrap_or(id)
        .to_string();
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
    let response = client
        .get(&endpoint)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Accept", "application/json")
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
