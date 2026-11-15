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
        Ok(())
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
        // Version 2 is the migration contract.
        let version: i64 = store
            .conn()
            .expect("conn")
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .expect("user_version");
        assert_eq!(version, 2);
        // A second open must not re-run the ALTER statements.
        drop(store);
        Store::open(&db.0).expect("reopen an already-migrated database");
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
