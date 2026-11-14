// Store module: application-owned persistence (SQLite).
// Rules (see .docs/02-decisiones.md):
// - This module never stores conversation transcripts. pi owns session JSONL files;
//   nabla.db only indexes their metadata.
// - Workspaces and the session index live in SQLite; user-editable preferences
//   live in settings.json (T5).

use std::path::Path;
use std::sync::{Mutex, MutexGuard};

// Payloads cross the Tauri bridge as camelCase JSON: the same convention
// settings.json uses (see L3). The TS contracts mirror these shapes (S4).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Workspace {
    pub id: i64,
    pub path: String,
    pub name: String,
    pub created_at: String,
}

// Not a command payload (pi writes sessions; nabla only indexes them), but it is
// serialized anyway so the contract stays symmetric with SessionRow.
// Used by the upcoming pi-runtime feature: sessions are indexed when a pi
// session spawns (see .docs/03-referencias.md, per-session topology).
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionNew {
    pub workspace_id: i64,
    /// Absolute path of the pi-owned session JSONL file. Unique. Content is never stored here.
    pub pi_session_path: String,
    pub title: Option<String>,
    pub model: Option<String>,
    pub started_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRow {
    pub id: i64,
    pub workspace_id: i64,
    pub pi_session_path: String,
    pub title: Option<String>,
    pub model: Option<String>,
    pub started_at: String,
    pub updated_at: String,
}

#[derive(Debug)]
pub enum StoreError {
    Sqlite(String),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::Sqlite(msg) => write!(f, "store sqlite error: {msg}"),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<rusqlite::Error> for StoreError {
    fn from(e: rusqlite::Error) -> Self {
        StoreError::Sqlite(e.to_string())
    }
}

pub type StoreResult<T> = Result<T, StoreError>;

pub struct Store {
    /// rusqlite's Connection is `Send` but not `Sync`; the store is shared by
    /// every Tauri command (which run on worker threads), so access is
    /// serialized through a mutex exactly like a single-threaded connection
    /// would be.
    conn: Mutex<rusqlite::Connection>,
}

impl Store {
    /// Opens (creating if needed) the database and applies pending migrations.
    pub fn open(path: &Path) -> StoreResult<Store> {
        let conn = rusqlite::Connection::open(path)?;
        // Enforce FK cascade behavior on every connection, not just by default.
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        let store = Store {
            conn: Mutex::new(conn),
        };
        store.migrate()?;
        Ok(store)
    }

    /// Locks the connection for the duration of one operation. Any panic while
    /// holding the guard poisons the mutex, so a corrupted connection surfaces
    /// as an error instead of being silently reused.
    fn conn(&self) -> StoreResult<MutexGuard<'_, rusqlite::Connection>> {
        self.conn
            .lock()
            .map_err(|_| StoreError::Sqlite("connection mutex poisoned".to_string()))
    }

    /// Versioned migrations tracked by SQLite's PRAGMA user_version.
    /// Each migration is idempotent by definition: it runs only when its
    /// version equals the current user_version.
    fn migrate(&self) -> StoreResult<()> {
        let conn = self.conn()?;
        let current: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        const LATEST: i64 = 1;
        if current < 1 {
            // S1 schema: application-owned index only. No transcript/content
            // columns exist anywhere: pi owns the session files.
            conn.execute_batch(
                "BEGIN;
                 CREATE TABLE IF NOT EXISTS workspaces (
                   id INTEGER PRIMARY KEY AUTOINCREMENT,
                   path TEXT NOT NULL UNIQUE,
                   name TEXT NOT NULL,
                   created_at TEXT NOT NULL
                 );
                 CREATE TABLE IF NOT EXISTS sessions (
                   id INTEGER PRIMARY KEY AUTOINCREMENT,
                   workspace_id INTEGER NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
                   pi_session_path TEXT NOT NULL UNIQUE,
                   title TEXT,
                   model TEXT,
                   started_at TEXT NOT NULL,
                   updated_at TEXT NOT NULL
                 );
                 CREATE INDEX IF NOT EXISTS idx_sessions_workspace_updated
                   ON sessions(workspace_id, updated_at DESC);
                 PRAGMA user_version = 1;
                 COMMIT;",
            )?;
        }
        let _ = LATEST;
        Ok(())
    }

    pub fn add_workspace(&self, path: &str, name: Option<&str>) -> StoreResult<Workspace> {
        let name = match name {
            Some(n) if !n.trim().is_empty() => n.trim().to_string(),
            _ => default_name_from_path(path),
        };
        let created_at = now_iso();
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO workspaces (path, name, created_at) VALUES (?1, ?2, ?3)",
            rusqlite::params![path, name, created_at],
        )?;
        let id = conn.last_insert_rowid();
        Ok(Workspace {
            id,
            path: path.to_string(),
            name,
            created_at,
        })
    }

    pub fn list_workspaces(&self) -> StoreResult<Vec<Workspace>> {
        let conn = self.conn()?;
        let mut stmt =
            conn.prepare("SELECT id, path, name, created_at FROM workspaces ORDER BY created_at, id")?;
        let rows = stmt
            .query_map([], |row| {
                Ok(Workspace {
                    id: row.get(0)?,
                    path: row.get(1)?,
                    name: row.get(2)?,
                    created_at: row.get(3)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn remove_workspace(&self, id: i64) -> StoreResult<bool> {
        let conn = self.conn()?;
        let affected = conn.execute("DELETE FROM workspaces WHERE id = ?1", [id])?;
        Ok(affected > 0)
    }

    // Used by the upcoming pi-runtime feature (see SessionNew note above).
    #[allow(dead_code)]
    pub fn upsert_session(&self, session: &SessionNew) -> StoreResult<SessionRow> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO sessions (workspace_id, pi_session_path, title, model, started_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(pi_session_path) DO UPDATE SET
               workspace_id = excluded.workspace_id,
               title = excluded.title,
               model = excluded.model,
               updated_at = excluded.updated_at",
            rusqlite::params![
                session.workspace_id,
                session.pi_session_path,
                session.title,
                session.model,
                session.started_at,
                session.updated_at,
            ],
        )?;
        let row = conn.query_row(
            "SELECT id, workspace_id, pi_session_path, title, model, started_at, updated_at
             FROM sessions WHERE pi_session_path = ?1",
            [&session.pi_session_path],
            |row| {
                Ok(SessionRow {
                    id: row.get(0)?,
                    workspace_id: row.get(1)?,
                    pi_session_path: row.get(2)?,
                    title: row.get(3)?,
                    model: row.get(4)?,
                    started_at: row.get(5)?,
                    updated_at: row.get(6)?,
                })
            },
        )?;
        Ok(row)
    }

    pub fn list_sessions(&self, workspace_id: i64) -> StoreResult<Vec<SessionRow>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, workspace_id, pi_session_path, title, model, started_at, updated_at
             FROM sessions WHERE workspace_id = ?1
             ORDER BY updated_at DESC, id DESC",
        )?;
        let rows = stmt
            .query_map([workspace_id], |row| {
                Ok(SessionRow {
                    id: row.get(0)?,
                    workspace_id: row.get(1)?,
                    pi_session_path: row.get(2)?,
                    title: row.get(3)?,
                    model: row.get(4)?,
                    started_at: row.get(5)?,
                    updated_at: row.get(6)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }
}

/// Derives a workspace name from its path: the last non-empty segment.
fn default_name_from_path(path: &str) -> String {
    path.trim_end_matches('/')
        .rsplit('/')
        .find(|s| !s.is_empty())
        .unwrap_or(path)
        .to_string()
}

/// ISO-8601 UTC timestamp for row metadata.
fn now_iso() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Minimal UTC formatting without pulling an extra date crate: YYYY-MM-DDTHH:MM:SSZ.
    let days_since_epoch = secs / 86_400;
    let rem = secs % 86_400;
    let (h, mi, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let (y, mo, d) = civil_from_days(days_since_epoch as i64);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
}

/// Days-since-epoch to civil date (Howard Hinnant's algorithm).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDb(std::path::PathBuf);
    impl TempDb {
        fn new(tag: &str) -> Self {
            let mut p = std::env::temp_dir();
            p.push(format!(
                "nabla_test_{}_{}_{tag}.db",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            TempDb(p)
        }
    }
    impl Drop for TempDb {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    fn ws_new(store: &Store, path: &str) -> Workspace {
        store.add_workspace(path, None).expect("add workspace")
    }

    fn session_new(ws_id: i64, pi_path: &str) -> SessionNew {
        SessionNew {
            workspace_id: ws_id,
            pi_session_path: pi_path.to_string(),
            title: Some("first title".into()),
            model: Some("gpt-6.1-sol".into()),
            started_at: "2026-11-14T10:00:00Z".into(),
            updated_at: "2026-11-14T10:05:00Z".into(),
        }
    }

    #[test]
    fn open_applies_migrations_and_is_reusable() {
        let db = TempDb::new("migrations");
        let store = Store::open(&db.0).expect("open");
        // S1: workspaces are entities sessions reference.
        let ws = ws_new(&store, "/tmp/proj-a");
        assert!(ws.id > 0);
        assert_eq!(ws.path, "/tmp/proj-a");
        // Default name derived from path.
        assert_eq!(ws.name, "proj-a");
        // Durability: close and reopen, data survives.
        drop(store);
        let reopened = Store::open(&db.0).expect("reopen");
        let all = reopened.list_workspaces().expect("list");
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].path, "/tmp/proj-a");
    }

    #[test]
    fn workspace_path_is_unique() {
        let db = TempDb::new("ws_unique");
        let store = Store::open(&db.0).expect("open");
        ws_new(&store, "/tmp/proj-a");
        assert!(store.add_workspace("/tmp/proj-a", Some("dup")).is_err());
    }

    #[test]
    fn workspace_name_defaults_from_path_and_accepts_override() {
        let db = TempDb::new("ws_name");
        let store = Store::open(&db.0).expect("open");
        let ws = store
            .add_workspace("/tmp/deep/nested-dir", Some("Custom"))
            .expect("add");
        assert_eq!(ws.name, "Custom");
        let ws2 = ws_new(&store, "/tmp/deep/nested-dir-2");
        assert_eq!(ws2.name, "nested-dir-2");
    }

    #[test]
    fn session_upsert_inserts_then_updates_without_duplicating() {
        let db = TempDb::new("session_upsert");
        let store = Store::open(&db.0).expect("open");
        let ws = ws_new(&store, "/tmp/proj-a");

        let first = store
            .upsert_session(&session_new(ws.id, "/sessions/a.jsonl"))
            .expect("insert");
        assert!(first.id > 0);
        assert_eq!(first.title.as_deref(), Some("first title"));

        let mut update = session_new(ws.id, "/sessions/a.jsonl");
        update.title = Some("second title".into());
        update.updated_at = "2026-11-14T11:00:00Z".into();
        let second = store.upsert_session(&update).expect("upsert");
        assert_eq!(second.id, first.id, "same pi path must reuse the row");
        assert_eq!(second.title.as_deref(), Some("second title"));
        assert_eq!(second.updated_at, "2026-11-14T11:00:00Z");

        let listed = store.list_sessions(ws.id).expect("list");
        assert_eq!(listed.len(), 1, "upsert must not duplicate");
        assert_eq!(listed[0].title.as_deref(), Some("second title"));
    }

    #[test]
    fn sessions_are_scoped_to_their_workspace() {
        let db = TempDb::new("session_scope");
        let store = Store::open(&db.0).expect("open");
        let a = ws_new(&store, "/tmp/proj-a");
        let b = ws_new(&store, "/tmp/proj-b");
        store
            .upsert_session(&session_new(a.id, "/sessions/a1.jsonl"))
            .expect("insert a1");
        store
            .upsert_session(&session_new(b.id, "/sessions/b1.jsonl"))
            .expect("insert b1");

        let in_a = store.list_sessions(a.id).expect("list a");
        assert_eq!(in_a.len(), 1);
        assert_eq!(in_a[0].pi_session_path, "/sessions/a1.jsonl");
        let in_b = store.list_sessions(b.id).expect("list b");
        assert_eq!(in_b.len(), 1);
    }

    #[test]
    fn removing_workspace_cascades_its_session_index_rows() {
        let db = TempDb::new("ws_cascade");
        let store = Store::open(&db.0).expect("open");
        let ws = ws_new(&store, "/tmp/proj-a");
        store
            .upsert_session(&session_new(ws.id, "/sessions/a1.jsonl"))
            .expect("insert");
        assert!(store.remove_workspace(ws.id).expect("remove"));
        assert!(store.list_workspaces().expect("list").is_empty());
        assert!(store.list_sessions(ws.id).expect("list").is_empty());
        assert!(!store.remove_workspace(ws.id).expect("remove again"));
    }

    #[test]
    fn sessions_order_by_updated_at_desc() {
        let db = TempDb::new("session_order");
        let store = Store::open(&db.0).expect("open");
        let ws = ws_new(&store, "/tmp/proj-a");
        let mut older = session_new(ws.id, "/sessions/old.jsonl");
        older.updated_at = "2026-11-14T09:00:00Z".into();
        let mut newer = session_new(ws.id, "/sessions/new.jsonl");
        newer.updated_at = "2026-11-14T12:00:00Z".into();
        store.upsert_session(&older).expect("insert old");
        store.upsert_session(&newer).expect("insert new");
        let listed = store.list_sessions(ws.id).expect("list");
        assert_eq!(listed[0].pi_session_path, "/sessions/new.jsonl");
        assert_eq!(listed[1].pi_session_path, "/sessions/old.jsonl");
    }

    // S1 negative guard: the session index holds metadata only. The schema has no
    // transcript column, so this is enforced by design; this test pins the API
    // shape that makes storing content impossible via SessionNew/SessionRow.
    #[test]
    fn session_rows_carry_metadata_only() {
        let db = TempDb::new("metadata_only");
        let store = Store::open(&db.0).expect("open");
        let ws = ws_new(&store, "/tmp/proj-a");
        let row = store
            .upsert_session(&session_new(ws.id, "/sessions/a.jsonl"))
            .expect("insert");
        // Compiled-in proof: SessionRow fields are exactly these.
        let SessionRow {
            id: _,
            workspace_id: _,
            pi_session_path,
            title,
            model,
            started_at: _,
            updated_at: _,
        } = &row;
        assert!(pi_session_path.ends_with(".jsonl"));
        assert!(title.is_some() && model.is_some());
    }
}
