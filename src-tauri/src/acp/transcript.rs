//! Native Pi v3 JSONL session transcript bridge for bidirectional session
//! continuity between Nabla's core engine and ACP agents (Antigravity,
//! Claude Code, Gemini CLI).
//!
//! - When an ACP agent handles a turn, messages are appended in Pi's exact v3
//!   tree format (`type: "session"` header + `type: "message"` linked via
//!   `parentId`), so returning to a core model reloads the full conversation.
//! - When switching from core (or another ACP agent) into an ACP agent, recent
//!   turns are extracted from the same JSONL file for handoff context, and the
//!   `session_context` MCP tool exposes the complete history on demand.

use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

/// One normalized conversation turn read from a Nabla session JSONL file.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionTurn {
    pub index: usize,
    pub role: String,
    pub text: String,
    pub timestamp: Option<String>,
}

fn now_epoch_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn now_iso_utc() -> String {
    // Simple RFC3339 UTC timestamp using epoch seconds (matches Pi's ISO format).
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Civil date conversion (Howard Hinnant algorithm) — zero extra crate deps.
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let hour = rem / 3600;
    let min = (rem % 3600) / 60;
    let sec = rem % 60;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    format!("{year:04}-{m:02}-{d:02}T{hour:02}:{min:02}:{sec:02}.000Z")
}

fn random_hex(bytes_len: usize) -> String {
    let mut buf = vec![0u8; bytes_len];
    let _ = getrandom::fill(&mut buf);
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

fn random_uuid_v4() -> String {
    let mut b = [0u8; 16];
    let _ = getrandom::fill(&mut b);
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]
    )
}

/// Scans an existing JSONL file to find whether it has a v3 `session` header
/// and what the latest entry `id` is (for chaining `parentId`).
fn scan_header_and_leaf(path: &Path) -> (bool, Option<String>) {
    let Ok(file) = fs::File::open(path) else {
        return (false, None);
    };
    let mut has_header = false;
    let mut leaf_id: Option<String> = None;
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(val) = serde_json::from_str::<Value>(trimmed) else {
            continue;
        };
        let kind = val.get("type").and_then(Value::as_str).unwrap_or("");
        if kind == "session" {
            has_header = true;
        } else if let Some(id) = val.get("id").and_then(Value::as_str)
            && !id.is_empty()
        {
            leaf_id = Some(id.to_string());
        }
    }
    (has_header, leaf_id)
}

/// Appends one user or assistant message in Pi's native v3 JSONL tree format
/// so that Nabla's core engine (`pi --session <path>`) can reload it verbatim.
pub fn append_pi_session_message(
    path: &Path,
    cwd: &Path,
    role: &str,
    text: &str,
    provider: &str,
    model: &str,
) -> Result<(), String> {
    if text.trim().is_empty() {
        return Ok(());
    }
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).map_err(|e| format!("failed to create session dir: {e}"))?;
    }

    let (has_header, leaf_id) = scan_header_and_leaf(path);
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| format!("failed to open session file {}: {e}", path.display()))?;

    let iso = now_iso_utc();
    if !has_header {
        let header = json!({
            "type": "session",
            "version": 3,
            "id": random_uuid_v4(),
            "timestamp": iso,
            "cwd": cwd.to_string_lossy(),
        });
        let line = serde_json::to_string(&header).map_err(|e| e.to_string())?;
        writeln!(file, "{line}").map_err(|e| e.to_string())?;
    }

    let entry_id = random_hex(4);
    let epoch_ms = now_epoch_millis();
    let message = if role == "assistant" {
        json!({
            "role": "assistant",
            "content": [{ "type": "text", "text": text }],
            "api": "acp",
            "provider": provider,
            "model": model,
            "usage": {
                "input": 0,
                "output": 0,
                "cacheRead": 0,
                "cacheWrite": 0,
                "totalTokens": 0,
                "cost": {
                    "input": 0,
                    "output": 0,
                    "cacheRead": 0,
                    "cacheWrite": 0,
                    "total": 0
                }
            },
            "stopReason": "stop",
            "timestamp": epoch_ms,
        })
    } else {
        json!({
            "role": "user",
            "content": [{ "type": "text", "text": text }],
            "timestamp": epoch_ms,
        })
    };

    let entry = json!({
        "type": "message",
        "id": entry_id,
        "parentId": leaf_id,
        "timestamp": iso,
        "message": message,
    });
    let line = serde_json::to_string(&entry).map_err(|e| e.to_string())?;
    writeln!(file, "{line}").map_err(|e| e.to_string())?;
    Ok(())
}

