use std::path::{Path, PathBuf};

use rusqlite::Connection;

use crate::error::{AppError, AppResult};

#[derive(Clone, Debug)]
pub struct Store {
    db_path: PathBuf,
}

impl Store {
    pub fn initialize(app_data: &Path) -> AppResult<Self> {
        std::fs::create_dir_all(app_data)?;
        for name in ["uploads", "outputs", "templates", "logs"] {
            std::fs::create_dir_all(app_data.join(name))?;
        }

        let store = Self {
            db_path: app_data.join("dreampaper.sqlite"),
        };
        let conn = store.connection()?;
        conn.execute_batch(SCHEMA)?;
        Self::migrate(&conn)?;
        Self::run_fts5_smoke_test(&conn)?;
        Ok(store)
    }

    /// Version the schema past the `CREATE TABLE IF NOT EXISTS` baseline.
    ///
    /// `SCHEMA` is the v1 baseline and stays idempotent; anything newer runs
    /// here as a numbered step inside one transaction, so a failure leaves the
    /// old database untouched instead of half-migrated. `schema_version` is the
    /// only cursor: a step is applied exactly when the stored version is below
    /// its number, and the version is bumped in the same transaction.
    fn migrate(conn: &Connection) -> AppResult<()> {
        let current: i64 = conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'schema_version'",
                [],
                |row| row.get::<_, String>(0),
            )
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(1);
        for (version, sql) in MIGRATIONS {
            if current >= *version {
                continue;
            }
            conn.execute_batch("BEGIN IMMEDIATE;")?;
            let applied = conn.execute_batch(sql).and_then(|_| {
                conn.execute(
                    "INSERT OR REPLACE INTO meta(key, value) VALUES ('schema_version', ?1)",
                    [version.to_string()],
                )
                .map(|_| ())
            });
            match applied {
                Ok(()) => conn.execute_batch("COMMIT;")?,
                Err(error) => {
                    let _ = conn.execute_batch("ROLLBACK;");
                    return Err(AppError::with_detail(
                        "schema_migration_failed",
                        format!("数据库升级到版本 {version} 失败：{error}"),
                        serde_json::json!({ "from": current, "to": version }),
                    ));
                }
            }
        }
        Ok(())
    }

    pub fn schema_version(&self) -> AppResult<i64> {
        let conn = self.connection()?;
        let value: String = conn.query_row(
            "SELECT value FROM meta WHERE key = 'schema_version'",
            [],
            |row| row.get(0),
        )?;
        value
            .parse()
            .map_err(|_| AppError::new("schema_version_invalid", "schema_version 不是数字"))
    }

    pub fn connection(&self) -> AppResult<Connection> {
        let conn = Connection::open(&self.db_path)?;
        conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 5000;")?;
        Ok(conn)
    }

    fn run_fts5_smoke_test(conn: &Connection) -> AppResult<()> {
        conn.execute_batch(
            "CREATE VIRTUAL TABLE IF NOT EXISTS fts_probe USING fts5(body);\
             DELETE FROM fts_probe;\
             INSERT INTO fts_probe(body) VALUES ('dream paper sqlite fts5 probe');",
        )?;
        {
            let mut stmt =
                conn.prepare("SELECT rowid FROM fts_probe WHERE fts_probe MATCH 'dream'")?;
            let mut rows = stmt.query([])?;
            let _ = rows.next()?;
        }
        conn.execute_batch("DROP TABLE IF EXISTS fts_probe;")?;
        Ok(())
    }
}

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS meta(
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

INSERT OR IGNORE INTO meta(key, value) VALUES ('schema_version', '1');

CREATE TABLE IF NOT EXISTS config(
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS assets(
  id TEXT PRIMARY KEY,
  kind TEXT NOT NULL,
  filename TEXT NOT NULL,
  mime TEXT NOT NULL,
  path TEXT NOT NULL,
  bytes INTEGER NOT NULL,
  created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS template_packs(
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  version TEXT,
  source_path TEXT NOT NULL,
  created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS templates(
  id TEXT PRIMARY KEY,
  pack_id TEXT,
  source_id TEXT NOT NULL,
  kind TEXT NOT NULL,
  category TEXT,
  rounded_ratio TEXT,
  visual_intent TEXT NOT NULL,
  content_summary TEXT NOT NULL,
  image_path TEXT NOT NULL,
  created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS jobs(
  id TEXT PRIMARY KEY,
  mode TEXT NOT NULL,
  status TEXT NOT NULL,
  payload_json TEXT NOT NULL,
  result_json TEXT,
  error_json TEXT,
  message TEXT,
  stage TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS job_stages(
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  job_id TEXT NOT NULL,
  stage TEXT NOT NULL,
  message TEXT NOT NULL,
  status TEXT NOT NULL,
  created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS job_design_logs(
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  job_id TEXT NOT NULL,
  step TEXT NOT NULL,
  label TEXT NOT NULL,
  status TEXT NOT NULL,
  content TEXT NOT NULL,
  created_at TEXT NOT NULL,
  UNIQUE(job_id, step)
);

CREATE INDEX IF NOT EXISTS job_design_logs_job ON job_design_logs(job_id);

CREATE TABLE IF NOT EXISTS artifacts(
  id TEXT PRIMARY KEY,
  job_id TEXT,
  kind TEXT NOT NULL,
  path TEXT NOT NULL,
  mime TEXT,
  meta_json TEXT NOT NULL,
  created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS documents(
  id TEXT PRIMARY KEY,
  asset_id TEXT,
  title TEXT,
  parser TEXT NOT NULL,
  status TEXT NOT NULL,
  meta_json TEXT NOT NULL,
  created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS chunks(
  id TEXT PRIMARY KEY,
  document_id TEXT NOT NULL,
  ord INTEGER NOT NULL,
  heading TEXT,
  page INTEGER,
  start_char INTEGER,
  end_char INTEGER,
  text TEXT NOT NULL,
  meta_json TEXT NOT NULL
);

CREATE VIRTUAL TABLE IF NOT EXISTS chunks_fts USING fts5(
  title,
  heading,
  body
);
"#;

/// Numbered schema steps applied on top of the v1 baseline, in order.
///
/// Append only; never edit a shipped step. Each step must be safe to run on
/// a database that already has every earlier step applied.
const MIGRATIONS: &[(i64, &str)] = &[(2, MIGRATION_V2_WORKBENCH), (3, MIGRATION_V3_MEMORY)];

/// v2: the image workbench. Source snapshots are immutable and deduplicated by
/// content digest; projects reference them by id, and the JSON document on
/// disk — not this index — is the project's source of truth. Exports only
/// record where the user saved a PNG. Assets carry no manual refcount: an
/// asset is live while any project row points at it.
const MIGRATION_V2_WORKBENCH: &str = r#"
CREATE TABLE IF NOT EXISTS workbench_assets(
  id TEXT PRIMARY KEY,
  digest TEXT NOT NULL UNIQUE,
  filename TEXT NOT NULL,
  mime TEXT NOT NULL,
  path TEXT NOT NULL,
  bytes INTEGER NOT NULL,
  width INTEGER NOT NULL,
  height INTEGER NOT NULL,
  created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS workbench_projects(
  id TEXT PRIMARY KEY,
  asset_id TEXT NOT NULL REFERENCES workbench_assets(id),
  name TEXT NOT NULL,
  path TEXT NOT NULL,
  schema_version INTEGER NOT NULL,
  revision INTEGER NOT NULL,
  export_width INTEGER NOT NULL,
  export_height INTEGER NOT NULL,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS workbench_projects_asset ON workbench_projects(asset_id);

CREATE TABLE IF NOT EXISTS workbench_exports(
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL REFERENCES workbench_projects(id) ON DELETE CASCADE,
  filename TEXT NOT NULL,
  path TEXT NOT NULL,
  width INTEGER NOT NULL,
  height INTEGER NOT NULL,
  created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS workbench_exports_project ON workbench_exports(project_id);
"#;

/// v3: case memory. One row per job that reached the implement stage, holding
/// the design product and the user's rating. `memory_fts` indexes the task
/// brief rewritten as CJK bigrams (see `memory.rs`); it is external-content
/// free so the index text can differ from the stored brief.
const MIGRATION_V3_MEMORY: &str = r#"
CREATE TABLE IF NOT EXISTS memory_cases(
  rowid INTEGER PRIMARY KEY,
  job_id TEXT NOT NULL UNIQUE,
  mode TEXT NOT NULL,
  title TEXT NOT NULL,
  brief TEXT NOT NULL,
  design_json TEXT NOT NULL,
  rating TEXT,
  created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS memory_cases_mode ON memory_cases(mode);

CREATE VIRTUAL TABLE IF NOT EXISTS memory_fts USING fts5(body);
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialize_is_idempotent_and_survives_fts_probe() {
        let dir = std::env::temp_dir().join(format!(
            "dreampaper-store-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);

        Store::initialize(&dir).expect("首次初始化应成功");
        let store = Store::initialize(&dir).expect("重复初始化应成功");

        let conn = store.connection().expect("应能取到连接");
        let tables: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' \
                 AND name IN ('jobs','job_stages','assets','templates','documents','chunks')",
                [],
                |row| row.get(0),
            )
            .expect("应能查询 schema");
        assert_eq!(tables, 6, "建表不完整");

        let probe: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name = 'fts_probe'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(probe, 0, "fts_probe 未被清理");

        drop(conn);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A v1 database (baseline schema only, version row = 1) must come out of
    /// `initialize` at the latest version with the workbench tables present,
    /// and a second run must be a no-op.
    #[test]
    fn migrates_a_v1_database_forward_once() {
        let dir = std::env::temp_dir().join(format!(
            "dreampaper-store-migrate-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        {
            let conn = Connection::open(dir.join("dreampaper.sqlite")).unwrap();
            conn.execute_batch(SCHEMA).unwrap();
            let version: String = conn
                .query_row(
                    "SELECT value FROM meta WHERE key='schema_version'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(version, "1");
        }

        let store = Store::initialize(&dir).expect("升级应成功");
        assert_eq!(store.schema_version().unwrap(), 3);
        let conn = store.connection().unwrap();
        let tables: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name IN \
                 ('workbench_assets','workbench_projects','workbench_exports')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(tables, 3, "工作台表未建齐");
        let memory: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name IN ('memory_cases','memory_fts')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(memory, 2, "案例记忆表未建齐");
        drop(conn);

        let again = Store::initialize(&dir).expect("重复初始化应成功");
        assert_eq!(again.schema_version().unwrap(), 3);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
