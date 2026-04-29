#![cfg(test)]

use crate::storage::{Storage, StorageConnection};
use rusqlite::{Connection, params};
use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

#[cfg(test)]
fn tempdir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("tmo_test_{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn open_applies_sqlite_pragmas() {
    let dir = tempdir();
    let database_path = dir.join("runtime.sqlite3");
    let storage = Storage::with_config(
        database_path,
        crate::storage::StorageConfig {
            busy_timeout: std::time::Duration::from_secs(2),
            journal_mode: crate::storage::JournalMode::Wal,
            synchronous: crate::storage::SynchronousMode::Full,
            temp_store: crate::storage::TempStoreMode::Memory,
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
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn bootstrap_initializes_schema_version_once() {
    let dir = tempdir();
    let database_path = dir.join("runtime.sqlite3");
    let storage = Storage::new(database_path);

    let bootstrap = storage
        .bootstrap()
        .unwrap_or_else(|error| panic!("failed to bootstrap storage: {error}"));

    assert_eq!(bootstrap.migration().previous_version, 0);
    assert_eq!(
        bootstrap.migration().current_version,
        crate::storage::CURRENT_SCHEMA_VERSION
    );
    assert_eq!(bootstrap.migration().applied_versions, vec![1, 2, 3, 4, 5]);
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
    assert!(tables.contains("external_effects"));
    assert!(tables.contains("processed_updates"));

    let indexes = sqlite_objects(bootstrap.connection().connection(), "index");
    assert!(indexes.contains("idx_msg_chat_date"));
    assert!(indexes.contains("idx_msg_chat_user_date"));
    assert!(indexes.contains("idx_msg_chat_reply"));
    assert!(indexes.contains("idx_jobs_dedupe_key"));
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn bootstrap_is_idempotent_after_initialization() {
    let dir = tempdir();
    let database_path = dir.join("runtime.sqlite3");
    let storage = Storage::new(database_path);

    let first = storage
        .bootstrap()
        .unwrap_or_else(|error| panic!("first bootstrap failed: {error}"));
    drop(first);

    let second = storage
        .bootstrap()
        .unwrap_or_else(|error| panic!("second bootstrap failed: {error}"));

    assert_eq!(
        second.migration().previous_version,
        crate::storage::CURRENT_SCHEMA_VERSION
    );
    assert_eq!(
        second.migration().current_version,
        crate::storage::CURRENT_SCHEMA_VERSION
    );
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

    let job_indexes = index_names_for_table(second.connection().connection(), "jobs");
    assert_eq!(
        job_indexes,
        BTreeSet::from([String::from("idx_jobs_dedupe_key")])
    );
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn init_rejects_future_schema_version() {
    let dir = tempdir();
    let database_path = dir.join("runtime.sqlite3");
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
        crate::storage::StorageError::UnsupportedSchemaVersion {
            found: 99,
            supported: crate::storage::CURRENT_SCHEMA_VERSION
        }
    ));
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn bootstrap_preserves_required_table_shapes() {
    let dir = tempdir();
    let database_path = dir.join("runtime.sqlite3");
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
        table_column_names(connection, "external_effects"),
        vec![
            "idempotency_key",
            "operation",
            "request_json",
            "result_json",
            "status",
            "created_at",
            "updated_at",
            "error_json",
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
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn user_upsert_patch_preserves_existing_fields_and_updates_seen_bounds() {
    let storage = bootstrapped_storage();

    let initial = storage
        .upsert_user(&crate::storage::UserPatch {
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
        .upsert_user(&crate::storage::UserPatch {
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
    let entry = crate::storage::KvEntry {
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
    let first = crate::storage::ProcessedUpdateRecord {
        update_id: 404,
        event_id: String::from("evt-404"),
        processed_at: String::from("2026-04-21T14:00:00Z"),
        execution_mode: String::from("telegram"),
        status: String::from(crate::storage::PROCESSED_UPDATE_STATUS_COMPLETED),
    };
    let second = crate::storage::ProcessedUpdateRecord {
        update_id: 404,
        event_id: String::from("evt-replayed"),
        processed_at: String::from("2026-04-21T14:01:00Z"),
        execution_mode: String::from("replay"),
        status: String::from(crate::storage::PROCESSED_UPDATE_STATUS_COMPLETED),
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
    let pending = crate::storage::ProcessedUpdateRecord {
        update_id: 405,
        event_id: String::from("evt-405"),
        processed_at: String::from("2026-04-21T14:05:00Z"),
        execution_mode: String::from("telegram"),
        status: String::from(crate::storage::PROCESSED_UPDATE_STATUS_PENDING),
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
        Some(crate::storage::ProcessedUpdateRecord {
            update_id: 405,
            event_id: String::from("evt-405"),
            processed_at: String::from("2026-04-21T14:05:30Z"),
            execution_mode: String::from("telegram"),
            status: String::from(crate::storage::PROCESSED_UPDATE_STATUS_COMPLETED),
        })
    );
}

#[test]
fn migration_v2_backfills_processed_update_status_for_existing_rows() {
    let dir = tempdir();
    let database_path = dir.join("runtime.sqlite3");

    let connection = rusqlite::Connection::open(&database_path)
        .unwrap_or_else(|error| panic!("failed to create sqlite database: {error}"));
    connection
        .execute_batch(crate::storage::MIGRATION_V1_SQL)
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
    assert_eq!(
        migrated.migration().current_version,
        crate::storage::CURRENT_SCHEMA_VERSION
    );
    assert_eq!(migrated.migration().applied_versions, vec![2, 3, 4, 5]);
    assert_eq!(
        loaded,
        Some(crate::storage::ProcessedUpdateRecord {
            update_id: 777,
            event_id: String::from("evt-777"),
            processed_at: String::from("2026-04-21T18:00:00Z"),
            execution_mode: String::from("telegram"),
            status: String::from(crate::storage::PROCESSED_UPDATE_STATUS_COMPLETED),
        })
    );
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn jobs_preserve_dedupe_keys_and_payloads() {
    let storage = bootstrapped_storage();
    let job = crate::storage::JobRecord {
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
fn jobs_dedupe_by_partial_unique_index() {
    let storage = bootstrapped_storage();
    let first = crate::storage::JobRecord {
        job_id: String::from("job-dup-1"),
        executor_unit: String::from("units.warn"),
        run_at: String::from("2026-04-21T15:00:00Z"),
        scheduled_at: String::from("2026-04-21T14:59:30Z"),
        status: String::from("scheduled"),
        dedupe_key: Some(String::from("mute:chat-1:user-9")),
        payload_json: String::from("{\"reason\":\"first\"}"),
        retry_count: 0,
        max_retries: 3,
        last_error_code: None,
        last_error_text: None,
        audit_action_id: Some(String::from("act-job-dup-1")),
        created_at: String::from("2026-04-21T14:59:30Z"),
        updated_at: String::from("2026-04-21T14:59:30Z"),
    };
    let second = crate::storage::JobRecord {
        job_id: String::from("job-dup-2"),
        payload_json: String::from("{\"reason\":\"second\"}"),
        audit_action_id: Some(String::from("act-job-dup-2")),
        ..first.clone()
    };

    let stored_first = storage
        .insert_job(&first)
        .unwrap_or_else(|error| panic!("failed to insert first job: {error}"));
    let stored_second = storage
        .insert_job(&second)
        .unwrap_or_else(|error| panic!("failed to insert duplicate job: {error}"));

    assert_eq!(stored_first, first);
    assert_eq!(stored_second, first);
    assert!(
        storage
            .get_job(&second.job_id)
            .unwrap_or_else(|error| panic!("failed to load duplicate job: {error}"))
            .is_none()
    );
}

#[test]
fn stale_processing_jobs_are_recovered_to_scheduled() {
    let storage = bootstrapped_storage();
    let stale = crate::storage::JobRecord {
        job_id: String::from("job-stale"),
        executor_unit: String::from("units.warn"),
        run_at: String::from("2026-04-21T15:00:00Z"),
        scheduled_at: String::from("2026-04-21T14:59:30Z"),
        status: String::from("processing"),
        dedupe_key: Some(String::from("mute:chat-2:user-9")),
        payload_json: String::from("{\"reason\":\"stale\"}"),
        retry_count: 1,
        max_retries: 3,
        last_error_code: None,
        last_error_text: None,
        audit_action_id: Some(String::from("act-job-stale")),
        created_at: String::from("2026-04-21T14:59:30Z"),
        updated_at: String::from("2026-04-21T14:59:30Z"),
    };
    let fresh = crate::storage::JobRecord {
        job_id: String::from("job-fresh"),
        executor_unit: String::from("units.warn"),
        run_at: String::from("2026-04-21T15:01:00Z"),
        scheduled_at: String::from("2026-04-21T15:00:30Z"),
        status: String::from("processing"),
        dedupe_key: Some(String::from("mute:chat-3:user-9")),
        payload_json: String::from("{\"reason\":\"fresh\"}"),
        retry_count: 0,
        max_retries: 3,
        last_error_code: None,
        last_error_text: None,
        audit_action_id: Some(String::from("act-job-fresh")),
        created_at: String::from("2026-04-21T15:00:30Z"),
        updated_at: String::from("2026-04-21T15:00:30Z"),
    };

    storage
        .insert_job(&stale)
        .unwrap_or_else(|error| panic!("failed to insert stale job: {error}"));
    storage
        .insert_job(&fresh)
        .unwrap_or_else(|error| panic!("failed to insert fresh job: {error}"));

    let recovered = storage
        .recover_stale_processing_jobs("2026-04-21T15:00:00Z", "2026-04-21T15:05:00Z")
        .unwrap_or_else(|error| panic!("failed to recover stale jobs: {error}"));
    let recovered_job = storage
        .get_job(&stale.job_id)
        .unwrap_or_else(|error| panic!("failed to load recovered job: {error}"))
        .expect("recovered job exists");
    let fresh_job = storage
        .get_job(&fresh.job_id)
        .unwrap_or_else(|error| panic!("failed to load fresh job: {error}"))
        .expect("fresh job exists");

    assert_eq!(recovered, 1);
    assert_eq!(recovered_job.status, "scheduled");
    assert_eq!(recovered_job.retry_count, stale.retry_count + 1);
    assert_eq!(recovered_job.updated_at, "2026-04-21T15:05:00Z");
    assert_eq!(fresh_job.status, "processing");
    assert_eq!(fresh_job.retry_count, fresh.retry_count);
}

#[test]
fn audit_log_accepts_reversible_and_non_reversible_actions() {
    let storage = bootstrapped_storage();
    let reversible = crate::storage::AuditLogEntry {
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
    let non_reversible = crate::storage::AuditLogEntry {
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

#[test]
fn external_effects_preserve_request_result_and_status_transitions() {
    let storage = bootstrapped_storage();
    let reserved = crate::storage::ExternalEffectRecord {
        idempotency_key: String::from("tg.delete:-100:77"),
        operation: String::from("tg.delete"),
        request_json: String::from("{\"op\":\"tg.delete\",\"chat_id\":-100,\"message_id\":77}"),
        result_json: None,
        status: String::from(crate::storage::EXTERNAL_EFFECT_STATUS_IN_PROGRESS),
        created_at: String::from("2026-04-21T17:00:00Z"),
        updated_at: String::from("2026-04-21T17:00:00Z"),
        error_json: None,
    };

    let inserted = storage
        .reserve_external_effect(&reserved)
        .unwrap_or_else(|error| panic!("failed to reserve external effect: {error}"));
    let completed = storage
        .complete_external_effect(
            &reserved.idempotency_key,
            "{\"chat_id\":-100,\"message_id\":77}",
            "2026-04-21T17:00:01Z",
        )
        .unwrap_or_else(|error| panic!("failed to complete external effect: {error}"));
    let loaded = storage
        .get_external_effect(&reserved.idempotency_key)
        .unwrap_or_else(|error| panic!("failed to load external effect: {error}"))
        .expect("external effect exists");

    assert!(matches!(
        inserted,
        crate::storage::ExternalEffectReservation::Inserted(ref effect)
            if effect == &reserved
    ));
    assert!(completed);
    assert_eq!(
        loaded.status,
        crate::storage::EXTERNAL_EFFECT_STATUS_COMPLETED
    );
    assert_eq!(
        loaded.result_json.as_deref(),
        Some("{\"chat_id\":-100,\"message_id\":77}")
    );
    assert!(loaded.error_json.is_none());
}

#[cfg(test)]
fn bootstrapped_storage() -> StorageConnection {
    let dir = tempdir();
    let database_path = dir.join("runtime.sqlite3");
    let storage = Storage::new(database_path);

    storage
        .init()
        .unwrap_or_else(|error| panic!("failed to initialize storage: {error}"))
}

#[cfg(test)]
fn sqlite_objects(connection: &Connection, object_type: &str) -> BTreeSet<String> {
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

#[cfg(test)]
fn index_names_for_table(connection: &Connection, table_name: &str) -> BTreeSet<String> {
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

#[cfg(test)]
fn table_column_names(connection: &Connection, table_name: &str) -> Vec<String> {
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
