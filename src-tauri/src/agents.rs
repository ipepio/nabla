// Agent profiles module: the subagent profiles the human configures, and the
// gate that decides whether a session can delegate at all.
//
// S1 of `agent-profiles.md`: profiles are NABLA's, not pi's. They live in one
// human-editable file, `<data_dir>/agents.json`, next to `settings.json` and
// `permissions.json` — never inside the project, and never under a `.pi/`
// directory (that convention belongs to pi, and the extension used to read it
// from `<cwd>/<CONFIG_DIR_NAME>/agents`; that read is gone).
//
// The file is written atomically (the same temp-then-rename the settings file
// uses) and read whole. Unknown keys are tolerated, so a file written by a newer
// build still loads.
//
// S3: **nothing is active by default**. The master switch (`enabled`) and every
// profile's own switch (`enabled`) are `false` when absent, and a missing file is
// the off state, not an error. The decision is read FRESH AT SPAWN TIME by
// `process::ProcessManager` (`config_for_spawn`), never cached at boot: turning
// the switch on affects the next session, and turning it off affects the next
// session too. A session that is already running keeps what it started with —
// there is no hot reload, on purpose: the extension is loaded into a pi process
// with `-e` and pi has no way to unload one.
//
// S7: the routing table travels with the same decision (`routing_table_enabled`),
// so the model only reads how to delegate when it really can.

use std::path::{Path, PathBuf};

use crate::settings;

/// S1: Nabla's profile store, beside the two files the app already owns.
pub fn config_path(data_dir: &Path) -> PathBuf {
    data_dir.join("agents.json")
}

/// One named profile (S1, S2). camelCase on the wire, so `src/contracts/types.ts`
/// mirrors these keys field by field.
///
/// `tools` is the child's allowlist as the human configured it; an empty list is
/// a real answer ("this profile gets no tools") and not a validation error.
/// `model` is `provider/model` or a bare model id, resolved against the session's
/// authenticated registry at run time (S6); `effort` is one of pi's thinking
/// levels. `instructions` is the child's whole system prompt.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AgentProfile {
    pub name: String,
    pub description: String,
    pub tools: Vec<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub instructions: String,
    pub enabled: bool,
}

/// The whole store: the master switch, the fallback model and the profiles.
///
/// `#[serde(default)]` at the container level is what makes S3 hold for a file
/// with missing keys: every absent field takes its `Default`, which is `false`,
/// `None` and an empty list.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AgentsConfig {
    /// S3: the master switch. Off means no delegation tools at all.
    pub enabled: bool,
    /// S6, second level of the model resolution order: the model a profile uses
    /// when it names none. The parent session's model is the third level.
    pub model_default: Option<String>,
    pub profiles: Vec<AgentProfile>,
}

impl AgentsConfig {
    /// S3: delegation is a capability only when the human turned the master
    /// switch on AND there is at least one profile to offer. On with nothing
    /// enabled would load four tools that can only answer "nothing to run".
    pub fn delegation_active(&self) -> bool {
        self.enabled && self.enabled_profiles().next().is_some()
    }

    /// S2: the profiles the tools may use, in file order.
    pub fn enabled_profiles(&self) -> impl Iterator<Item = &AgentProfile> {
        self.profiles.iter().filter(|profile| profile.enabled)
    }
}

/// One model's worth of shape checking, shared by the profiles and the default.
///
/// No model list lives here on purpose (the catalogue is pi's, and the session's
/// authenticated registry is what really decides): the check is only that the
/// text can be a model id at all. A `provider/model` pair with an empty half, or
/// a value with spaces in it, can never resolve and would fail at run time
/// instead of at save time.
fn validate_model(model: &str, owner: &str) -> Result<(), AgentsError> {
    let trimmed = model.trim();
    if trimmed.is_empty() {
        return Err(AgentsError::Invalid(format!(
            "{owner} is empty: remove the field to fall back to the next level of the \
             resolution order, or name a model as `provider/model`"
        )));
    }
    if trimmed.chars().any(char::is_whitespace) {
        return Err(AgentsError::Invalid(format!(
            "{owner} is {trimmed:?}, which cannot be a model id: a model id carries no \
             spaces (use `provider/model`, or a bare id)"
        )));
    }
    if let Some((provider, id)) = trimmed.split_once('/')
        && (provider.is_empty() || id.is_empty())
    {
        return Err(AgentsError::Invalid(format!(
            "{owner} is {trimmed:?}, which is not `provider/model`: both halves have to \
             be named"
        )));
    }
    Ok(())
}

