//! Local prompt skills (`.md` templates) discoverable from `/` in the composer.
//!
//! Scans two directories when present:
//! - `<config_dir>/skills/*.md` (global user skills)
//! - `<workspace>/.nabla/skills/*.md` (workspace-local skills)
//!
//! Each file may begin with minimal YAML frontmatter (`name:` and `description:`);
//! the remaining Markdown body is the prompt template inserted when chosen.

use std::fs;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillEntry {
    pub id: String,
    pub name: String,
    pub description: String,
    pub prompt: String,
    /// `"global"` or `"workspace"`.
    pub source: String,
}

const MAX_SKILL_BYTES: u64 = 128 * 1024;

/// List all readable `.md` skills from global config and (optionally) a workspace.
/// Workspace skills override global skills with the same `name`.
pub fn list_skills(config_dir: &Path, workspace_path: Option<&Path>) -> Vec<SkillEntry> {
    let mut entries = scan_dir(&config_dir.join("skills"), "global");
    if let Some(ws) = workspace_path {
        let local = scan_dir(&ws.join(".nabla").join("skills"), "workspace");
        for skill in local {
            entries.retain(|existing| existing.name != skill.name);
            entries.push(skill);
        }
    }
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    entries
}

fn scan_dir(dir: &Path, source: &str) -> Vec<SkillEntry> {
    let Ok(read_dir) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in read_dir.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let is_md = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("md"));
        if !is_md {
            continue;
        }
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if meta.len() > MAX_SKILL_BYTES {
            continue;
        }
        let Ok(raw) = fs::read_to_string(&path) else {
            continue;
        };
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("skill")
            .trim();
        if stem.is_empty() {
            continue;
        }
        if let Some(parsed) = parse_skill(stem, &raw, source) {
            out.push(parsed);
        }
    }
    out
}

fn parse_skill(fallback_name: &str, raw: &str, source: &str) -> Option<SkillEntry> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    let mut name = fallback_name.to_string();
    let mut description = String::new();
    let mut body = trimmed;

    if let Some(after_open) = trimmed.strip_prefix("---\n").or_else(|| trimmed.strip_prefix("---\r\n"))
        && let Some(close_idx) = after_open.find("\n---")
    {
        let header = &after_open[..close_idx];
        let rest = &after_open[close_idx + 4..];
        body = rest.trim();

        for line in header.lines() {
            if let Some(v) = line.strip_prefix("name:") {
                let cleaned = v.trim().trim_matches('"').trim_matches('\'');
                if !cleaned.is_empty() {
                    name = cleaned.to_string();
                }
            } else if let Some(v) = line.strip_prefix("description:") {
                let cleaned = v.trim().trim_matches('"').trim_matches('\'');
                description = cleaned.to_string();
            }
        }
    }

    if body.is_empty() {
        return None;
    }
    if description.is_empty() {
        description = body
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("")
            .trim()
            .chars()
            .take(90)
            .collect();
    }

    let slug = sanitize_slug(&name);
    if slug.is_empty() {
        return None;
    }

    Some(SkillEntry {
        id: format!("{source}:{slug}"),
        name: slug,
        description,
        prompt: body.to_string(),
        source: source.to_string(),
    })
}

fn sanitize_slug(raw: &str) -> String {
    raw.trim()
        .trim_start_matches('/')
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

/// Saves or updates a global skill `.md` file under `<config_dir>/skills/<slug>.md`.
pub fn save_skill(
    config_dir: &Path,
    name: &str,
    description: &str,
    prompt: &str,
) -> Result<SkillEntry, String> {
    let slug = sanitize_slug(name);
    if slug.is_empty() {
        return Err("skill name must contain letters or digits".to_string());
    }
    let body = prompt.trim();
    if body.is_empty() {
        return Err("skill prompt must not be empty".to_string());
    }
    let desc = description.trim().replace(['\r', '\n'], " ");
    let dir = config_dir.join("skills");
    fs::create_dir_all(&dir)
        .map_err(|e| format!("failed to create skills dir {}: {e}", dir.display()))?;
    let path = dir.join(format!("{slug}.md"));
    let content = if desc.is_empty() {
        format!("---\nname: {slug}\n---\n\n{body}\n")
    } else {
        format!("---\nname: {slug}\ndescription: {desc}\n---\n\n{body}\n")
    };
    fs::write(&path, content.as_bytes())
        .map_err(|e| format!("failed to write skill {}: {e}", path.display()))?;
    parse_skill(&slug, &content, "global")
        .ok_or_else(|| "failed to parse saved skill".to_string())
}

/// Deletes a global skill `.md` file from `<config_dir>/skills/<slug>.md`.
pub fn delete_skill(config_dir: &Path, name: &str) -> Result<bool, String> {
    let slug = sanitize_slug(name);
    if slug.is_empty() {
        return Err("invalid skill name".to_string());
    }
    let path = config_dir.join("skills").join(format!("{slug}.md"));
    if !path.exists() {
        return Ok(false);
    }
    fs::remove_file(&path)
        .map_err(|e| format!("failed to delete skill {}: {e}", path.display()))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_global_and_workspace_skills_with_frontmatter_override() {
        let base = std::env::temp_dir().join(format!("nabla-skills-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let global_dir = base.join("config").join("skills");
        let ws_dir = base.join("ws").join(".nabla").join("skills");
        fs::create_dir_all(&global_dir).unwrap();
        fs::create_dir_all(&ws_dir).unwrap();

        fs::write(
            global_dir.join("review.md"),
            "---\nname: review\ndescription: Global code review\n---\nReview this diff carefully.",
        )
        .unwrap();
        fs::write(
            ws_dir.join("review.md"),
            "---\nname: review\ndescription: Workspace override\n---\nReview against project rules.",
        )
        .unwrap();
        fs::write(ws_dir.join("summary.md"), "Summarize the current thread.").unwrap();

        let skills = list_skills(&base.join("config"), Some(&base.join("ws")));
        assert_eq!(skills.len(), 2);
        assert_eq!(skills[0].name, "review");
        assert_eq!(skills[0].source, "workspace");
        assert_eq!(skills[0].description, "Workspace override");
        assert_eq!(skills[1].name, "summary");
        assert_eq!(skills[1].prompt, "Summarize the current thread.");

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn save_and_delete_global_skill_round_trip() {
        let base = std::env::temp_dir().join(format!("nabla-skills-crud-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);

        let saved = save_skill(&base, "/Code Review", "Checks diffs", "Inspect every change.")
            .expect("save skill");
        assert_eq!(saved.name, "code-review");
        assert_eq!(saved.description, "Checks diffs");
        assert_eq!(saved.prompt, "Inspect every change.");

        let listed = list_skills(&base, None);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "code-review");

        assert!(delete_skill(&base, "code-review").expect("delete"));
        assert!(!delete_skill(&base, "code-review").expect("delete missing"));
        assert!(list_skills(&base, None).is_empty());

        let _ = fs::remove_dir_all(&base);
    }
}
