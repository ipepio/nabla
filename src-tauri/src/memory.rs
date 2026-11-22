//! Persistent Agent Memory store (`memory.db`, Engram pattern).
//!
//! Architecture rule (AGENTS.md #4): agent memory lives in its own dedicated
//! SQLite database (`<data_dir>/memory.db`), completely separate from `nabla.db`
//! and exposed strictly behind the contracts seam (`commands.rs` + `mcp_server.rs`).

use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, params};

/// One durable memory record stored by the agent or the user.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryEntry {
    pub id: i64,
    /// `"global"` or a workspace-scoped label (e.g. `"workspace:/path"`).
    pub scope: String,
    /// `"preference"` | `"fact"` | `"decision"` | `"project"` | `"note"`.
    pub kind: String,
    pub title: String,
    pub content: String,
    /// Comma-separated tags for quick filtering and full-text search.
    pub tags: String,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Thread-safe handle to `<data_dir>/memory.db`.
pub struct MemoryStore {
    conn: Mutex<Connection>,
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn normalize_scope(raw: Option<&str>) -> String {
    let trimmed = raw.unwrap_or("").trim();
    if trimmed.is_empty() {
        "global".to_string()
    } else {
        trimmed.to_string()
    }
}

fn normalize_kind(raw: Option<&str>) -> String {
    let normalized = raw.unwrap_or("").trim().to_ascii_lowercase();
    match normalized.as_str() {
        "preference" | "fact" | "decision" | "project" | "note" => normalized,
        _ => "note".to_string(),
    }
}

fn normalize_tags(raw: Option<&str>) -> String {
    raw.unwrap_or("")
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Builds a safe FTS5 prefix query from arbitrary user/agent text.
fn build_fts_query(raw: &str) -> Option<String> {
    let terms: Vec<String> = raw
        .split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|term| format!("\"{}\"*", term.replace('"', "\"\"")))
        .collect();
    if terms.is_empty() {
        None
    } else {
        Some(terms.join(" AND "))
    }
}

impl MemoryStore {
    /// Opens (or initializes) `<data_dir>/memory.db` with WAL + FTS5 index.
    pub fn open(path: &Path) -> Result<Self, String> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("failed to create memory dir {}: {e}", parent.display()))?;
        }
        let conn = Connection::open(path)
            .map_err(|e| format!("failed to open memory db {}: {e}", path.display()))?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| format!("memory db WAL pragma failed: {e}"))?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS memories (
                id         INTEGER PRIMARY KEY AUTOINCREMENT,
                scope      TEXT    NOT NULL,
                kind       TEXT    NOT NULL,
                title      TEXT    NOT NULL,
                content    TEXT    NOT NULL,
                tags       TEXT    NOT NULL DEFAULT '',
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_memories_scope_updated
                ON memories (scope, updated_at DESC);
            CREATE VIRTUAL TABLE IF NOT EXISTS memories_fts USING fts5(
                title,
                content,
                tags,
                kind,
                scope,
                content='memories',
                content_rowid='id',
                tokenize='unicode61'
            );
            CREATE TRIGGER IF NOT EXISTS memories_ai AFTER INSERT ON memories BEGIN
                INSERT INTO memories_fts(rowid, title, content, tags, kind, scope)
                VALUES (new.id, new.title, new.content, new.tags, new.kind, new.scope);
            END;
            CREATE TRIGGER IF NOT EXISTS memories_ad AFTER DELETE ON memories BEGIN
                INSERT INTO memories_fts(memories_fts, rowid, title, content, tags, kind, scope)
                VALUES ('delete', old.id, old.title, old.content, old.tags, old.kind, old.scope);
            END;
            CREATE TRIGGER IF NOT EXISTS memories_au AFTER UPDATE ON memories BEGIN
                INSERT INTO memories_fts(memories_fts, rowid, title, content, tags, kind, scope)
                VALUES ('delete', old.id, old.title, old.content, old.tags, old.kind, old.scope);
                INSERT INTO memories_fts(rowid, title, content, tags, kind, scope)
                VALUES (new.id, new.title, new.content, new.tags, new.kind, new.scope);
            END;",
        )
        .map_err(|e| format!("memory db schema init failed: {e}"))?;

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Saves a memory. If a record with the same `(scope, title)` already exists,
    /// updates it in place (Engram upsert behavior) to prevent duplicates.
    pub fn save(
        &self,
        title: &str,
        content: &str,
        kind: Option<&str>,
        tags: Option<&str>,
        scope: Option<&str>,
    ) -> Result<MemoryEntry, String> {
        let title = title.trim();
        let content = content.trim();
        if title.is_empty() {
            return Err("memory title must not be empty".to_string());
        }
        if content.is_empty() {
            return Err("memory content must not be empty".to_string());
        }
        let scope = normalize_scope(scope);
        let kind = normalize_kind(kind);
        let tags = normalize_tags(tags);
        let now = now_secs();

        let conn = self
            .conn
            .lock()
            .map_err(|_| "memory db lock poisoned".to_string())?;

        let existing: Option<(i64, i64)> = conn
            .query_row(
                "SELECT id, created_at FROM memories WHERE scope = ?1 AND lower(title) = lower(?2) LIMIT 1",
                params![scope, title],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|e| format!("memory lookup failed: {e}"))?;

        if let Some((id, created_at)) = existing {
            conn.execute(
                "UPDATE memories SET kind = ?1, title = ?2, content = ?3, tags = ?4, updated_at = ?5 WHERE id = ?6",
                params![kind, title, content, tags, now, id],
            )
            .map_err(|e| format!("memory update failed: {e}"))?;
            Ok(MemoryEntry {
                id,
                scope,
                kind,
                title: title.to_string(),
                content: content.to_string(),
                tags,
                created_at,
                updated_at: now,
            })
        } else {
            conn.execute(
                "INSERT INTO memories (scope, kind, title, content, tags, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![scope, kind, title, content, tags, now, now],
            )
            .map_err(|e| format!("memory insert failed: {e}"))?;
            let id = conn.last_insert_rowid();
            Ok(MemoryEntry {
                id,
                scope,
                kind,
                title: title.to_string(),
                content: content.to_string(),
                tags,
                created_at: now,
                updated_at: now,
            })
        }
    }

    /// Lists or FTS5-searches memories, newest/most relevant first.
    pub fn search(
        &self,
        query: Option<&str>,
        scope_filter: Option<&str>,
        limit: usize,
    ) -> Result<Vec<MemoryEntry>, String> {
        let limit = limit.clamp(1, 100) as i64;
        let scope = scope_filter.map(str::trim).filter(|s| !s.is_empty());
        let fts = query.and_then(build_fts_query);

        let conn = self
            .conn
            .lock()
            .map_err(|_| "memory db lock poisoned".to_string())?;

        let map_row = |row: &rusqlite::Row<'_>| -> rusqlite::Result<MemoryEntry> {
            Ok(MemoryEntry {
                id: row.get(0)?,
                scope: row.get(1)?,
                kind: row.get(2)?,
                title: row.get(3)?,
                content: row.get(4)?,
                tags: row.get(5)?,
                created_at: row.get(6)?,
                updated_at: row.get(7)?,
            })
        };

        let mut rows = Vec::new();
        match (fts, scope) {
            (Some(q), Some(sc)) => {
                let mut stmt = conn
                    .prepare(
                        "SELECT m.id, m.scope, m.kind, m.title, m.content, m.tags, m.created_at, m.updated_at
                         FROM memories_fts f
                         JOIN memories m ON m.id = f.rowid
                         WHERE memories_fts MATCH ?1 AND m.scope = ?2
                         ORDER BY bm25(memories_fts), m.updated_at DESC
                         LIMIT ?3",
                    )
                    .map_err(|e| format!("memory search prepare failed: {e}"))?;
                let iter = stmt
                    .query_map(params![q, sc, limit], map_row)
                    .map_err(|e| format!("memory search failed: {e}"))?;
                for item in iter {
                    rows.push(item.map_err(|e| e.to_string())?);
                }
            }
            (Some(q), None) => {
                let mut stmt = conn
                    .prepare(
                        "SELECT m.id, m.scope, m.kind, m.title, m.content, m.tags, m.created_at, m.updated_at
                         FROM memories_fts f
                         JOIN memories m ON m.id = f.rowid
                         WHERE memories_fts MATCH ?1
                         ORDER BY bm25(memories_fts), m.updated_at DESC
                         LIMIT ?2",
                    )
                    .map_err(|e| format!("memory search prepare failed: {e}"))?;
                let iter = stmt
                    .query_map(params![q, limit], map_row)
                    .map_err(|e| format!("memory search failed: {e}"))?;
                for item in iter {
                    rows.push(item.map_err(|e| e.to_string())?);
                }
            }
            (None, Some(sc)) => {
                let mut stmt = conn
                    .prepare(
                        "SELECT id, scope, kind, title, content, tags, created_at, updated_at
                         FROM memories
                         WHERE scope = ?1
                         ORDER BY updated_at DESC, id DESC
                         LIMIT ?2",
                    )
                    .map_err(|e| format!("memory list prepare failed: {e}"))?;
                let iter = stmt
                    .query_map(params![sc, limit], map_row)
                    .map_err(|e| format!("memory list failed: {e}"))?;
                for item in iter {
                    rows.push(item.map_err(|e| e.to_string())?);
                }
            }
            (None, None) => {
                let mut stmt = conn
                    .prepare(
                        "SELECT id, scope, kind, title, content, tags, created_at, updated_at
                         FROM memories
                         ORDER BY updated_at DESC, id DESC
                         LIMIT ?1",
                    )
                    .map_err(|e| format!("memory list prepare failed: {e}"))?;
                let iter = stmt
                    .query_map(params![limit], map_row)
                    .map_err(|e| format!("memory list failed: {e}"))?;
                for item in iter {
                    rows.push(item.map_err(|e| e.to_string())?);
                }
            }
        }
        Ok(rows)
    }

    /// Deletes a single memory by id. Returns `true` if a row was removed.
    pub fn delete(&self, id: i64) -> Result<bool, String> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| "memory db lock poisoned".to_string())?;
        let affected = conn
            .execute("DELETE FROM memories WHERE id = ?1", params![id])
            .map_err(|e| format!("memory delete failed: {e}"))?;
        Ok(affected > 0)
    }

    /// Clears all memories, or only those matching `scope_filter`.
    pub fn clear(&self, scope_filter: Option<&str>) -> Result<usize, String> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| "memory db lock poisoned".to_string())?;
        let affected = match scope_filter.map(str::trim).filter(|s| !s.is_empty()) {
            Some(sc) => conn
                .execute("DELETE FROM memories WHERE scope = ?1", params![sc])
                .map_err(|e| format!("memory clear failed: {e}"))?,
            None => conn
                .execute("DELETE FROM memories", [])
                .map_err(|e| format!("memory clear failed: {e}"))?,
        };
        Ok(affected)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db(tag: &str) -> std::path::PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "nabla_memory_test_{}_{}_{tag}.db",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        path
    }

    #[test]
    fn save_upsert_and_fts_search() {
        let path = temp_db("fts");
        let store = MemoryStore::open(&path).expect("open memory store");

        let first = store
            .save(
                "Response language",
                "Always reply to the maintainer in Spanish",
                Some("preference"),
                Some("language, style"),
                None,
            )
            .expect("save first");
        assert_eq!(first.scope, "global");
        assert_eq!(first.kind, "preference");

        // Upsert by same scope + title (case-insensitive) updates in place.
        let updated = store
            .save(
                "response language",
                "Reply in Spanish, keep code in English",
                Some("preference"),
                Some("language, code"),
                None,
            )
            .expect("upsert");
        assert_eq!(updated.id, first.id);

        let hits = store
            .search(Some("Spanish"), None, 10)
            .expect("search Spanish");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].content, "Reply in Spanish, keep code in English");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn scope_filtering_delete_and_clear() {
        let path = temp_db("scope");
        let store = MemoryStore::open(&path).expect("open memory store");

        let g = store
            .save("Editor", "Prefers Notion-style blocks", Some("preference"), None, Some("global"))
            .expect("save global");
        let w = store
            .save("Build command", "Run pnpm run build", Some("project"), None, Some("workspace:/nabla"))
            .expect("save workspace");

        let only_ws = store
            .search(None, Some("workspace:/nabla"), 10)
            .expect("list ws");
        assert_eq!(only_ws.len(), 1);
        assert_eq!(only_ws[0].id, w.id);

        assert!(store.delete(g.id).expect("delete global"));
        assert!(!store.delete(g.id).expect("delete missing"));

        let cleared = store.clear(None).expect("clear all");
        assert_eq!(cleared, 1);
        assert!(store.search(None, None, 10).expect("empty").is_empty());

        let _ = std::fs::remove_file(&path);
    }
}