impl AgentsConfig {
    /// Validates the store before it is written. Each message says which profile
    /// and what is wrong with it, because the human reads them in the dialog.
    pub fn validate(&self) -> Result<(), AgentsError> {
        if let Some(model) = &self.model_default {
            validate_model(model, "the default subagent model")?;
        }
        // Names are the handle `run_agent` takes, so they are unique by the same
        // rule the extension matches by: lowercased and trimmed.
        let mut seen: Vec<String> = Vec::with_capacity(self.profiles.len());
        for (index, profile) in self.profiles.iter().enumerate() {
            let name = profile.name.trim();
            if name.is_empty() {
                return Err(AgentsError::Invalid(format!(
                    "profile #{} has no name, and the name is how the agent asks for it",
                    index + 1
                )));
            }
            if profile.description.trim().is_empty() {
                return Err(AgentsError::Invalid(format!(
                    "the profile `{name}` has no description, and the description is what \
                     tells the model when to reach for it"
                )));
            }
            if profile.tools.iter().any(|tool| tool.trim().is_empty()) {
                return Err(AgentsError::Invalid(format!(
                    "the profile `{name}` lists an empty tool name: remove it, or name a tool"
                )));
            }
            if let Some(effort) = &profile.effort
                && settings::parse_thinking_level(effort).is_none()
            {
                return Err(AgentsError::Invalid(format!(
                    "the effort of the profile `{name}` is {effort:?}, which is not a thinking \
                     level pi has ({})",
                    settings::THINKING_LEVELS.join(", ")
                )));
            }
            if let Some(model) = &profile.model {
                validate_model(model, &format!("the model of the profile `{name}`"))?;
            }
            let key = name.to_lowercase();
            if seen.contains(&key) {
                return Err(AgentsError::Invalid(format!(
                    "two profiles are named `{name}`: names are how the agent picks one, so \
                     they have to be unique (case does not matter)"
                )));
            }
            seen.push(key);
        }
        Ok(())
    }
}

#[derive(Debug)]
pub enum AgentsError {
    Io(String),
    Parse(String),
    /// A profile the file would carry cannot be one: the message names it.
    Invalid(String),
}

impl std::fmt::Display for AgentsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AgentsError::Io(message) => write!(f, "agents io error: {message}"),
            AgentsError::Parse(message) => write!(f, "agents parse error: {message}"),
            AgentsError::Invalid(message) => write!(f, "invalid subagent profile: {message}"),
        }
    }
}

impl std::error::Error for AgentsError {}

pub type AgentsResult<T> = Result<T, AgentsError>;

