use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, TransactionBehavior};
use thiserror::Error;

const CURRENT_SCHEMA_VERSION: u32 = 1;
const MIGRATION_V1_SQL: &str = "
CREATE TABLE IF NOT EXISTS schema_bootstrap (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

INSERT OR IGNORE INTO schema_bootstrap (key, value)
VALUES ('storage_bootstrap', 'initialized');

CREATE TABLE IF NOT EXISTS users (
  user_id INTEGER PRIMARY KEY,
  username TEXT,
  display_name TEXT,
  first_seen_at TEXT NOT NULL,
  last_seen_at TEXT NOT NULL,
  warn_count INTEGER NOT NULL DEFAULT 0,
  shadowbanned INTEGER NOT NULL DEFAULT 0,
  reputation INTEGER NOT NULL DEFAULT 0,
  state_json TEXT,
  updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS kv_store (
  scope_kind TEXT NOT NULL,
  scope_id TEXT NOT NULL,
  key TEXT NOT NULL,
  value_json TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  PRIMARY KEY (scope_kind, scope_id, key)
);

CREATE TABLE IF NOT EXISTS message_journal (
  chat_id INTEGER NOT NULL,
  message_id INTEGER NOT NULL,
  user_id INTEGER,
  date_utc TEXT NOT NULL,
  update_type TEXT NOT NULL,
  text TEXT,
  normalized_text TEXT,
  has_media INTEGER NOT NULL DEFAULT 0,
  reply_to_message_id INTEGER,
  file_ids_json TEXT,
  meta_json TEXT,
  PRIMARY KEY (chat_id, message_id)
);

CREATE INDEX IF NOT EXISTS idx_msg_chat_date
ON message_journal(chat_id, date_utc);

CREATE INDEX IF NOT EXISTS idx_msg_chat_user_date
ON message_journal(chat_id, user_id, date_utc);

CREATE INDEX IF NOT EXISTS idx_msg_chat_reply
ON message_journal(chat_id, reply_to_message_id);

CREATE TABLE IF NOT EXISTS jobs (
  job_id TEXT PRIMARY KEY,
  executor_unit TEXT NOT NULL,
  run_at TEXT NOT NULL,
  scheduled_at TEXT NOT NULL,
  status TEXT NOT NULL,
  dedupe_key TEXT,
  payload_json TEXT NOT NULL,
  retry_count INTEGER NOT NULL DEFAULT 0,
  max_retries INTEGER NOT NULL DEFAULT 0,
  last_error_code TEXT,
  last_error_text TEXT,
  audit_action_id TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS audit_log (
  action_id TEXT PRIMARY KEY,
  trace_id TEXT,
  request_id TEXT,
  unit_name TEXT NOT NULL,
  execution_mode TEXT NOT NULL,
  op TEXT NOT NULL,
  actor_user_id INTEGER,
  chat_id INTEGER,
  target_kind TEXT,
  target_id TEXT,
  trigger_message_id INTEGER,
  idempotency_key TEXT,
  reversible INTEGER NOT NULL DEFAULT 0,
  compensation_json TEXT,
  args_json TEXT NOT NULL,
  result_json TEXT,
  created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS processed_updates (
  update_id INTEGER PRIMARY KEY,
  event_id TEXT NOT NULL,
  processed_at TEXT NOT NULL,
  execution_mode TEXT NOT NULL
);

PRAGMA user_version = 1;
";

#[derive(Debug, Clone)]
pub struct Storage {
    database_path: PathBuf,
    config: StorageConfig,
}

impl Storage {
    pub fn new(database_path: PathBuf) -> Self {
        Self {
            database_path,
            config: StorageConfig::default(),
        }
    }

    pub fn with_config(database_path: PathBuf, config: StorageConfig) -> Self {
        Self {
            database_path,
            config,
        }
    }

    pub fn database_path(&self) -> &Path {
        &self.database_path
    }

    pub fn config(&self) -> &StorageConfig {
        &self.config
    }

    pub fn open(&self) -> Result<StorageConnection, StorageError> {
        StorageConnection::open(self.database_path.clone(), self.config.clone())
    }

    pub fn init(&self) -> Result<StorageConnection, StorageError> {
        let mut connection = self.open()?;
        connection.init_schema()?;
        Ok(connection)
    }

    pub fn bootstrap(&self) -> Result<StorageBootstrap, StorageError> {
        let mut connection = self.open()?;
        let migration = connection.init_schema()?;

        Ok(StorageBootstrap {
            connection,
            migration,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageConfig {
    pub busy_timeout: Duration,
    pub journal_mode: JournalMode,
    pub synchronous: SynchronousMode,
    pub temp_store: TempStoreMode,
    pub foreign_keys: bool,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            busy_timeout: Duration::from_secs(5),
            journal_mode: JournalMode::Wal,
            synchronous: SynchronousMode::Normal,
            temp_store: TempStoreMode::Memory,
            foreign_keys: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JournalMode {
    Delete,
    Wal,
}

impl JournalMode {
    fn as_sql(self) -> &'static str {
        match self {
            Self::Delete => "DELETE",
            Self::Wal => "WAL",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SynchronousMode {
    Off,
    Normal,
    Full,
}

impl SynchronousMode {
    fn as_sql(self) -> &'static str {
        match self {
            Self::Off => "OFF",
            Self::Normal => "NORMAL",
            Self::Full => "FULL",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TempStoreMode {
    Default,
    File,
    Memory,
}

impl TempStoreMode {
    fn as_sql(self) -> &'static str {
        match self {
            Self::Default => "DEFAULT",
            Self::File => "FILE",
            Self::Memory => "MEMORY",
        }
    }
}

#[derive(Debug)]
pub struct StorageBootstrap {
    connection: StorageConnection,
    migration: MigrationResult,
}

impl StorageBootstrap {
    pub fn connection(&self) -> &StorageConnection {
        &self.connection
    }

    pub fn connection_mut(&mut self) -> &mut StorageConnection {
        &mut self.connection
    }

    pub fn into_connection(self) -> StorageConnection {
        self.connection
    }

    pub fn migration(&self) -> &MigrationResult {
        &self.migration
    }
}

#[derive(Debug)]
pub struct StorageConnection {
    path: PathBuf,
    config: StorageConfig,
    connection: Connection,
}

impl StorageConnection {
    pub fn open(path: PathBuf, config: StorageConfig) -> Result<Self, StorageError> {
        ensure_parent_dir(&path)?;

        let connection = Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        connection.busy_timeout(config.busy_timeout)?;

        let mut storage = Self {
            path,
            config,
            connection,
        };
        storage.apply_connection_pragmas()?;

        Ok(storage)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn config(&self) -> &StorageConfig {
        &self.config
    }

    pub fn connection(&self) -> &Connection {
        &self.connection
    }

    pub fn current_schema_version(&self) -> Result<u32, StorageError> {
        Ok(read_user_version(&self.connection)?)
    }

    pub fn init_schema(&mut self) -> Result<MigrationResult, StorageError> {
        let current_version = self.current_schema_version()?;

        if current_version > CURRENT_SCHEMA_VERSION {
            return Err(StorageError::UnsupportedSchemaVersion {
                found: current_version,
                supported: CURRENT_SCHEMA_VERSION,
            });
        }

        let mut applied_versions = Vec::new();
        if current_version < 1 {
            self.apply_migration_v1()?;
            applied_versions.push(1);
        }

        let final_version = self.current_schema_version()?;

        Ok(MigrationResult {
            previous_version: current_version,
            current_version: final_version,
            applied_versions,
        })
    }

    fn apply_connection_pragmas(&mut self) -> Result<(), StorageError> {
        self.connection
            .pragma_update(None, "journal_mode", self.config.journal_mode.as_sql())?;
        self.connection
            .pragma_update(None, "synchronous", self.config.synchronous.as_sql())?;
        self.connection
            .pragma_update(None, "temp_store", self.config.temp_store.as_sql())?;
        self.connection.pragma_update(
            None,
            "foreign_keys",
            if self.config.foreign_keys {
                "ON"
            } else {
                "OFF"
            },
        )?;

        Ok(())
    }

    fn apply_migration_v1(&mut self) -> Result<(), StorageError> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;

        tx.execute_batch(MIGRATION_V1_SQL)?;
        tx.commit()?;

        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationResult {
    pub previous_version: u32,
    pub current_version: u32,
    pub applied_versions: Vec<u32>,
}

impl MigrationResult {
    pub fn changed(&self) -> bool {
        !self.applied_versions.is_empty()
    }
}

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("failed to create storage directory `{path}`")]
    CreateDirectory {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("sqlite error")]
    Sqlite(#[from] rusqlite::Error),
    #[error("unsupported schema version {found}; supported up to {supported}")]
    UnsupportedSchemaVersion { found: u32, supported: u32 },
}

fn ensure_parent_dir(path: &Path) -> Result<(), StorageError> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent).map_err(|source| StorageError::CreateDirectory {
            path: parent.to_path_buf(),
            source,
        })?;
    }

    Ok(())
}

fn read_user_version(connection: &Connection) -> rusqlite::Result<u32> {
    connection.pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
}

#[cfg(test)]
mod tests {
    use super::{
        JournalMode, Storage, StorageConfig, SynchronousMode, TempStoreMode, CURRENT_SCHEMA_VERSION,
    };
    use std::collections::BTreeSet;

    use rusqlite::params;
    use tempfile::tempdir;

    #[test]
    fn open_applies_sqlite_pragmas() {
        let dir = tempdir().unwrap_or_else(|error| panic!("failed to create tempdir: {error}"));
        let database_path = dir.path().join("runtime.sqlite3");
        let storage = Storage::with_config(
            database_path,
            StorageConfig {
                busy_timeout: std::time::Duration::from_secs(2),
                journal_mode: JournalMode::Wal,
                synchronous: SynchronousMode::Full,
                temp_store: TempStoreMode::Memory,
                foreign_keys: true,
            },
        );

        let connection = storage
            .open()
            .unwrap_or_else(|error| panic!("failed to open storage connection: {error}"));

        let journal_mode: String = connection
            .connection()
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .unwrap_or_else(|error| panic!("failed to read journal_mode pragma: {error}"));
        let synchronous: u32 = connection
            .connection()
            .pragma_query_value(None, "synchronous", |row| row.get(0))
            .unwrap_or_else(|error| panic!("failed to read synchronous pragma: {error}"));
        let foreign_keys: u32 = connection
            .connection()
            .pragma_query_value(None, "foreign_keys", |row| row.get(0))
            .unwrap_or_else(|error| panic!("failed to read foreign_keys pragma: {error}"));
        let temp_store: u32 = connection
            .connection()
            .pragma_query_value(None, "temp_store", |row| row.get(0))
            .unwrap_or_else(|error| panic!("failed to read temp_store pragma: {error}"));

        assert_eq!(journal_mode.to_ascii_lowercase(), "wal");
        assert_eq!(synchronous, 2);
        assert_eq!(foreign_keys, 1);
        assert_eq!(temp_store, 2);
    }

    #[test]
    fn bootstrap_initializes_schema_version_once() {
        let dir = tempdir().unwrap_or_else(|error| panic!("failed to create tempdir: {error}"));
        let database_path = dir.path().join("runtime.sqlite3");
        let storage = Storage::new(database_path);

        let bootstrap = storage
            .bootstrap()
            .unwrap_or_else(|error| panic!("failed to bootstrap storage: {error}"));

        assert_eq!(bootstrap.migration().previous_version, 0);
        assert_eq!(
            bootstrap.migration().current_version,
            CURRENT_SCHEMA_VERSION
        );
        assert_eq!(bootstrap.migration().applied_versions, vec![1]);
        assert!(bootstrap.migration().changed());

        let row_count: u32 = bootstrap
            .connection()
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM schema_bootstrap WHERE key = 'storage_bootstrap'",
                [],
                |row| row.get(0),
            )
            .unwrap_or_else(|error| panic!("failed to query schema_bootstrap: {error}"));
        assert_eq!(row_count, 1);

        let tables = sqlite_objects(bootstrap.connection().connection(), "table");
        assert!(tables.contains("schema_bootstrap"));
        assert!(tables.contains("users"));
        assert!(tables.contains("kv_store"));
        assert!(tables.contains("message_journal"));
        assert!(tables.contains("jobs"));
        assert!(tables.contains("audit_log"));
        assert!(tables.contains("processed_updates"));

        let indexes = sqlite_objects(bootstrap.connection().connection(), "index");
        assert!(indexes.contains("idx_msg_chat_date"));
        assert!(indexes.contains("idx_msg_chat_user_date"));
        assert!(indexes.contains("idx_msg_chat_reply"));
    }

    #[test]
    fn bootstrap_is_idempotent_after_initialization() {
        let dir = tempdir().unwrap_or_else(|error| panic!("failed to create tempdir: {error}"));
        let database_path = dir.path().join("runtime.sqlite3");
        let storage = Storage::new(database_path);

        let first = storage
            .bootstrap()
            .unwrap_or_else(|error| panic!("first bootstrap failed: {error}"));
        drop(first);

        let second = storage
            .bootstrap()
            .unwrap_or_else(|error| panic!("second bootstrap failed: {error}"));

        assert_eq!(second.migration().previous_version, CURRENT_SCHEMA_VERSION);
        assert_eq!(second.migration().current_version, CURRENT_SCHEMA_VERSION);
        assert!(second.migration().applied_versions.is_empty());
        assert!(!second.migration().changed());

        let journal_indexes =
            index_names_for_table(second.connection().connection(), "message_journal");
        assert_eq!(
            journal_indexes,
            BTreeSet::from([
                String::from("idx_msg_chat_date"),
                String::from("idx_msg_chat_reply"),
                String::from("idx_msg_chat_user_date"),
            ])
        );
    }

    #[test]
    fn init_rejects_future_schema_version() {
        let dir = tempdir().unwrap_or_else(|error| panic!("failed to create tempdir: {error}"));
        let database_path = dir.path().join("runtime.sqlite3");
        let storage = Storage::new(database_path.clone());

        let connection = rusqlite::Connection::open(&database_path)
            .unwrap_or_else(|error| panic!("failed to create sqlite database: {error}"));
        connection
            .execute_batch("PRAGMA user_version = 99;")
            .unwrap_or_else(|error| panic!("failed to set user_version: {error}"));
        drop(connection);

        let error = storage
            .init()
            .expect_err("init must reject unsupported schema version");
        assert!(matches!(
            error,
            super::StorageError::UnsupportedSchemaVersion {
                found: 99,
                supported: CURRENT_SCHEMA_VERSION
            }
        ));
    }

    #[test]
    fn bootstrap_preserves_required_table_shapes() {
        let dir = tempdir().unwrap_or_else(|error| panic!("failed to create tempdir: {error}"));
        let database_path = dir.path().join("runtime.sqlite3");
        let storage = Storage::new(database_path);

        let bootstrap = storage
            .bootstrap()
            .unwrap_or_else(|error| panic!("failed to bootstrap storage: {error}"));
        let connection = bootstrap.connection().connection();

        assert_eq!(
            table_column_names(connection, "users"),
            vec![
                "user_id",
                "username",
                "display_name",
                "first_seen_at",
                "last_seen_at",
                "warn_count",
                "shadowbanned",
                "reputation",
                "state_json",
                "updated_at",
            ]
        );
        assert_eq!(
            table_column_names(connection, "kv_store"),
            vec!["scope_kind", "scope_id", "key", "value_json", "updated_at"]
        );
        assert_eq!(
            table_column_names(connection, "message_journal"),
            vec![
                "chat_id",
                "message_id",
                "user_id",
                "date_utc",
                "update_type",
                "text",
                "normalized_text",
                "has_media",
                "reply_to_message_id",
                "file_ids_json",
                "meta_json",
            ]
        );
        assert_eq!(
            table_column_names(connection, "jobs"),
            vec![
                "job_id",
                "executor_unit",
                "run_at",
                "scheduled_at",
                "status",
                "dedupe_key",
                "payload_json",
                "retry_count",
                "max_retries",
                "last_error_code",
                "last_error_text",
                "audit_action_id",
                "created_at",
                "updated_at",
            ]
        );
        assert_eq!(
            table_column_names(connection, "audit_log"),
            vec![
                "action_id",
                "trace_id",
                "request_id",
                "unit_name",
                "execution_mode",
                "op",
                "actor_user_id",
                "chat_id",
                "target_kind",
                "target_id",
                "trigger_message_id",
                "idempotency_key",
                "reversible",
                "compensation_json",
                "args_json",
                "result_json",
                "created_at",
            ]
        );
        assert_eq!(
            table_column_names(connection, "processed_updates"),
            vec!["update_id", "event_id", "processed_at", "execution_mode"]
        );
    }

    fn sqlite_objects(connection: &rusqlite::Connection, object_type: &str) -> BTreeSet<String> {
        let mut statement = connection
            .prepare(
                "SELECT name
                 FROM sqlite_master
                 WHERE type = ?1 AND name NOT LIKE 'sqlite_%'
                 ORDER BY name",
            )
            .unwrap_or_else(|error| panic!("failed to prepare sqlite_master query: {error}"));

        statement
            .query_map(params![object_type], |row| row.get::<_, String>(0))
            .unwrap_or_else(|error| panic!("failed to query sqlite_master: {error}"))
            .collect::<Result<BTreeSet<_>, _>>()
            .unwrap_or_else(|error| panic!("failed to collect sqlite objects: {error}"))
    }

    fn index_names_for_table(
        connection: &rusqlite::Connection,
        table_name: &str,
    ) -> BTreeSet<String> {
        let mut statement = connection
            .prepare(
                "SELECT name
                 FROM sqlite_master
                 WHERE type = 'index'
                   AND tbl_name = ?1
                   AND name NOT LIKE 'sqlite_%'
                 ORDER BY name",
            )
            .unwrap_or_else(|error| panic!("failed to prepare index query: {error}"));

        statement
            .query_map(params![table_name], |row| row.get::<_, String>(0))
            .unwrap_or_else(|error| panic!("failed to query indexes: {error}"))
            .collect::<Result<BTreeSet<_>, _>>()
            .unwrap_or_else(|error| panic!("failed to collect indexes: {error}"))
    }

    fn table_column_names(connection: &rusqlite::Connection, table_name: &str) -> Vec<String> {
        let pragma = format!("PRAGMA table_info({table_name})");
        let mut statement = connection
            .prepare(&pragma)
            .unwrap_or_else(|error| panic!("failed to prepare table_info pragma: {error}"));

        statement
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap_or_else(|error| panic!("failed to query table_info: {error}"))
            .collect::<Result<Vec<_>, _>>()
            .unwrap_or_else(|error| panic!("failed to collect column names: {error}"))
    }
}