fn extract_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(parts) => {
            let mut out = Vec::new();
            for part in parts {
                if let Some(s) = part.as_str() {
                    out.push(s.to_string());
                } else if let Some(obj) = part.as_object()
                    && obj.get("type").and_then(Value::as_str).unwrap_or("text") == "text"
                    && let Some(t) = obj.get("text").and_then(Value::as_str)
                {
                    out.push(t.to_string());
                }
            }
            out.join("\n")
        }
        _ => String::new(),
    }
}

/// Reads user & assistant messages from a Nabla/Pi session JSONL file.
pub fn read_session_messages(path: &Path) -> Vec<SessionTurn> {
    let Ok(file) = fs::File::open(path) else {
        return Vec::new();
    };
    let mut turns = Vec::new();
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(val) = serde_json::from_str::<Value>(trimmed) else {
            continue;
        };

        // Pi v3 entry: `{ type: "message", timestamp, message: { role, content } }`
        let (msg_obj, ts) = if val.get("type").and_then(Value::as_str) == Some("message") {
            let Some(m) = val.get("message") else {
                continue;
            };
            let ts = val
                .get("timestamp")
                .and_then(Value::as_str)
                .map(str::to_string);
            (m, ts)
        } else if val.get("role").is_some() {
            let ts = val
                .get("timestamp")
                .and_then(Value::as_str)
                .map(str::to_string);
            (&val, ts)
        } else {
            continue;
        };

        let role = match msg_obj
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str()
        {
            "user" => "user",
            "assistant" => "assistant",
            _ => continue,
        };
        let text = msg_obj
            .get("content")
            .map(extract_text)
            .unwrap_or_default()
            .trim()
            .to_string();
        if text.is_empty() {
            continue;
        }
        turns.push(SessionTurn {
            index: turns.len() + 1,
            role: role.to_string(),
            text,
            timestamp: ts,
        });
    }
    turns
}

/// Builds a compact `<conversation_history>` handoff block from the tail of a
/// session's transcript so an ACP agent joining mid-conversation has immediate
/// continuity (and knows it can call `session_context` for older turns).
pub fn format_handoff_context(turns: &[SessionTurn], max_turns: usize, max_chars: usize) -> Option<String> {
    if turns.is_empty() {
        return None;
    }
    let total = turns.len();
    let start = total.saturating_sub(max_turns);
    let mut lines = Vec::new();
    let mut used = 0usize;

    for turn in turns[start..].iter().rev() {
        let label = if turn.role == "user" { "User" } else { "Assistant" };
        let entry = format!("[#{}] {}: {}", turn.index, label, turn.text);
        let entry_len = entry.len() + 1;
        if used + entry_len > max_chars && !lines.is_empty() {
            break;
        }
        used += entry_len;
        lines.push(entry);
    }
    lines.reverse();

    let omitted = total.saturating_sub(lines.len());
    let mut header = String::from(
        "This Nabla conversation already has prior turns (from another model in the same session). Continue seamlessly from this context.",
    );
    if omitted > 0 {
        header.push_str(&format!(
            " ({omitted} earlier message(s) omitted; call the `session_context` MCP tool if you need them.)"
        ));
    }
    Some(format!(
        "<conversation_history>\n{header}\n\n{}\n</conversation_history>",
        lines.join("\n\n")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_and_chains_pi_v3_session_entries() {
        let dir = std::env::temp_dir().join(format!("nabla-acp-transcript-{}", random_hex(4)));
        let path = dir.join("session.jsonl");
        let cwd = Path::new("/tmp/ws");

        append_pi_session_message(&path, cwd, "user", "Hola mundo", "acp:antigravity", "gemini-4")
            .unwrap();
        append_pi_session_message(
            &path,
            cwd,
            "assistant",
            "Hola, ¿en qué te ayudo?",
            "acp:antigravity",
            "gemini-4",
        )
        .unwrap();

        let raw = fs::read_to_string(&path).unwrap();
        let lines: Vec<Value> = raw
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0]["type"], "session");
        assert_eq!(lines[0]["version"], 3);
        assert_eq!(lines[1]["type"], "message");
        assert!(lines[1]["parentId"].is_null());
        assert_eq!(lines[2]["type"], "message");
        assert_eq!(lines[2]["parentId"], lines[1]["id"]);
        assert_eq!(lines[2]["message"]["role"], "assistant");

        let turns = read_session_messages(&path);
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].role, "user");
        assert_eq!(turns[0].text, "Hola mundo");
        assert_eq!(turns[1].role, "assistant");
        assert_eq!(turns[1].text, "Hola, ¿en qué te ayudo?");

        let handoff = format_handoff_context(&turns, 10, 4000).unwrap();
        assert!(handoff.contains("Hola mundo"));
        assert!(handoff.contains("Hola, ¿en qué te ayudo?"));

        let _ = fs::remove_dir_all(&dir);
    }
}
