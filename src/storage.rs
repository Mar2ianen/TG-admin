use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, TransactionBehavior};
use thiserror::Error;

const CURRENT_SCHEMA_VERSION: u32 = 1;

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

        tx.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS schema_bootstrap (
              key TEXT PRIMARY KEY,
              value TEXT NOT NULL
            );
            INSERT OR IGNORE INTO schema_bootstrap (key, value)
            VALUES ('storage_bootstrap', 'initialized');
            PRAGMA user_version = 1;
            ",
        )?;
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
        CURRENT_SCHEMA_VERSION, JournalMode, Storage, StorageConfig, SynchronousMode, TempStoreMode,
    };
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
}