/// Loads the store from `path`. A missing file is the off state (S3), not an
/// error: a fresh install has never configured a profile, and that is exactly
/// what "nothing is active by default" means.
pub fn load(path: &Path) -> AgentsResult<AgentsConfig> {
    match std::fs::read_to_string(path) {
        Ok(raw) => serde_json::from_str(&raw).map_err(|e| AgentsError::Parse(e.to_string())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(AgentsConfig::default()),
        Err(e) => Err(AgentsError::Io(e.to_string())),
    }
}

/// Validates and writes the store atomically, through the one temp-then-rename
/// helper the settings file already uses.
pub fn save(path: &Path, config: &AgentsConfig) -> AgentsResult<()> {
    config.validate()?;
    let json =
        serde_json::to_string_pretty(config).map_err(|e| AgentsError::Parse(e.to_string()))?;
    settings::write_atomic(path, &json).map_err(|e| AgentsError::Io(e.to_string()))
}

/// S3/S8: what one spawn reads, fresh from disk.
///
/// `None` (a manager without a data dir, which only tests build) is the off
/// state; so is a file this build cannot parse — delegation is a capability, and
/// an unreadable store must never grant one. The caller that wants to tell the
/// human about an unparseable file is the settings dialog, not a spawn.
pub fn config_for_spawn(data_dir: Option<&Path>) -> AgentsConfig {
    match data_dir {
        Some(dir) => load(&config_path(dir)).unwrap_or_default(),
        None => AgentsConfig::default(),
    }
}

/// S3/S8: the subagent extension this spawn loads, or `None` for no delegation at
/// all. The master switch and the enabled profiles are the whole gate: off means
/// the `-e` entry never reaches pi's argv, so `run_agent` does not exist for the
/// model instead of existing and failing.
pub fn extension_for_spawn(config: &AgentsConfig, materialized: &Path) -> Option<PathBuf> {
    config
        .delegation_active()
        .then(|| materialized.to_path_buf())
}

/// S7: whether the routing table joins the system prompt. Same decision, so the
/// prompt and the tools can never disagree about whether delegating is possible.
pub fn routing_table_enabled(config: &AgentsConfig) -> bool {
    config.delegation_active()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Scratch directory outside the worktree, removed on drop.
    struct Scratch {
        dir: PathBuf,
    }

    impl Scratch {
        fn new(tag: &str) -> Scratch {
            let mut dir = std::env::temp_dir();
            dir.push(format!(
                "nabla_agents_{}_{}_{tag}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&dir).expect("create scratch dir");
            Scratch { dir }
        }

        fn path(&self) -> PathBuf {
            config_path(&self.dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn profile(name: &str) -> AgentProfile {
        AgentProfile {
            name: name.to_string(),
            description: format!("{name} does one thing"),
            tools: vec!["read".to_string()],
            model: None,
            effort: None,
            instructions: "Do it.".to_string(),
            enabled: true,
        }
    }

    fn config(profiles: Vec<AgentProfile>) -> AgentsConfig {
        AgentsConfig {
            enabled: true,
            model_default: None,
            profiles,
        }
    }

    // S3: a fresh install has no file, and a missing file is the off state — not
    // an error and not an empty-but-on switch.
    #[test]
    fn a_missing_file_is_the_off_state_with_no_profiles() {
        let scratch = Scratch::new("missing");
        let loaded = load(&scratch.path()).expect("a missing file loads as the off state");
        assert!(!loaded.enabled, "S3: nothing is active by default");
        assert!(loaded.profiles.is_empty());
        assert_eq!(loaded, AgentsConfig::default());
        assert!(
            !loaded.delegation_active(),
            "no master switch, no delegation"
        );
    }

    // S3: every default is off, and the wire keys are camelCase so the TS mirror
    // can be written field by field.
    #[test]
    fn defaults_are_off_and_serialize_camel_case() {
        let config = AgentsConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.model_default, None);
        assert!(config.profiles.is_empty());
        assert!(
            !AgentProfile::default().enabled,
            "a profile that says nothing about its switch is off"
        );

        let json = serde_json::to_value(AgentsConfig {
            enabled: true,
            model_default: Some("nan/glm5.3".to_string()),
            profiles: vec![profile("explorer")],
        })
        .expect("serialize");
        assert_eq!(json["enabled"], serde_json::json!(true));
        assert_eq!(json["modelDefault"], serde_json::json!("nan/glm5.3"));
        let entry = &json["profiles"][0];
        for key in [
            "name",
            "description",
            "tools",
            "model",
            "effort",
            "instructions",
            "enabled",
        ] {
            assert!(
                entry.get(key).is_some(),
                "the payload must carry `{key}`: {entry}"
            );
        }
        assert_eq!(entry["instructions"], serde_json::json!("Do it."));
    }

    // S1: the file is written atomically and reads back identical; the temp file
    // the write uses never survives it.
    #[test]
    fn atomic_write_round_trips_and_leaves_no_temp_file() {
        let scratch = Scratch::new("roundtrip");
        let written = AgentsConfig {
            enabled: true,
            model_default: Some("nan/glm5.3".to_string()),
            profiles: vec![AgentProfile {
                name: "judge".to_string(),
                description: "is it right".to_string(),
                tools: vec!["read".to_string(), "grep".to_string()],
                model: Some("openai/gpt-5.4".to_string()),
                effort: Some("high".to_string()),
                instructions: "Review the change.".to_string(),
                enabled: true,
            }],
        };
        save(&scratch.path(), &written).expect("save");
        assert_eq!(load(&scratch.path()).expect("load"), written);

        // One file, and no leftover temp beside it.
        let mut names: Vec<String> = std::fs::read_dir(&scratch.dir)
            .expect("read dir")
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .collect();
        names.sort();
        assert_eq!(names, vec!["agents.json".to_string()]);
    }

    // S2: a name is how `run_agent` addresses a profile, so two profiles cannot
    // share one — not even by case, which is how the extension matches.
    #[test]
    fn a_duplicate_name_is_refused_case_insensitively() {
        let scratch = Scratch::new("duplicate");
        let duplicated = config(vec![profile("worker"), profile("Worker")]);
        match save(&scratch.path(), &duplicated) {
            Err(AgentsError::Invalid(message)) => {
                assert!(
                    message.to_lowercase().contains("name"),
                    "the message has to say what is wrong: {message}"
                );
            }
            other => panic!("a duplicate name must be refused, got: {other:?}"),
        }
        assert!(
            !scratch.path().exists(),
            "a refused save writes no file at all"
        );
    }

    // S6: `effort` travels to pi as `--thinking`/`thinkingLevel`, so a word this
    // build does not know is refused at save time instead of failing at run time.
    #[test]
    fn an_unknown_effort_is_refused_and_the_levels_are_those_pi_names() {
        let scratch = Scratch::new("effort");
        for level in settings::THINKING_LEVELS {
            save(
                &scratch.path(),
                &config(vec![AgentProfile {
                    effort: Some((*level).to_string()),
                    ..profile("explorer")
                }]),
            )
            .unwrap_or_else(|err| panic!("{level} is a pi thinking level: {err}"));
        }

        let bad = config(vec![AgentProfile {
            effort: Some("mucho".to_string()),
            ..profile("explorer")
        }]);
        match save(&scratch.path(), &bad) {
            Err(AgentsError::Invalid(message)) => {
                assert!(message.contains("mucho"), "{message}");
                assert!(message.contains("explorer"), "{message}");
                assert!(message.contains("off"), "the levels are named: {message}");
            }
            other => panic!("an unknown effort must be refused, got: {other:?}"),
        }
    }

    // S2: a profile without a name or a description is not addressable and not
    // explainable, so it never reaches the file.
    #[test]
    fn a_profile_without_a_name_or_a_description_is_refused() {
        let scratch = Scratch::new("nameless");
        for broken in [
            AgentProfile {
                name: "   ".to_string(),
                ..profile("x")
            },
            AgentProfile {
                description: "  ".to_string(),
                ..profile("y")
            },
        ] {
            match save(&scratch.path(), &config(vec![broken.clone()])) {
                Err(AgentsError::Invalid(_)) => {}
                other => panic!("{broken:?} must be refused, got: {other:?}"),
            }
        }
        assert!(!scratch.path().exists());
    }

    // S6: a model that cannot be an id (`a b`, `/glm`) can never resolve, so the
    // save says so. An empty tool list is a real choice, not an error.
    #[test]
    fn model_shape_is_checked_and_an_empty_tool_list_is_allowed() {
        let scratch = Scratch::new("model");
        for bad in ["", "  ", "nan /glm5.3", "/glm5.3", "nan/"] {
            match save(
                &scratch.path(),
                &config(vec![AgentProfile {
                    model: Some(bad.to_string()),
                    ..profile("explorer")
                }]),
            ) {
                Err(AgentsError::Invalid(message)) => {
                    assert!(message.contains("explorer"), "{message}")
                }
                other => panic!("model {bad:?} must be refused, got: {other:?}"),
            }
        }
        // No model list is hardcoded: a pair and a bare id both pass.
        save(
            &scratch.path(),
            &config(vec![AgentProfile {
                tools: Vec::new(),
                model: Some("nan/glm5.3".to_string()),
                ..profile("explorer")
            }]),
        )
        .expect("a provider/model pair is a model");
        save(
            &scratch.path(),
            &config(vec![AgentProfile {
                model: Some("glm5.3".to_string()),
                ..profile("explorer")
            }]),
        )
        .expect("a bare model id is a model");
    }

    // S1: the settings header rule, applied here too — a file a newer build wrote
    // (or a hand edit) must still load, unknown keys and all.
    #[test]
    fn unknown_keys_still_load() {
        let scratch = Scratch::new("unknown");
        std::fs::write(
            scratch.path(),
            r#"{
                "enabled": true,
                "futureSetting": {"x": 1},
                "profiles": [
                    {"name": "explorer", "description": "where it is", "enabled": true,
                     "tools": ["read"], "instructions": "Find.", "futureProfileKey": 2}
                ]
            }"#,
        )
        .expect("write fixture");

        let loaded = load(&scratch.path()).expect("unknown keys are tolerated");
        assert!(loaded.enabled);
        assert_eq!(loaded.profiles.len(), 1);
        assert_eq!(loaded.profiles[0].name, "explorer");
        assert!(loaded.profiles[0].enabled);
        assert_eq!(
            loaded.model_default, None,
            "an absent field takes its default"
        );
    }

    // S3/S8: the extension is loaded only when there is something to offer. Off,
    // or on with nothing enabled, means the `-e` entry never reaches pi.
    #[test]
    fn extension_for_spawn_answers_none_until_the_gate_is_open() {
        let materialized = Path::new("/data/extensions/nabla-subagents.ts");

        let off = AgentsConfig {
            enabled: false,
            ..config(vec![profile("explorer")])
        };
        assert_eq!(
            extension_for_spawn(&off, materialized),
            None,
            "S3: the master switch off loads nothing"
        );

        let nothing_enabled = AgentsConfig {
            enabled: true,
            ..config(vec![AgentProfile {
                enabled: false,
                ..profile("explorer")
            }])
        };
        assert_eq!(
            extension_for_spawn(&nothing_enabled, materialized),
            None,
            "S3: nothing to offer means nothing to load"
        );
        assert!(!nothing_enabled.delegation_active());

        let open = config(vec![profile("explorer")]);
        assert_eq!(
            extension_for_spawn(&open, materialized),
            Some(materialized.to_path_buf()),
            "one enabled profile is enough to delegate"
        );
    }

    // S7: the prompt section follows the same switch, so the model is never told
    // how to delegate when it has no tools to do it with.
    #[test]
    fn the_routing_table_follows_the_master_switch() {
        assert!(!routing_table_enabled(&AgentsConfig::default()));
        assert!(!routing_table_enabled(&AgentsConfig {
            enabled: false,
            ..config(vec![profile("explorer")])
        }));
        assert!(!routing_table_enabled(&AgentsConfig {
            enabled: true,
            ..config(vec![AgentProfile {
                enabled: false,
                ..profile("explorer")
            }])
        }));
        assert!(routing_table_enabled(&config(vec![profile("explorer")])));
    }

    // S3/S8: the decision is read fresh at spawn time, so a change on disk reaches
    // the NEXT session without a restart — and an unreadable store reads as off.
    #[test]
    fn config_for_spawn_reads_the_file_fresh() {
        let scratch = Scratch::new("fresh");
        assert!(
            !config_for_spawn(Some(&scratch.dir)).delegation_active(),
            "no file, no delegation"
        );

        save(&scratch.path(), &config(vec![profile("explorer")])).expect("save");
        assert!(config_for_spawn(Some(&scratch.dir)).delegation_active());

        save(
            &scratch.path(),
            &AgentsConfig {
                enabled: false,
                ..config(vec![profile("explorer")])
            },
        )
        .expect("save off");
        assert!(
            !config_for_spawn(Some(&scratch.dir)).delegation_active(),
            "turning the switch off closes the gate for the next spawn"
        );

        // A body this build cannot parse is the off state, never a granted one.
        std::fs::write(scratch.path(), "{ not json").expect("write fixture");
        assert!(!config_for_spawn(Some(&scratch.dir)).delegation_active());
        // And a manager without a data dir never delegates.
        assert!(!config_for_spawn(None).delegation_active());
    }
}
