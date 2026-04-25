use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use thiserror::Error;

const CURRENT_SCHEMA_VERSION: u32 = 2;
pub const PROCESSED_UPDATE_STATUS_PENDING: &str = "pending";
pub const PROCESSED_UPDATE_STATUS_COMPLETED: &str = "completed";
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
const MIGRATION_V2_SQL: &str = "
ALTER TABLE processed_updates
ADD COLUMN status TEXT NOT NULL DEFAULT 'completed';

PRAGMA user_version = 2;
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserRecord {
    pub user_id: i64,
    pub username: Option<String>,
    pub display_name: Option<String>,
    pub first_seen_at: String,
    pub last_seen_at: String,
    pub warn_count: i64,
    pub shadowbanned: bool,
    pub reputation: i64,
    pub state_json: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserPatch {
    pub user_id: i64,
    pub username: Option<String>,
    pub display_name: Option<String>,
    pub seen_at: String,
    pub warn_count: Option<i64>,
    pub shadowbanned: Option<bool>,
    pub reputation: Option<i64>,
    pub state_json: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KvEntry {
    pub scope_kind: String,
    pub scope_id: String,
    pub key: String,
    pub value_json: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessedUpdateRecord {
    pub update_id: i64,
    pub event_id: String,
    pub processed_at: String,
    pub execution_mode: String,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageJournalRecord {
    pub chat_id: i64,
    pub message_id: i64,
    pub user_id: Option<i64>,
    pub date_utc: String,
    pub update_type: String,
    pub text: Option<String>,
    pub normalized_text: Option<String>,
    pub has_media: bool,
    pub reply_to_message_id: Option<i64>,
    pub file_ids_json: Option<String>,
    pub meta_json: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct AuditLogFilter {
    pub action_id: Option<String>,
    pub trace_id: Option<String>,
    pub request_id: Option<String>,
    pub idempotency_key: Option<String>,
    pub trigger_message_id: Option<i64>,
    pub actor_user_id: Option<i64>,
    pub chat_id: Option<i64>,
    pub op: Option<String>,
    pub target_id: Option<String>,
    pub reversible: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobRecord {
    pub job_id: String,
    pub executor_unit: String,
    pub run_at: String,
    pub scheduled_at: String,
    pub status: String,
    pub dedupe_key: Option<String>,
    pub payload_json: String,
    pub retry_count: i64,
    pub max_retries: i64,
    pub last_error_code: Option<String>,
    pub last_error_text: Option<String>,
    pub audit_action_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditLogEntry {
    pub action_id: String,
    pub trace_id: Option<String>,
    pub request_id: Option<String>,
    pub unit_name: String,
    pub execution_mode: String,
    pub op: String,
    pub actor_user_id: Option<i64>,
    pub chat_id: Option<i64>,
    pub target_kind: Option<String>,
    pub target_id: Option<String>,
    pub trigger_message_id: Option<i64>,
    pub idempotency_key: Option<String>,
    pub reversible: bool,
    pub compensation_json: Option<String>,
    pub args_json: String,
    pub result_json: Option<String>,
    pub created_at: String,
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
        if current_version < 2 {
            self.apply_migration_v2()?;
            applied_versions.push(2);
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

    fn apply_migration_v2(&mut self) -> Result<(), StorageError> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;

        tx.execute_batch(MIGRATION_V2_SQL)?;
        tx.commit()?;

        Ok(())
    }

    pub fn get_user(&self, user_id: i64) -> Result<Option<UserRecord>, StorageError> {
        let mut statement = self.connection.prepare(
            "SELECT user_id, username, display_name, first_seen_at, last_seen_at,
                    warn_count, shadowbanned, reputation, state_json, updated_at
             FROM users
             WHERE user_id = ?1",
        )?;

        statement
            .query_row(params![user_id], map_user_row)
            .optional()
            .map_err(StorageError::from)
    }

    pub fn upsert_user(&self, patch: &UserPatch) -> Result<UserRecord, StorageError> {
        self.connection.execute(
            "INSERT INTO users (
                 user_id, username, display_name, first_seen_at, last_seen_at,
                 warn_count, shadowbanned, reputation, state_json, updated_at
             )
             VALUES (
                 ?1, ?2, ?3, ?4, ?4,
                 COALESCE(?5, 0),
                 COALESCE(?6, 0),
                 COALESCE(?7, 0),
                 ?8,
                 ?9
             )
             ON CONFLICT(user_id) DO UPDATE SET
                 username = COALESCE(?2, users.username),
                 display_name = COALESCE(?3, users.display_name),
                 first_seen_at = CASE
                     WHEN ?4 < users.first_seen_at THEN ?4
                     ELSE users.first_seen_at
                 END,
                 last_seen_at = CASE
                     WHEN ?4 > users.last_seen_at THEN ?4
                     ELSE users.last_seen_at
                 END,
                 warn_count = COALESCE(?5, users.warn_count),
                 shadowbanned = COALESCE(?6, users.shadowbanned),
                 reputation = COALESCE(?7, users.reputation),
                 state_json = COALESCE(?8, users.state_json),
                 updated_at = ?9",
            params![
                patch.user_id,
                patch.username,
                patch.display_name,
                patch.seen_at,
                patch.warn_count,
                patch.shadowbanned.map(bool_to_sqlite),
                patch.reputation,
                patch.state_json,
                patch.updated_at,
            ],
        )?;

        self.get_user(patch.user_id)?
            .ok_or(StorageError::MissingRow("users"))
    }

    pub fn get_kv(
        &self,
        scope_kind: &str,
        scope_id: &str,
        key: &str,
    ) -> Result<Option<KvEntry>, StorageError> {
        let mut statement = self.connection.prepare(
            "SELECT scope_kind, scope_id, key, value_json, updated_at
             FROM kv_store
             WHERE scope_kind = ?1 AND scope_id = ?2 AND key = ?3",
        )?;

        statement
            .query_row(params![scope_kind, scope_id, key], map_kv_row)
            .optional()
            .map_err(StorageError::from)
    }

    pub fn set_kv(&self, entry: &KvEntry) -> Result<(), StorageError> {
        self.connection.execute(
            "INSERT INTO kv_store (scope_kind, scope_id, key, value_json, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(scope_kind, scope_id, key) DO UPDATE SET
                 value_json = excluded.value_json,
                 updated_at = excluded.updated_at",
            params![
                entry.scope_kind,
                entry.scope_id,
                entry.key,
                entry.value_json,
                entry.updated_at,
            ],
        )?;

        Ok(())
    }

    pub fn get_processed_update(
        &self,
        update_id: i64,
    ) -> Result<Option<ProcessedUpdateRecord>, StorageError> {
        let mut statement = self.connection.prepare(
            "SELECT update_id, event_id, processed_at, execution_mode, status
             FROM processed_updates
             WHERE update_id = ?1",
        )?;

        statement
            .query_row(params![update_id], map_processed_update_row)
            .optional()
            .map_err(StorageError::from)
    }

    pub fn mark_processed_update(
        &self,
        record: &ProcessedUpdateRecord,
    ) -> Result<Option<ProcessedUpdateRecord>, StorageError> {
        validate_processed_update_status(&record.status)?;

        let inserted = self.connection.execute(
            "INSERT OR IGNORE INTO processed_updates
                 (update_id, event_id, processed_at, execution_mode, status)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                record.update_id,
                record.event_id,
                record.processed_at,
                record.execution_mode,
                record.status,
            ],
        )?;

        if inserted > 0 {
            Ok(None)
        } else {
            self.get_processed_update(record.update_id)
        }
    }

    pub fn complete_processed_update(
        &self,
        update_id: i64,
        processed_at: &str,
    ) -> Result<bool, StorageError> {
        let updated = self.connection.execute(
            "UPDATE processed_updates
             SET status = ?2,
                 processed_at = ?3
             WHERE update_id = ?1
               AND status <> ?2",
            params![update_id, PROCESSED_UPDATE_STATUS_COMPLETED, processed_at,],
        )?;

        Ok(updated > 0)
    }

    pub fn append_message_journal(
        &self,
        record: &MessageJournalRecord,
    ) -> Result<(), StorageError> {
        self.connection.execute(
            "INSERT INTO message_journal (
                 chat_id, message_id, user_id, date_utc, update_type, text,
                 normalized_text, has_media, reply_to_message_id, file_ids_json, meta_json
             )
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT(chat_id, message_id) DO UPDATE SET
                 user_id = excluded.user_id,
                 date_utc = excluded.date_utc,
                 update_type = excluded.update_type,
                 text = excluded.text,
                 normalized_text = excluded.normalized_text,
                 has_media = excluded.has_media,
                 reply_to_message_id = excluded.reply_to_message_id,
                 file_ids_json = excluded.file_ids_json,
                 meta_json = excluded.meta_json",
            params![
                record.chat_id,
                record.message_id,
                record.user_id,
                record.date_utc,
                record.update_type,
                record.text,
                record.normalized_text,
                bool_to_sqlite(record.has_media),
                record.reply_to_message_id,
                record.file_ids_json,
                record.meta_json,
            ],
        )?;

        Ok(())
    }

    pub fn message_window(
        &self,
        chat_id: i64,
        anchor_message_id: i64,
        up: usize,
        down: usize,
        include_anchor: bool,
    ) -> Result<Vec<MessageJournalRecord>, StorageError> {
        let mut before_statement = self.connection.prepare(
            "SELECT chat_id, message_id, user_id, date_utc, update_type, text,
                    normalized_text, has_media, reply_to_message_id, file_ids_json, meta_json
             FROM message_journal
             WHERE chat_id = ?1 AND message_id < ?2
             ORDER BY message_id DESC
             LIMIT ?3",
        )?;
        let mut before = before_statement
            .query_map(
                params![chat_id, anchor_message_id, up],
                map_message_journal_row,
            )?
            .collect::<Result<Vec<_>, _>>()
            .map_err(StorageError::from)?;
        before.reverse();

        let anchor = if include_anchor {
            let mut anchor_statement = self.connection.prepare(
                "SELECT chat_id, message_id, user_id, date_utc, update_type, text,
                        normalized_text, has_media, reply_to_message_id, file_ids_json, meta_json
                 FROM message_journal
                 WHERE chat_id = ?1 AND message_id = ?2",
            )?;
            anchor_statement
                .query_row(params![chat_id, anchor_message_id], map_message_journal_row)
                .optional()
                .map_err(StorageError::from)?
                .into_iter()
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };

        let mut after_statement = self.connection.prepare(
            "SELECT chat_id, message_id, user_id, date_utc, update_type, text,
                    normalized_text, has_media, reply_to_message_id, file_ids_json, meta_json
             FROM message_journal
             WHERE chat_id = ?1 AND message_id > ?2
             ORDER BY message_id ASC
             LIMIT ?3",
        )?;
        let after = after_statement
            .query_map(
                params![chat_id, anchor_message_id, down],
                map_message_journal_row,
            )?
            .collect::<Result<Vec<_>, _>>()
            .map_err(StorageError::from)?;

        let mut messages = before;
        messages.extend(anchor);
        messages.extend(after);
        Ok(messages)
    }

    pub fn messages_by_user(
        &self,
        chat_id: i64,
        user_id: i64,
        since: &str,
        limit: usize,
    ) -> Result<Vec<MessageJournalRecord>, StorageError> {
        let mut statement = self.connection.prepare(
            "SELECT chat_id, message_id, user_id, date_utc, update_type, text,
                    normalized_text, has_media, reply_to_message_id, file_ids_json, meta_json
             FROM message_journal
             WHERE chat_id = ?1 AND user_id = ?2 AND date_utc >= ?3
             ORDER BY date_utc DESC, message_id DESC
             LIMIT ?4",
        )?;

        statement
            .query_map(
                params![chat_id, user_id, since, limit],
                map_message_journal_row,
            )?
            .collect::<Result<Vec<_>, _>>()
            .map_err(StorageError::from)
    }

    pub fn get_job(&self, job_id: &str) -> Result<Option<JobRecord>, StorageError> {
        let mut statement = self.connection.prepare(
            "SELECT job_id, executor_unit, run_at, scheduled_at, status, dedupe_key,
                    payload_json, retry_count, max_retries, last_error_code,
                    last_error_text, audit_action_id, created_at, updated_at
             FROM jobs
             WHERE job_id = ?1",
        )?;

        statement
            .query_row(params![job_id], map_job_row)
            .optional()
            .map_err(StorageError::from)
    }

    pub fn insert_job(&self, job: &JobRecord) -> Result<(), StorageError> {
        self.connection.execute(
            "INSERT INTO jobs (
                 job_id, executor_unit, run_at, scheduled_at, status, dedupe_key,
                 payload_json, retry_count, max_retries, last_error_code,
                 last_error_text, audit_action_id, created_at, updated_at
             )
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
                job.job_id,
                job.executor_unit,
                job.run_at,
                job.scheduled_at,
                job.status,
                job.dedupe_key,
                job.payload_json,
                job.retry_count,
                job.max_retries,
                job.last_error_code,
                job.last_error_text,
                job.audit_action_id,
                job.created_at,
                job.updated_at,
            ],
        )?;

        Ok(())
    }

    pub fn poll_due_jobs(&self, now: &str, limit: usize) -> Result<Vec<JobRecord>, StorageError> {
        let mut statement = self.connection.prepare(
            "SELECT job_id, executor_unit, run_at, scheduled_at, status, dedupe_key,
                    payload_json, retry_count, max_retries, last_error_code,
                    last_error_text, audit_action_id, created_at, updated_at
             FROM jobs
             WHERE status = 'scheduled' AND run_at <= ?1
             ORDER BY run_at ASC
             LIMIT ?2",
        )?;

        statement
            .query_map(params![now, limit as i64], map_job_row)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(StorageError::from)
    }

    pub fn update_job_status(
        &self,
        job_id: &str,
        status: &str,
        error_text: Option<&str>,
        now: &str,
    ) -> Result<(), StorageError> {
        self.connection.execute(
            "UPDATE jobs SET status = ?2, last_error_text = ?3, updated_at = ?4
             WHERE job_id = ?1",
            params![job_id, status, error_text, now],
        )?;
        Ok(())
    }

    pub fn get_audit_entry(&self, action_id: &str) -> Result<Option<AuditLogEntry>, StorageError> {
        let mut statement = self.connection.prepare(
            "SELECT action_id, trace_id, request_id, unit_name, execution_mode, op,
                    actor_user_id, chat_id, target_kind, target_id, trigger_message_id,
                    idempotency_key, reversible, compensation_json, args_json,
                    result_json, created_at
             FROM audit_log
             WHERE action_id = ?1",
        )?;

        statement
            .query_row(params![action_id], map_audit_row)
            .optional()
            .map_err(StorageError::from)
    }

    pub fn find_audit_by_idempotency_key(
        &self,
        idempotency_key: &str,
    ) -> Result<Vec<AuditLogEntry>, StorageError> {
        let mut statement = self.connection.prepare(
            "SELECT action_id, trace_id, request_id, unit_name, execution_mode, op,
                    actor_user_id, chat_id, target_kind, target_id, trigger_message_id,
                    idempotency_key, reversible, compensation_json, args_json,
                    result_json, created_at
             FROM audit_log
             WHERE idempotency_key = ?1
             ORDER BY created_at, action_id",
        )?;

        statement
            .query_map(params![idempotency_key], map_audit_row)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(StorageError::from)
    }

    pub fn find_audit_entries(
        &self,
        filter: &AuditLogFilter,
        limit: usize,
    ) -> Result<Vec<AuditLogEntry>, StorageError> {
        let mut statement = self.connection.prepare(
            "SELECT action_id, trace_id, request_id, unit_name, execution_mode, op,
                    actor_user_id, chat_id, target_kind, target_id, trigger_message_id,
                    idempotency_key, reversible, compensation_json, args_json,
                    result_json, created_at
             FROM audit_log
             WHERE (?1 IS NULL OR action_id = ?1)
               AND (?2 IS NULL OR trace_id = ?2)
               AND (?3 IS NULL OR request_id = ?3)
               AND (?4 IS NULL OR idempotency_key = ?4)
               AND (?5 IS NULL OR trigger_message_id = ?5)
               AND (?6 IS NULL OR actor_user_id = ?6)
               AND (?7 IS NULL OR chat_id = ?7)
               AND (?8 IS NULL OR op = ?8)
               AND (?9 IS NULL OR target_id = ?9)
               AND (?10 IS NULL OR reversible = ?10)
             ORDER BY created_at DESC, action_id DESC
             LIMIT ?11",
        )?;

        statement
            .query_map(
                params![
                    filter.action_id,
                    filter.trace_id,
                    filter.request_id,
                    filter.idempotency_key,
                    filter.trigger_message_id,
                    filter.actor_user_id,
                    filter.chat_id,
                    filter.op,
                    filter.target_id,
                    filter.reversible.map(bool_to_sqlite),
                    limit,
                ],
                map_audit_row,
            )?
            .collect::<Result<Vec<_>, _>>()
            .map_err(StorageError::from)
    }

    pub fn append_audit_entry(&self, entry: &AuditLogEntry) -> Result<(), StorageError> {
        self.connection.execute(
            "INSERT INTO audit_log (
                 action_id, trace_id, request_id, unit_name, execution_mode, op,
                 actor_user_id, chat_id, target_kind, target_id, trigger_message_id,
                 idempotency_key, reversible, compensation_json, args_json,
                 result_json, created_at
             )
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
            params![
                entry.action_id,
                entry.trace_id,
                entry.request_id,
                entry.unit_name,
                entry.execution_mode,
                entry.op,
                entry.actor_user_id,
                entry.chat_id,
                entry.target_kind,
                entry.target_id,
                entry.trigger_message_id,
                entry.idempotency_key,
                bool_to_sqlite(entry.reversible),
                entry.compensation_json,
                entry.args_json,
                entry.result_json,
                entry.created_at,
            ],
        )?;

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
    #[error("invalid processed update status `{status}`")]
    InvalidProcessedUpdateStatus { status: String },
    #[error("expected persisted row in `{0}` after write")]
    MissingRow(&'static str),
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

fn bool_to_sqlite(value: bool) -> i64 {
    i64::from(value)
}

fn sqlite_to_bool(value: i64) -> bool {
    value != 0
}

fn validate_processed_update_status(status: &str) -> Result<(), StorageError> {
    match status {
        PROCESSED_UPDATE_STATUS_PENDING | PROCESSED_UPDATE_STATUS_COMPLETED => Ok(()),
        _ => Err(StorageError::InvalidProcessedUpdateStatus {
            status: status.to_owned(),
        }),
    }
}

fn map_user_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<UserRecord> {
    Ok(UserRecord {
        user_id: row.get(0)?,
        username: row.get(1)?,
        display_name: row.get(2)?,
        first_seen_at: row.get(3)?,
        last_seen_at: row.get(4)?,
        warn_count: row.get(5)?,
        shadowbanned: sqlite_to_bool(row.get(6)?),
        reputation: row.get(7)?,
        state_json: row.get(8)?,
        updated_at: row.get(9)?,
    })
}

fn map_kv_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<KvEntry> {
    Ok(KvEntry {
        scope_kind: row.get(0)?,
        scope_id: row.get(1)?,
        key: row.get(2)?,
        value_json: row.get(3)?,
        updated_at: row.get(4)?,
    })
}

fn map_processed_update_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ProcessedUpdateRecord> {
    Ok(ProcessedUpdateRecord {
        update_id: row.get(0)?,
        event_id: row.get(1)?,
        processed_at: row.get(2)?,
        execution_mode: row.get(3)?,
        status: row.get(4)?,
    })
}

fn map_message_journal_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<MessageJournalRecord> {
    Ok(MessageJournalRecord {
        chat_id: row.get(0)?,
        message_id: row.get(1)?,
        user_id: row.get(2)?,
        date_utc: row.get(3)?,
        update_type: row.get(4)?,
        text: row.get(5)?,
        normalized_text: row.get(6)?,
        has_media: sqlite_to_bool(row.get(7)?),
        reply_to_message_id: row.get(8)?,
        file_ids_json: row.get(9)?,
        meta_json: row.get(10)?,
    })
}

fn map_job_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<JobRecord> {
    Ok(JobRecord {
        job_id: row.get(0)?,
        executor_unit: row.get(1)?,
        run_at: row.get(2)?,
        scheduled_at: row.get(3)?,
        status: row.get(4)?,
        dedupe_key: row.get(5)?,
        payload_json: row.get(6)?,
        retry_count: row.get(7)?,
        max_retries: row.get(8)?,
        last_error_code: row.get(9)?,
        last_error_text: row.get(10)?,
        audit_action_id: row.get(11)?,
        created_at: row.get(12)?,
        updated_at: row.get(13)?,
    })
}

fn map_audit_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<AuditLogEntry> {
    Ok(AuditLogEntry {
        action_id: row.get(0)?,
        trace_id: row.get(1)?,
        request_id: row.get(2)?,
        unit_name: row.get(3)?,
        execution_mode: row.get(4)?,
        op: row.get(5)?,
        actor_user_id: row.get(6)?,
        chat_id: row.get(7)?,
        target_kind: row.get(8)?,
        target_id: row.get(9)?,
        trigger_message_id: row.get(10)?,
        idempotency_key: row.get(11)?,
        reversible: sqlite_to_bool(row.get(12)?),
        compensation_json: row.get(13)?,
        args_json: row.get(14)?,
        result_json: row.get(15)?,
        created_at: row.get(16)?,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        AuditLogEntry, CURRENT_SCHEMA_VERSION, JobRecord, JournalMode, KvEntry, MIGRATION_V1_SQL,
        PROCESSED_UPDATE_STATUS_COMPLETED, PROCESSED_UPDATE_STATUS_PENDING, ProcessedUpdateRecord,
        Storage, StorageConfig, SynchronousMode, TempStoreMode, UserPatch,
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
        assert_eq!(bootstrap.migration().applied_versions, vec![1, 2]);
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
            vec![
                "update_id",
                "event_id",
                "processed_at",
                "execution_mode",
                "status",
            ]
        );
    }

    #[test]
    fn user_upsert_patch_preserves_existing_fields_and_updates_seen_bounds() {
        let storage = bootstrapped_storage();

        let initial = storage
            .upsert_user(&UserPatch {
                user_id: 77,
                username: Some(String::from("first_name")),
                display_name: None,
                seen_at: String::from("2026-04-21T10:00:00Z"),
                warn_count: Some(1),
                shadowbanned: Some(false),
                reputation: Some(10),
                state_json: Some(String::from("{\"note\":\"initial\"}")),
                updated_at: String::from("2026-04-21T10:00:01Z"),
            })
            .unwrap_or_else(|error| panic!("failed to insert initial user patch: {error}"));

        assert_eq!(initial.first_seen_at, "2026-04-21T10:00:00Z");
        assert_eq!(initial.last_seen_at, "2026-04-21T10:00:00Z");

        let patched = storage
            .upsert_user(&UserPatch {
                user_id: 77,
                username: None,
                display_name: Some(String::from("Display Name")),
                seen_at: String::from("2026-04-21T12:00:00Z"),
                warn_count: None,
                shadowbanned: Some(true),
                reputation: Some(25),
                state_json: None,
                updated_at: String::from("2026-04-21T12:00:30Z"),
            })
            .unwrap_or_else(|error| panic!("failed to patch existing user: {error}"));

        assert_eq!(patched.username.as_deref(), Some("first_name"));
        assert_eq!(patched.display_name.as_deref(), Some("Display Name"));
        assert_eq!(patched.first_seen_at, "2026-04-21T10:00:00Z");
        assert_eq!(patched.last_seen_at, "2026-04-21T12:00:00Z");
        assert_eq!(patched.warn_count, 1);
        assert!(patched.shadowbanned);
        assert_eq!(patched.reputation, 25);
        assert_eq!(
            patched.state_json.as_deref(),
            Some("{\"note\":\"initial\"}")
        );
    }

    #[test]
    fn kv_store_round_trips_sql_like_values_without_schema_damage() {
        let storage = bootstrapped_storage();
        let entry = KvEntry {
            scope_kind: String::from("chat'; DROP TABLE kv_store; --"),
            scope_id: String::from("scope-1"),
            key: String::from("json-key"),
            value_json: String::from("{\"expr\":\"x' OR 1=1 --\"}"),
            updated_at: String::from("2026-04-21T13:00:00Z"),
        };

        storage
            .set_kv(&entry)
            .unwrap_or_else(|error| panic!("failed to set kv value: {error}"));

        let loaded = storage
            .get_kv(&entry.scope_kind, &entry.scope_id, &entry.key)
            .unwrap_or_else(|error| panic!("failed to load kv value: {error}"));

        assert_eq!(loaded, Some(entry));

        let kv_store_still_exists: u32 = storage
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'kv_store'",
                [],
                |row| row.get(0),
            )
            .unwrap_or_else(|error| panic!("failed to verify kv_store existence: {error}"));
        assert_eq!(kv_store_still_exists, 1);
    }

    #[test]
    fn processed_updates_deduplicate_replay_marks() {
        let storage = bootstrapped_storage();
        let first = ProcessedUpdateRecord {
            update_id: 404,
            event_id: String::from("evt-404"),
            processed_at: String::from("2026-04-21T14:00:00Z"),
            execution_mode: String::from("telegram"),
            status: String::from(PROCESSED_UPDATE_STATUS_COMPLETED),
        };
        let second = ProcessedUpdateRecord {
            update_id: 404,
            event_id: String::from("evt-replayed"),
            processed_at: String::from("2026-04-21T14:01:00Z"),
            execution_mode: String::from("replay"),
            status: String::from(PROCESSED_UPDATE_STATUS_COMPLETED),
        };

        let inserted_first = storage
            .mark_processed_update(&first)
            .unwrap_or_else(|error| panic!("failed to mark first processed update: {error}"));
        let inserted_second = storage
            .mark_processed_update(&second)
            .unwrap_or_else(|error| panic!("failed to mark replayed update: {error}"));
        let loaded = storage
            .get_processed_update(first.update_id)
            .unwrap_or_else(|error| panic!("failed to load processed update: {error}"));

        assert!(inserted_first.is_none());
        assert_eq!(inserted_second, Some(first.clone()));
        assert_eq!(loaded, Some(first));
    }

    #[test]
    fn processed_updates_support_pending_to_completed_transition() {
        let storage = bootstrapped_storage();
        let pending = ProcessedUpdateRecord {
            update_id: 405,
            event_id: String::from("evt-405"),
            processed_at: String::from("2026-04-21T14:05:00Z"),
            execution_mode: String::from("telegram"),
            status: String::from(PROCESSED_UPDATE_STATUS_PENDING),
        };

        let inserted = storage
            .mark_processed_update(&pending)
            .unwrap_or_else(|error| panic!("failed to insert pending processed update: {error}"));
        let completed = storage
            .complete_processed_update(405, "2026-04-21T14:05:30Z")
            .unwrap_or_else(|error| panic!("failed to complete processed update: {error}"));
        let loaded = storage
            .get_processed_update(405)
            .unwrap_or_else(|error| panic!("failed to load completed processed update: {error}"));

        assert!(inserted.is_none());
        assert!(completed);
        assert_eq!(
            loaded,
            Some(ProcessedUpdateRecord {
                update_id: 405,
                event_id: String::from("evt-405"),
                processed_at: String::from("2026-04-21T14:05:30Z"),
                execution_mode: String::from("telegram"),
                status: String::from(PROCESSED_UPDATE_STATUS_COMPLETED),
            })
        );
    }

    #[test]
    fn migration_v2_backfills_processed_update_status_for_existing_rows() {
        let dir = tempdir().unwrap_or_else(|error| panic!("failed to create tempdir: {error}"));
        let database_path = dir.path().join("runtime.sqlite3");

        let connection = rusqlite::Connection::open(&database_path)
            .unwrap_or_else(|error| panic!("failed to create sqlite database: {error}"));
        connection
            .execute_batch(MIGRATION_V1_SQL)
            .unwrap_or_else(|error| panic!("failed to apply v1 schema: {error}"));
        connection
            .execute(
                "INSERT INTO processed_updates (update_id, event_id, processed_at, execution_mode)
                 VALUES (?1, ?2, ?3, ?4)",
                params![777_i64, "evt-777", "2026-04-21T18:00:00Z", "telegram"],
            )
            .unwrap_or_else(|error| panic!("failed to seed v1 processed update row: {error}"));
        drop(connection);

        let storage = Storage::new(database_path);
        let migrated = storage
            .bootstrap()
            .unwrap_or_else(|error| panic!("failed to migrate storage: {error}"));
        let loaded = migrated
            .connection()
            .get_processed_update(777)
            .unwrap_or_else(|error| panic!("failed to load migrated processed update: {error}"));

        assert_eq!(migrated.migration().previous_version, 1);
        assert_eq!(migrated.migration().current_version, CURRENT_SCHEMA_VERSION);
        assert_eq!(migrated.migration().applied_versions, vec![2]);
        assert_eq!(
            loaded,
            Some(ProcessedUpdateRecord {
                update_id: 777,
                event_id: String::from("evt-777"),
                processed_at: String::from("2026-04-21T18:00:00Z"),
                execution_mode: String::from("telegram"),
                status: String::from(PROCESSED_UPDATE_STATUS_COMPLETED),
            })
        );
    }

    #[test]
    fn jobs_preserve_dedupe_keys_and_payloads() {
        let storage = bootstrapped_storage();
        let job = JobRecord {
            job_id: String::from("job-1"),
            executor_unit: String::from("units.warn"),
            run_at: String::from("2026-04-21T15:00:00Z"),
            scheduled_at: String::from("2026-04-21T14:59:30Z"),
            status: String::from("scheduled"),
            dedupe_key: Some(String::from("mute:chat-1:user-9")),
            payload_json: String::from("{\"reason\":\"spam'; DROP TABLE jobs; --\"}"),
            retry_count: 0,
            max_retries: 3,
            last_error_code: None,
            last_error_text: None,
            audit_action_id: Some(String::from("act-job-1")),
            created_at: String::from("2026-04-21T14:59:30Z"),
            updated_at: String::from("2026-04-21T14:59:30Z"),
        };

        storage
            .insert_job(&job)
            .unwrap_or_else(|error| panic!("failed to insert job: {error}"));

        let loaded = storage
            .get_job(&job.job_id)
            .unwrap_or_else(|error| panic!("failed to load job: {error}"));

        assert_eq!(loaded, Some(job));
    }

    #[test]
    fn audit_log_accepts_reversible_and_non_reversible_actions() {
        let storage = bootstrapped_storage();
        let reversible = AuditLogEntry {
            action_id: String::from("audit-1"),
            trace_id: Some(String::from("trace-1")),
            request_id: Some(String::from("req-1")),
            unit_name: String::from("units.warn"),
            execution_mode: String::from("telegram"),
            op: String::from("mute"),
            actor_user_id: Some(100),
            chat_id: Some(-10001),
            target_kind: Some(String::from("user")),
            target_id: Some(String::from("42")),
            trigger_message_id: Some(501),
            idempotency_key: Some(String::from("idem-1")),
            reversible: true,
            compensation_json: Some(String::from("{\"undo\":\"unmute\"}")),
            args_json: String::from("{\"duration\":\"10m\"}"),
            result_json: Some(String::from("{\"ok\":true}")),
            created_at: String::from("2026-04-21T16:00:00Z"),
        };
        let non_reversible = AuditLogEntry {
            action_id: String::from("audit-2"),
            trace_id: None,
            request_id: None,
            unit_name: String::from("units.cleanup"),
            execution_mode: String::from("manual"),
            op: String::from("del"),
            actor_user_id: Some(100),
            chat_id: Some(-10001),
            target_kind: Some(String::from("message")),
            target_id: Some(String::from("chat'; DELETE FROM audit_log; --")),
            trigger_message_id: Some(777),
            idempotency_key: Some(String::from("idem-2")),
            reversible: false,
            compensation_json: None,
            args_json: String::from("{\"ids\":[1,2,3]}"),
            result_json: Some(String::from("{\"deleted\":3}")),
            created_at: String::from("2026-04-21T16:01:00Z"),
        };

        storage
            .append_audit_entry(&reversible)
            .unwrap_or_else(|error| panic!("failed to append reversible audit entry: {error}"));
        storage
            .append_audit_entry(&non_reversible)
            .unwrap_or_else(|error| panic!("failed to append non-reversible audit entry: {error}"));

        let loaded_reversible = storage
            .get_audit_entry(&reversible.action_id)
            .unwrap_or_else(|error| panic!("failed to load reversible audit entry: {error}"));
        let loaded_non_reversible = storage
            .get_audit_entry(&non_reversible.action_id)
            .unwrap_or_else(|error| panic!("failed to load non-reversible audit entry: {error}"));
        let idempotent_match = storage
            .find_audit_by_idempotency_key("idem-2")
            .unwrap_or_else(|error| panic!("failed to query audit by idempotency key: {error}"));

        assert_eq!(loaded_reversible, Some(reversible));
        assert_eq!(loaded_non_reversible, Some(non_reversible.clone()));
        assert_eq!(idempotent_match, vec![non_reversible]);
    }

    fn bootstrapped_storage() -> super::StorageConnection {
        let dir = tempdir().unwrap_or_else(|error| panic!("failed to create tempdir: {error}"));
        let database_path = dir.path().join("runtime.sqlite3");
        let storage = Storage::new(database_path);

        storage
            .init()
            .unwrap_or_else(|error| panic!("failed to initialize storage: {error}"))
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
