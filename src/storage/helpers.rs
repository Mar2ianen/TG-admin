use super::error::StorageError;
use super::types::*;
use rusqlite::Connection;
use std::fs;
use std::path::Path;

pub fn ensure_parent_dir(path: &Path) -> Result<(), StorageError> {
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

pub fn read_user_version(connection: &Connection) -> rusqlite::Result<u32> {
    connection.pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
}

pub fn bool_to_sqlite(value: bool) -> i64 {
    i64::from(value)
}

pub fn sqlite_to_bool(value: i64) -> bool {
    value != 0
}

pub fn validate_processed_update_status(status: &str) -> Result<(), StorageError> {
    match status {
        PROCESSED_UPDATE_STATUS_PENDING | PROCESSED_UPDATE_STATUS_COMPLETED => Ok(()),
        _ => Err(StorageError::InvalidProcessedUpdateStatus {
            status: status.to_owned(),
        }),
    }
}

pub fn map_user_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<UserRecord> {
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

pub fn map_kv_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<KvEntry> {
    Ok(KvEntry {
        scope_kind: row.get(0)?,
        scope_id: row.get(1)?,
        key: row.get(2)?,
        value_json: row.get(3)?,
        updated_at: row.get(4)?,
    })
}

pub fn map_processed_update_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<ProcessedUpdateRecord> {
    Ok(ProcessedUpdateRecord {
        update_id: row.get(0)?,
        event_id: row.get(1)?,
        processed_at: row.get(2)?,
        execution_mode: row.get(3)?,
        status: row.get(4)?,
    })
}

pub fn map_message_journal_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<MessageJournalRecord> {
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

pub fn map_job_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<JobRecord> {
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

pub fn map_audit_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<AuditLogEntry> {
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

pub fn map_external_effect_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ExternalEffectRecord> {
    Ok(ExternalEffectRecord {
        idempotency_key: row.get(0)?,
        operation: row.get(1)?,
        request_json: row.get(2)?,
        result_json: row.get(3)?,
        status: row.get(4)?,
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
        error_json: row.get(7)?,
    })
}
