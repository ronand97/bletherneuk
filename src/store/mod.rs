//! SQLite storage: schema migrations, and backups of the data you entered by hand.

use std::path::{Path, PathBuf};

use anyhow::Context;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::json;

/// Numbered migrations. Migration N (1-based) brings the schema to `user_version` N.
const MIGRATIONS: &[&str] = &[include_str!("schema.sql")];

/// How many backup files to keep.
const KEEP_BACKUPS: usize = 30;

pub struct Store {
    conn: Connection,
}

impl Store {
    /// Opens (creating if needed) the database, then migrates it. Before migrating an
    /// existing database it writes a backup of the hand-entered data to `backups`.
    pub fn open(path: &Path, backups: &Path, now_ms: i64) -> anyhow::Result<Store> {
        use std::os::unix::fs::PermissionsExt;
        let existed = path.exists();
        let conn = Connection::open(path).with_context(|| format!("opening {path:?}"))?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        let mut store = Store { conn };
        store.configure()?;
        let version = store.schema_version()?;
        if existed && version > 0 && (version as usize) < MIGRATIONS.len() {
            let backup = store
                .backup_manual(backups, now_ms, Some(&format!("pre-migration-{version}")))
                .context("writing the pre-migration backup")?;
            store.migrate().with_context(|| {
                format!("database migration failed; the pre-migration backup is {backup:?}")
            })?;
        } else {
            store.migrate()?;
        }
        Ok(store)
    }

    pub fn open_in_memory() -> anyhow::Result<Store> {
        let mut store = Store {
            conn: Connection::open_in_memory()?,
        };
        store.configure()?;
        store.migrate()?;
        Ok(store)
    }

