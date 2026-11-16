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
    /// S8: `pinned` drives the "Pinned" filter and the default list order.
    pub pinned: bool,
    /// S8: ISO-8601 timestamp of the last time the workspace was used, `None`
    /// until the first touch. It is the recency source for the "Recientes"
    /// filter, never a derived value.
    pub last_used_at: Option<String>,
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

/// S5: one persisted account badge — what the last probe said about a provider
/// and when it said it. `credential` is presence only: the credential itself
/// stays in pi's `auth.json` and never reaches this database.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderStatusRow {
    pub provider: String,
    /// The account state as the contract spells it (`ready`, `not_ready`,
    /// `invalid`, `unknown`). Stored as text, so a state this build does not
    /// know about can still be read back instead of being dropped.
    pub state: String,
    pub credential: bool,
    pub checked_at: String,
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
        if current < 2 {
            // v2 (S8): pin and recency support for the spaces sidebar. Both
            // columns are additive, so v1 rows keep their data and take the
            // defaults (unpinned, never used).
            conn.execute_batch(
                "BEGIN;
                 ALTER TABLE workspaces ADD COLUMN pinned INTEGER NOT NULL DEFAULT 0;
                 ALTER TABLE workspaces ADD COLUMN last_used_at TEXT;
                 PRAGMA user_version = 2;
                 COMMIT;",
            )?;
        }
        if current < 3 {
            // v3 (S5): the last known account state of each provider, so the
            // panel paints its badges from the database instead of spending
            // ~1.15 s per provider booting the CLI before it can show anything.
            // Additive: no existing table is altered.
            conn.execute_batch(
                "BEGIN;
                 CREATE TABLE IF NOT EXISTS provider_status (
                   provider TEXT PRIMARY KEY,
                   state TEXT NOT NULL,
                   credential INTEGER NOT NULL,
                   checked_at TEXT NOT NULL
                 );
                 PRAGMA user_version = 3;
                 COMMIT;",
            )?;
        }
        Ok(())
    }

    /// S5: records what a probe just answered for one provider, replacing the
    /// previous badge of that same provider and leaving every other row alone.
    /// One statement, so the row is inserted or rewritten without a second read.
    pub fn upsert_provider_status(
        &self,
        provider: &str,
        state: &str,
        credential: bool,
        checked_at: &str,
    ) -> StoreResult<()> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO provider_status (provider, state, credential, checked_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(provider) DO UPDATE SET
               state = excluded.state,
               credential = excluded.credential,
               checked_at = excluded.checked_at",
            rusqlite::params![provider, state, credential as i64, checked_at],
        )?;
        Ok(())
    }

    /// S5: every persisted badge, ordered by provider id. The UI maps badges by
    /// provider, so the order is a stable contract and not whatever the planner
    /// picks.
    pub fn list_provider_status(&self) -> StoreResult<Vec<ProviderStatusRow>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT provider, state, credential, checked_at FROM provider_status
             ORDER BY provider",
        )?;
        let rows = stmt
            .query_map([], provider_status_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn add_workspace(&self, path: &str, name: Option<&str>) -> StoreResult<Workspace> {
        let name = match name {
            Some(n) if !n.trim().is_empty() => n.trim().to_string(),
            _ => default_name_from_path(path),
        };
        let created_at = now_iso();
        let conn = self.conn()?;
        // S8: a new workspace starts unpinned and never used, so the default
        // order falls back to created_at for it.
        conn.execute(
            "INSERT INTO workspaces (path, name, created_at, pinned, last_used_at)
             VALUES (?1, ?2, ?3, 0, NULL)",
            rusqlite::params![path, name, created_at],
        )?;
        let id = conn.last_insert_rowid();
        Ok(Workspace {
            id,
            path: path.to_string(),
            name,
            created_at,
            pinned: false,
            last_used_at: None,
        })
    }

    pub fn list_workspaces(&self) -> StoreResult<Vec<Workspace>> {
        let conn = self.conn()?;
        // S8 default order: pinned spaces first, then the most recently used,
        // then creation order as the stable tie-break. `NULLS LAST` keeps a
        // never-used space below every used one instead of above it, which is
        // what a bare `DESC` would do.
        let mut stmt = conn.prepare(
            "SELECT id, path, name, created_at, pinned, last_used_at FROM workspaces
             ORDER BY pinned DESC, last_used_at DESC NULLS LAST, created_at, id",
        )?;
        let rows = stmt
            .query_map([], workspace_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// S8: pins or unpins one workspace. Returns whether a row changed, so the
    /// command layer can tell an unknown id from a successful update.
    pub fn set_workspace_pinned(&self, id: i64, pinned: bool) -> StoreResult<bool> {
        let conn = self.conn()?;
        let affected = conn.execute(
            "UPDATE workspaces SET pinned = ?2 WHERE id = ?1",
            rusqlite::params![id, pinned as i64],
        )?;
        Ok(affected > 0)
    }

    /// S8: records that the workspace was just used. Returns whether a row
    /// changed, so the command layer can tell an unknown id from a real touch.
    pub fn touch_workspace(&self, id: i64) -> StoreResult<bool> {
        let conn = self.conn()?;
        let affected = conn.execute(
            "UPDATE workspaces SET last_used_at = ?2 WHERE id = ?1",
            rusqlite::params![id, now_iso()],
        )?;
        Ok(affected > 0)
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

    /// Sets the human-readable alias of an indexed session (what the drawer
    /// shows). Returns whether a row changed. Only metadata: the transcript
    /// stays in pi's file.
    pub fn set_session_title(&self, id: i64, title: &str) -> StoreResult<bool> {
        let conn = self.conn()?;
        let affected = conn.execute(
            "UPDATE sessions SET title = ?1 WHERE id = ?2",
            rusqlite::params![title, id],
        )?;
        Ok(affected > 0)
    }

    /// Marks a session as used now, which is what the drawer sorts by
    /// (`updated_at DESC`). Metadata only: the transcript stays in pi's file.
    pub fn touch_session(&self, id: i64) -> StoreResult<bool> {
        let conn = self.conn()?;
        let affected = conn.execute(
            "UPDATE sessions SET updated_at = ?1 WHERE id = ?2",
            rusqlite::params![now_iso(), id],
        )?;
        Ok(affected > 0)
    }

    /// Looks one indexed session up by its row id, the runtime key of the pi
    /// process (S1). `None` means the id is not (or no longer) indexed: the
    /// command layer reports that instead of spawning anything for it.
    pub fn session_by_id(&self, id: i64) -> StoreResult<Option<SessionRow>> {
        use rusqlite::OptionalExtension;
        let conn = self.conn()?;
        let row = conn
            .query_row(
                "SELECT id, workspace_id, pi_session_path, title, model, started_at, updated_at
                 FROM sessions WHERE id = ?1",
                [id],
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
            )
            .optional()?;
        Ok(row)
    }

    /// Removes one session from the index. Metadata only: the transcript file
    /// belongs to pi, and the command layer archives it before calling this, so
    /// no file operation happens here. Returns whether a row changed, so an id
    /// that is not indexed is reported instead of passing as a silent success.
    pub fn delete_session(&self, id: i64) -> StoreResult<bool> {
        let conn = self.conn()?;
        let affected = conn.execute("DELETE FROM sessions WHERE id = ?1", [id])?;
        Ok(affected > 0)
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

/// Maps one `workspaces` row into its payload. Every workspace SELECT shares
/// this, so the column order lives in exactly one place.
fn workspace_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Workspace> {
    Ok(Workspace {
        id: row.get(0)?,
        path: row.get(1)?,
        name: row.get(2)?,
        created_at: row.get(3)?,
        pinned: row.get::<_, i64>(4)? != 0,
        last_used_at: row.get(5)?,
    })
}

/// Maps one `provider_status` row into its payload. SQLite stores no bool, so
/// the integer credential flag becomes a bool here, in one place.
fn provider_status_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ProviderStatusRow> {
    Ok(ProviderStatusRow {
        provider: row.get(0)?,
        state: row.get(1)?,
        credential: row.get::<_, i64>(2)? != 0,
        checked_at: row.get(3)?,
    })
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
            // WAL leaves `-wal`/`-shm` siblings behind once a connection has
            // written; removing all three keeps the system temp dir clean.
            let _ = std::fs::remove_file(&self.0);
            for suffix in ["-wal", "-shm"] {
                let mut sidecar = self.0.clone().into_os_string();
                sidecar.push(suffix);
                let _ = std::fs::remove_file(std::path::PathBuf::from(sidecar));
            }
        }
    }

    fn ws_new(store: &Store, path: &str) -> Workspace {
        store.add_workspace(path, None).expect("add workspace")
    }

    /// The schema version the connection carries right now.
    fn user_version(store: &Store) -> i64 {
        store
            .conn()
            .expect("conn")
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .expect("user_version")
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
        // S8: a new workspace starts unpinned and never used.
        assert!(!ws.pinned);
        assert_eq!(ws.last_used_at, None);
        // Durability: close and reopen, data survives.
        drop(store);
        let reopened = Store::open(&db.0).expect("reopen");
        let all = reopened.list_workspaces().expect("list");
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].path, "/tmp/proj-a");
        assert!(!all[0].pinned);
        assert_eq!(all[0].last_used_at, None);
    }

    // S8: the v2 migration adds pinned/last_used_at to an existing v1 database
    // without losing rows, and it is not re-run once the version is 2.
    #[test]
    fn open_upgrades_a_v1_database_in_place() {
        let db = TempDb::new("migrate_v1");
        {
            // Faithful v1 schema: this is exactly what shipped before v2.
            let conn = rusqlite::Connection::open(&db.0).expect("create v1 db");
            conn.execute_batch(
                "CREATE TABLE workspaces (
                   id INTEGER PRIMARY KEY AUTOINCREMENT,
                   path TEXT NOT NULL UNIQUE,
                   name TEXT NOT NULL,
                   created_at TEXT NOT NULL
                 );
                 CREATE TABLE sessions (
                   id INTEGER PRIMARY KEY AUTOINCREMENT,
                   workspace_id INTEGER NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
                   pi_session_path TEXT NOT NULL UNIQUE,
                   title TEXT,
                   model TEXT,
                   started_at TEXT NOT NULL,
                   updated_at TEXT NOT NULL
                 );
                 INSERT INTO workspaces (path, name, created_at)
                   VALUES ('/tmp/v1-proj', 'v1-proj', '2026-01-01T00:00:00Z');
                 PRAGMA user_version = 1;",
            )
            .expect("seed v1 schema");
        }

        let store = Store::open(&db.0).expect("upgrade v1 -> v2");
        let all = store.list_workspaces().expect("list");
        assert_eq!(all.len(), 1, "the v1 row must survive the migration");
        assert_eq!(all[0].name, "v1-proj");
        assert!(!all[0].pinned, "v1 rows default to unpinned");
        assert_eq!(all[0].last_used_at, None, "v1 rows have no last use");
        // The upgraded schema is fully usable, not just readable.
        let session = store
            .upsert_session(&session_new(all[0].id, "/sessions/v1.jsonl"))
            .expect("sessions still work after the upgrade");
        assert!(session.id > 0);
        // The migration chain runs to its head, so a v1 file leaves at v3 and
        // the cache table is present along the way.
        assert_eq!(user_version(&store), 3);
        assert!(store.list_provider_status().expect("list status").is_empty());
        // A second open must not re-run the ALTER statements.
        drop(store);
        Store::open(&db.0).expect("reopen an already-migrated database");
    }

    // S5: the v3 migration adds the provider-status cache to an existing v2
    // database and nothing else. Workspaces, sessions and their data must arrive
    // on the other side untouched.
    #[test]
    fn open_upgrades_a_v2_database_to_v3_keeping_workspaces_and_sessions() {
        let db = TempDb::new("migrate_v2");
        {
            // Faithful v2 schema: the v1 tables plus the pin/recency columns.
            let conn = rusqlite::Connection::open(&db.0).expect("create v2 db");
            conn.execute_batch(
                "CREATE TABLE workspaces (
                   id INTEGER PRIMARY KEY AUTOINCREMENT,
                   path TEXT NOT NULL UNIQUE,
                   name TEXT NOT NULL,
                   created_at TEXT NOT NULL,
                   pinned INTEGER NOT NULL DEFAULT 0,
                   last_used_at TEXT
                 );
                 CREATE TABLE sessions (
                   id INTEGER PRIMARY KEY AUTOINCREMENT,
                   workspace_id INTEGER NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
                   pi_session_path TEXT NOT NULL UNIQUE,
                   title TEXT,
                   model TEXT,
                   started_at TEXT NOT NULL,
                   updated_at TEXT NOT NULL
                 );
                 CREATE INDEX idx_sessions_workspace_updated
                   ON sessions(workspace_id, updated_at DESC);
                 INSERT INTO workspaces (path, name, created_at, pinned, last_used_at)
                   VALUES ('/tmp/v2-proj', 'v2-proj', '2026-02-02T00:00:00Z', 1, '2026-02-03T00:00:00Z');
                 INSERT INTO sessions
                   (workspace_id, pi_session_path, title, model, started_at, updated_at)
                   VALUES (1, '/sessions/v2.jsonl', 'kept', 'gpt-6.1-sol',
                           '2026-02-02T00:00:00Z', '2026-02-02T00:10:00Z');
                 PRAGMA user_version = 2;",
            )
            .expect("seed v2 schema");
        }

        let store = Store::open(&db.0).expect("upgrade v2 -> v3");

        let workspaces = store.list_workspaces().expect("list workspaces");
        assert_eq!(workspaces.len(), 1, "the v2 workspace must survive");
        assert!(workspaces[0].pinned, "v2 pin data must survive");
        assert_eq!(
            workspaces[0].last_used_at.as_deref(),
            Some("2026-02-03T00:00:00Z"),
            "v2 recency data must survive"
        );
        let sessions = store
            .list_sessions(workspaces[0].id)
            .expect("list sessions");
        assert_eq!(sessions.len(), 1, "the indexed v2 session must survive");
        assert_eq!(sessions[0].pi_session_path, "/sessions/v2.jsonl");

        // The new table starts empty, which is what paints "never checked".
        assert!(
            store.list_provider_status().expect("list status").is_empty(),
            "the upgraded cache has no rows until the first probe"
        );
        // Usable straight after the upgrade, not merely readable.
        store
            .upsert_provider_status("openai", "ready", true, "2026-02-03T00:00:00Z")
            .expect("the cache accepts a row after the upgrade");
        assert_eq!(store.list_provider_status().expect("list").len(), 1);

        assert_eq!(user_version(&store), 3, "v3 is the migration contract");
    }

    // S5: reopening an already-migrated database runs no migration again, so the
    // cache is a cache and not a table that is quietly rebuilt on every launch.
    #[test]
    fn provider_status_cache_is_untouched_when_the_database_is_reopened() {
        let db = TempDb::new("migrate_v3_idempotent");
        let store = Store::open(&db.0).expect("open");
        store
            .upsert_provider_status("anthropic", "not_ready", false, "2026-03-01T10:00:00Z")
            .expect("insert");
        drop(store);

        let reopened = Store::open(&db.0).expect("reopen a v3 database");
        assert_eq!(
            user_version(&reopened),
            3,
            "reopening must not change the schema version"
        );
        let rows = reopened.list_provider_status().expect("list");
        assert_eq!(
            rows,
            vec![ProviderStatusRow {
                provider: "anthropic".to_string(),
                state: "not_ready".to_string(),
                credential: false,
                checked_at: "2026-03-01T10:00:00Z".to_string(),
            }],
            "the persisted rows must be exactly what was written, once"
        );
    }

    // S5: one row per provider, rewritten in place by the next probe.
    #[test]
    fn upsert_provider_status_inserts_then_updates_without_duplicating() {
        let db = TempDb::new("status_upsert");
        let store = Store::open(&db.0).expect("open");

        store
            .upsert_provider_status("openai", "ready", true, "2026-03-01T10:00:00Z")
            .expect("insert");
        store
            .upsert_provider_status("openai", "invalid", false, "2026-03-01T11:00:00Z")
            .expect("update");

        let rows = store.list_provider_status().expect("list");
        assert_eq!(rows.len(), 1, "the same provider must reuse its row");
        assert_eq!(rows[0].state, "invalid");
        assert!(!rows[0].credential);
        assert_eq!(rows[0].checked_at, "2026-03-01T11:00:00Z");
    }

    // S5: the UI maps badges by provider id, so the list order is stable.
    #[test]
    fn list_provider_status_orders_by_provider_id() {
        let db = TempDb::new("status_order");
        let store = Store::open(&db.0).expect("open");
        for provider in ["openai-codex", "anthropic", "google", "openai"] {
            store
                .upsert_provider_status(provider, "ready", true, "2026-03-01T10:00:00Z")
                .expect("insert");
        }

        let providers: Vec<String> = store
            .list_provider_status()
            .expect("list")
            .into_iter()
            .map(|row| row.provider)
            .collect();
        assert_eq!(
            providers,
            vec!["anthropic", "google", "openai", "openai-codex"]
        );
    }

    // S5: SQLite has no bool, so the credential flag is a stored integer that
    // must come back as a bool in both directions.
    #[test]
    fn provider_status_round_trips_credential_as_bool() {
        let db = TempDb::new("status_credential");
        let store = Store::open(&db.0).expect("open");

        store
            .upsert_provider_status("openai", "ready", true, "2026-03-01T10:00:00Z")
            .expect("insert present");
        store
            .upsert_provider_status("google", "not_ready", false, "2026-03-01T10:00:00Z")
            .expect("insert absent");

        let rows = store.list_provider_status().expect("list");
        let read = |provider: &str| {
            rows.iter()
                .find(|row| row.provider == provider)
                .map(|row| row.credential)
                .expect("row for provider")
        };
        assert!(
            !read("google"),
            "an absent credential reads back as false"
        );
        assert!(
            read("openai"),
            "a present credential reads back as true"
        );

        store
            .upsert_provider_status("google", "ready", true, "2026-03-01T11:00:00Z")
            .expect("flip to present");
        store
            .upsert_provider_status("openai", "invalid", false, "2026-03-01T11:00:00Z")
            .expect("flip to absent");
        let rows = store.list_provider_status().expect("list");
        let read = |provider: &str| {
            rows.iter()
                .find(|row| row.provider == provider)
                .map(|row| row.credential)
                .expect("row for provider")
        };
        assert!(!read("openai"), "an absent credential reads back as false");
        assert!(read("google"));
    }

    // S5: a probe of one provider never rewrites the badge of another.
    #[test]
    fn upsert_provider_status_leaves_other_providers_untouched() {
        let db = TempDb::new("status_isolation");
        let store = Store::open(&db.0).expect("open");
        store
            .upsert_provider_status("openai", "ready", true, "2026-03-01T10:00:00Z")
            .expect("insert openai");

        store
            .upsert_provider_status("anthropic", "not_ready", false, "2026-03-01T12:00:00Z")
            .expect("insert anthropic");
        store
            .upsert_provider_status("anthropic", "unknown", false, "2026-03-01T13:00:00Z")
            .expect("update anthropic");

        let rows = store.list_provider_status().expect("list");
        assert_eq!(rows.len(), 2, "one row per provider");
        assert_eq!(
            rows[0],
            ProviderStatusRow {
                provider: "anthropic".to_string(),
                state: "unknown".to_string(),
                credential: false,
                checked_at: "2026-03-01T13:00:00Z".to_string(),
            }
        );
        assert_eq!(
            rows[1],
            ProviderStatusRow {
                provider: "openai".to_string(),
                state: "ready".to_string(),
                credential: true,
                checked_at: "2026-03-01T10:00:00Z".to_string(),
            },
            "the untouched provider keeps every field"
        );
    }

    // S8: pinning is a persisted flag, toggled in both directions.
    #[test]
    fn set_workspace_pinned_toggles_and_persists() {
        let db = TempDb::new("ws_pinned");
        let store = Store::open(&db.0).expect("open");
        let ws = ws_new(&store, "/tmp/proj-a");

        assert!(
            store.set_workspace_pinned(ws.id, true).expect("pin"),
            "pinning an existing row reports a change"
        );
        assert!(store.list_workspaces().expect("list")[0].pinned);

        // Persistence is a reopen, not a reload.
        drop(store);
        let reopened = Store::open(&db.0).expect("reopen");
        assert!(reopened.list_workspaces().expect("list")[0].pinned);

        assert!(
            reopened
                .set_workspace_pinned(ws.id, false)
                .expect("unpin"),
            "unpinning an existing row reports a change"
        );
        assert!(!reopened.list_workspaces().expect("list")[0].pinned);
        assert!(
            !reopened
                .set_workspace_pinned(9999, true)
                .expect("unknown id"),
            "an unknown id changes no row"
        );
    }

    // S8: touching records the moment of last use, once.
    #[test]
    fn touch_workspace_records_last_used_at() {
        let db = TempDb::new("ws_touch");
        let store = Store::open(&db.0).expect("open");
        let ws = ws_new(&store, "/tmp/proj-a");
        assert_eq!(store.list_workspaces().expect("list")[0].last_used_at, None);

        assert!(
            store.touch_workspace(ws.id).expect("touch"),
            "touching an existing row reports a change"
        );
        let touched = store.list_workspaces().expect("list")[0]
            .last_used_at
            .clone()
            .expect("a touched workspace has a last use");
        assert!(
            !touched.is_empty(),
            "last_used_at must be a real timestamp, got {touched:?}"
        );
        assert!(
            touched.contains('T') && touched.ends_with('Z'),
            "last_used_at is the store's ISO-8601 shape, got {touched:?}"
        );

        assert!(
            !store.touch_workspace(9999).expect("unknown id"),
            "an unknown id changes no row"
        );
    }

    // S8: the default list order is pinned first, then most recently used,
    // with never-used rows last and created_at/id as the stable tie-break.
    #[test]
    fn list_workspaces_orders_pinned_first_then_most_recently_used() {
        let db = TempDb::new("ws_order");
        let store = Store::open(&db.0).expect("open");
        let never_pinned = ws_new(&store, "/tmp/w-never-pinned");
        let used_pinned = ws_new(&store, "/tmp/w-used-pinned");
        // Never used at all: only its creation order decides where it lands.
        ws_new(&store, "/tmp/w-never-pinned-old");
        let never_pinned_old_used = ws_new(&store, "/tmp/w-never-pinned-old-used");
        let never_used_pinned = ws_new(&store, "/tmp/w-never-used-pinned");

        // Distinct use timestamps need more resolution than one wall clock
        // second, so the fixture sets them directly; everything is asserted
        // through the public list API afterwards.
        {
            let conn = store.conn().expect("conn");
            let set_used = |id: i64, at: &str| {
                conn.execute(
                    "UPDATE workspaces SET last_used_at = ?1 WHERE id = ?2",
                    rusqlite::params![at, id],
                )
                .expect("set last_used_at");
            };
            set_used(never_pinned_old_used.id, "2026-11-14T09:00:00Z");
            set_used(never_pinned.id, "2026-11-14T12:00:00Z");
            set_used(used_pinned.id, "2026-11-14T08:00:00Z");
        }
        store
            .set_workspace_pinned(used_pinned.id, true)
            .expect("pin used");
        store
            .set_workspace_pinned(never_used_pinned.id, true)
            .expect("pin never used");

        let order: Vec<String> = store
            .list_workspaces()
            .expect("list")
            .into_iter()
            .map(|workspace| workspace.path)
            .collect();
        assert_eq!(
            order,
            vec![
                // Pinned first, and within them the most recently used.
                "/tmp/w-used-pinned".to_string(),
                "/tmp/w-never-used-pinned".to_string(),
                // Unpinned next, most recent use first.
                "/tmp/w-never-pinned".to_string(),
                "/tmp/w-never-pinned-old-used".to_string(),
                // Never used last.
                "/tmp/w-never-pinned-old".to_string(),
            ]
        );
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

    // S1/S3: the runtime key is the session row id, so opening a session needs
    // a lookup by that id alone — no workspace in scope, and no row means no
    // process. The answer is metadata only, like every session read.
    #[test]
    fn touch_session_moves_it_to_the_top_of_the_list() {
        let db = TempDb::new("session_touch");
        let store = Store::open(&db.0).expect("open");
        let ws = ws_new(&store, "/tmp/proj-touch");
        let mut older = session_new(ws.id, "/sessions/older.jsonl");
        older.updated_at = "2026-11-14T09:00:00Z".into();
        let mut newer = session_new(ws.id, "/sessions/newer.jsonl");
        newer.updated_at = "2026-11-14T12:00:00Z".into();
        let old_row = store.upsert_session(&older).expect("insert old");
        store.upsert_session(&newer).expect("insert new");
        // Baseline: the later timestamp wins.
        assert_eq!(
            store.list_sessions(ws.id).expect("list")[0].pi_session_path,
            "/sessions/newer.jsonl"
        );
        // Touching the old one makes it the most recent activity, which is what
        // the drawer must show first even when the session predates the others.
        assert!(store.touch_session(old_row.id).expect("touch"));
        assert_eq!(
            store.list_sessions(ws.id).expect("list")[0].pi_session_path,
            "/sessions/older.jsonl"
        );
        assert!(!store.touch_session(9_999).expect("unknown is a no-op"));
    }

    #[test]
    fn set_session_title_persists_the_alias() {
        let db = TempDb::new("session_title");
        let store = Store::open(&db.0).expect("open");
        let ws = ws_new(&store, "/tmp/proj-title");
        let row = store
            .upsert_session(&session_new(ws.id, "/sessions/titled.jsonl"))
            .expect("insert");
        assert!(store
            .set_session_title(row.id, "Fix the drawer spacing")
            .expect("set"));
        let fetched = store
            .session_by_id(row.id)
            .expect("lookup")
            .expect("indexed row");
        assert_eq!(fetched.title.as_deref(), Some("Fix the drawer spacing"));
        // Unknown id: no row changed, and the caller learns it.
        assert!(!store.set_session_title(9_999, "ghost").expect("no-op"));
    }

    #[test]
    fn session_by_id_returns_the_indexed_row_and_none_for_an_unknown_id() {
        let db = TempDb::new("session_by_id");
        let store = Store::open(&db.0).expect("open");
        let ws = ws_new(&store, "/tmp/proj-a");
        let row = store
            .upsert_session(&session_new(ws.id, "/sessions/a.jsonl"))
            .expect("insert");

        let found = store
            .session_by_id(row.id)
            .expect("lookup")
            .expect("the indexed row is found by its id");
        assert_eq!(found, row, "the lookup answers the whole index row");
        assert_eq!(found.pi_session_path, "/sessions/a.jsonl");

        assert_eq!(store.session_by_id(row.id + 1).expect("lookup"), None);
        assert_eq!(store.session_by_id(0).expect("lookup"), None);
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

    // S1/S3: deleting a session removes its index row and nothing else. The row
    // is metadata only, so the transcript file pi owns is untouched here; the
    // command layer archives it before calling this.
    #[test]
    fn delete_session_removes_its_index_row_only() {
        let db = TempDb::new("session_delete");
        let store = Store::open(&db.0).expect("open");
        let ws = ws_new(&store, "/tmp/proj-a");
        let doomed = store
            .upsert_session(&session_new(ws.id, "/sessions/doomed.jsonl"))
            .expect("insert doomed");
        let kept = store
            .upsert_session(&session_new(ws.id, "/sessions/kept.jsonl"))
            .expect("insert kept");

        assert!(
            store.delete_session(doomed.id).expect("delete"),
            "deleting an indexed session reports the row it removed"
        );
        assert_eq!(
            store.session_by_id(doomed.id).expect("lookup"),
            None,
            "the deleted row is gone from the index"
        );
        let remaining = store.list_sessions(ws.id).expect("list");
        assert_eq!(remaining.len(), 1, "sibling sessions survive the delete");
        assert_eq!(remaining[0].id, kept.id);

        // Deleting again changes no row, and the caller learns it.
        assert!(!store.delete_session(doomed.id).expect("delete again"));
        assert!(!store.delete_session(9_999).expect("unknown id"));
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
