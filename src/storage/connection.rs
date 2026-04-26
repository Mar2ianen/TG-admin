use super::config::*;
use super::error::*;
use super::helpers::*;
use super::schema::*;
use super::types::*;
use chrono::Utc;
use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior, params};
use std::path::{Path, PathBuf};

pub struct StorageConnection {
    pub(crate) path: PathBuf,
    pub(crate) config: StorageConfig,
    pub(crate) connection: Connection,
}

impl std::fmt::Debug for StorageConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StorageConnection")
            .field("path", &self.path)
            .field("config", &self.config)
            .finish()
    }
}

impl Clone for StorageConnection {
    fn clone(&self) -> Self {
        Self::open(self.path.clone(), self.config.clone())
            .expect("failed to open cloned sqlite connection")
    }
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
        if current_version < 3 {
            self.apply_migration_v3()?;
            applied_versions.push(3);
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

    fn apply_migration_v3(&mut self) -> Result<(), StorageError> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;

        tx.execute_batch(MIGRATION_V3_SQL)?;
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

    pub fn increment_message_counters(
        &self,
        chat_id: i64,
        user_id: i64,
    ) -> Result<(), StorageError> {
        let now = Utc::now().to_rfc3339();

        // Increment user counter in chat
        self.connection.execute(
            "INSERT INTO message_counters (chat_id, user_id, count, updated_at)
             VALUES (?1, ?2, 1, ?3)
             ON CONFLICT(chat_id, user_id) DO UPDATE SET
                count = count + 1,
                updated_at = excluded.updated_at",
            params![chat_id, user_id, now],
        )?;

        // Increment chat overall counter
        self.connection.execute(
            "INSERT INTO chat_counters (chat_id, count, updated_at)
             VALUES (?1, 1, ?2)
             ON CONFLICT(chat_id) DO UPDATE SET
                count = count + 1,
                updated_at = excluded.updated_at",
            params![chat_id, now],
        )?;

        Ok(())
    }

    pub fn create_counter_snapshots(
        &mut self,
        period_type: &str,
        period_start: &str,
    ) -> Result<(), StorageError> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;

        // Snapshot user counters
        tx.execute(
            "INSERT OR REPLACE INTO counter_history (chat_id, user_id, period_type, period_start, count)
             SELECT chat_id, user_id, ?1, ?2, count FROM message_counters",
            params![period_type, period_start],
        )?;

        // Snapshot chat counters
        tx.execute(
            "INSERT OR REPLACE INTO counter_history (chat_id, user_id, period_type, period_start, count)
             SELECT chat_id, NULL, ?1, ?2, count FROM chat_counters",
            params![period_type, period_start],
        )?;

        // Reset current counters
        tx.execute("DELETE FROM message_counters", [])?;
        tx.execute("DELETE FROM chat_counters", [])?;

        tx.commit()?;
        Ok(())
    }
}