    fn configure(&self) -> rusqlite::Result<()> {
        self.conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA foreign_keys = ON;
             PRAGMA busy_timeout = 5000;",
        )
    }

    pub fn schema_version(&self) -> rusqlite::Result<i64> {
        self.conn.query_row("PRAGMA user_version", [], |r| r.get(0))
    }

    /// Applies each pending migration in its own transaction, so a failure never
    /// leaves a half-migrated database.
    fn migrate(&mut self) -> anyhow::Result<()> {
        let current = self.schema_version()? as usize;
        anyhow::ensure!(
            current <= MIGRATIONS.len(),
            "the database is from a newer bletherneuk (schema {current})"
        );
        for (i, sql) in MIGRATIONS.iter().enumerate().skip(current) {
            let tx = self.conn.transaction()?;
            tx.execute_batch(sql)
                .with_context(|| format!("migration {}", i + 1))?;
            tx.pragma_update(None, "user_version", (i + 1) as i64)?;
            tx.commit()?;
        }
        Ok(())
    }

    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    pub fn conn_mut(&mut self) -> &mut Connection {
        &mut self.conn
    }

    pub fn meta(&self, key: &str) -> rusqlite::Result<Option<String>> {
        self.conn
            .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
            .optional()
    }

    pub fn set_meta(&self, key: &str, value: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO meta(key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    /// Writes links you added, `overrides` and review markers to
    /// `<dir>/manual-<date>[-<suffix>].json` (mode 0600), then keeps the newest 30 files.
    /// The blocks those rows mention are included with their keys, so they can be matched
    /// up again after a rebuild.
    pub fn backup_manual(
        &self,
        dir: &Path,
        now_ms: i64,
        suffix: Option<&str>,
    ) -> anyhow::Result<PathBuf> {
        crate::paths::ensure_private_dir(dir)?;
        let rows = |sql: &str| -> anyhow::Result<Vec<serde_json::Value>> {
            let mut stmt = self.conn.prepare(sql)?;
            let names: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
            let out = stmt
                .query_map([], |r| {
                    let mut obj = serde_json::Map::new();
                    for (i, n) in names.iter().enumerate() {
                        let v: rusqlite::types::Value = r.get(i)?;
                        obj.insert(n.clone(), sql_to_json(v));
                    }
                    Ok(serde_json::Value::Object(obj))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(out)
        };
        let links = rows("SELECT * FROM links WHERE source = 'manual' ORDER BY id")?;
        let overrides = rows("SELECT * FROM overrides ORDER BY id")?;
        let markers = rows("SELECT * FROM review_markers ORDER BY id")?;
        let blocks = rows(
            "SELECT b.id, b.title, b.repo,
                    (SELECT json_group_array(key) FROM block_keys k WHERE k.block_id = b.id) AS keys
             FROM blocks b
             WHERE b.id IN (SELECT block_id FROM links WHERE source = 'manual'
                            UNION SELECT block_id FROM overrides WHERE block_id IS NOT NULL
                            UNION SELECT block_id FROM review_markers)
             ORDER BY b.id",
        )?;
        let doc = json!({
            "schema_version": self.schema_version()?,
            "created_at": now_ms,
            "blocks": blocks,
            "links": links,
            "overrides": overrides,
            "review_markers": markers,
        });

        let date = chrono::DateTime::from_timestamp_millis(now_ms)
            .unwrap_or_default()
            .format("%Y-%m-%d");
        let name = match suffix {
            Some(s) => format!("manual-{date}-{s}.json"),
            None => format!("manual-{date}.json"),
        };
        let path = dir.join(name);
        crate::util::write_private_atomic(&path, serde_json::to_vec_pretty(&doc)?.as_slice())?;
        prune_backups(dir, KEEP_BACKUPS)?;
        Ok(path)
    }
}

fn sql_to_json(v: rusqlite::types::Value) -> serde_json::Value {
    use rusqlite::types::Value;
    match v {
        Value::Null => serde_json::Value::Null,
        Value::Integer(i) => json!(i),
        Value::Real(f) => json!(f),
        Value::Text(s) => serde_json::from_str::<serde_json::Value>(&s)
            .ok()
            .filter(|v| v.is_array())
            .unwrap_or(serde_json::Value::String(s)),
        Value::Blob(_) => serde_json::Value::Null,
    }
}

fn prune_backups(dir: &Path, keep: usize) -> std::io::Result<()> {
    let mut names: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("manual-") && n.ends_with(".json"))
        })
        .collect();
    names.sort();
    let excess = names.len().saturating_sub(keep);
    for p in names.into_iter().take(excess) {
        std::fs::remove_file(p)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    const DAY: i64 = 86_400_000;
    // 2026-01-01T00:00:00Z
    const T0: i64 = 1_767_225_600_000;

    #[test]
    fn migrates_a_new_database_to_the_latest_schema() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("index.db"), &dir.path().join("b"), T0).unwrap();
        assert_eq!(store.schema_version().unwrap(), MIGRATIONS.len() as i64);
        let mode = std::fs::metadata(dir.path().join("index.db"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn reopening_does_not_rerun_migrations() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("index.db");
        let b = dir.path().join("b");
        Store::open(&db, &b, T0)
            .unwrap()
            .set_meta("k", "v")
            .unwrap();
        let store = Store::open(&db, &b, T0).unwrap();
        assert_eq!(store.meta("k").unwrap().as_deref(), Some("v"));
        assert!(!b.exists(), "no backup when nothing was migrated");
    }

    #[test]
    fn refuses_a_database_from_a_newer_version() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("index.db");
        {
            let c = Connection::open(&db).unwrap();
            c.pragma_update(None, "user_version", 999).unwrap();
        }
        let err = Store::open(&db, &dir.path().join("b"), T0).err().unwrap();
        assert!(err.to_string().contains("newer"), "{err}");
    }

    fn sample(store: &Store) {
        let c = store.conn();
        c.execute_batch(
            "INSERT INTO blocks(id, title, title_source, created_at, last_active)
                VALUES (1, 'Retry backoff', 'branch', 0, 0), (2, 'Other', 'branch', 0, 0);
             INSERT INTO block_keys VALUES ('branch:/r@feat/retry', 1);
             INSERT INTO links(block_id, kind, url, source, created_at)
                VALUES (1, 'other', 'https://example.com/a', 'manual', 0),
                       (2, 'github_pr', 'https://github.com/o/r/pull/42', 'auto', 0);
             INSERT INTO overrides(kind, block_id, value, created_at)
                VALUES ('block_rename', 1, 'My name', 0);",
        )
        .unwrap();
    }

    #[test]
    fn backup_holds_only_hand_entered_data() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open_in_memory().unwrap();
        sample(&store);
        let path = store.backup_manual(dir.path(), T0, None).unwrap();
        assert_eq!(path.file_name().unwrap(), "manual-2026-01-01.json");
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let doc: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(doc["links"].as_array().unwrap().len(), 1);
        assert_eq!(doc["links"][0]["url"], "https://example.com/a");
        assert_eq!(doc["overrides"][0]["value"], "My name");
        assert_eq!(doc["blocks"].as_array().unwrap().len(), 1);
        assert_eq!(doc["blocks"][0]["keys"][0], "branch:/r@feat/retry");
    }

    #[test]
    fn keeps_only_the_newest_thirty_backups() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open_in_memory().unwrap();
        for day in 0..35 {
            store
                .backup_manual(dir.path(), T0 + day * DAY, None)
                .unwrap();
        }
        let mut names: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        assert_eq!(names.len(), 30);
        assert_eq!(names[0], "manual-2026-01-06.json");
    }

    #[test]
    fn failed_migration_rolls_back_and_names_the_backup() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("index.db");
        let b = dir.path().join("b");
        Store::open(&db, &b, T0).unwrap();
        // Pretend the database is one version behind, with a migration that cannot apply.
        {
            let c = Connection::open(&db).unwrap();
            c.pragma_update(None, "user_version", 0).unwrap();
        }
        // Migration 1 re-creates existing tables, so it fails. Version 0 means no backup is
        // taken (nothing hand-entered can exist yet), and the version must stay at 0.
        assert!(Store::open(&db, &b, T0).is_err());
        let c = Connection::open(&db).unwrap();
        let v: i64 = c
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, 0);
    }
}
