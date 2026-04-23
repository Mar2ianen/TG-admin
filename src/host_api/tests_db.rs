use crate::host_api::test_support::{dry_run_storage_api, manual_event, storage_api};
use crate::host_api::{
    DbKvGetRequest, DbKvSetRequest, DbUserGetRequest, DbUserIncrRequest, DbUserPatchRequest,
    HostApi, HostApiErrorDetail, HostApiErrorKind, HostApiOperation,
};
use crate::storage::{KvEntry, UserPatch};

#[test]
fn db_user_get_returns_typed_user_value() {
    let event = manual_event();
    let (_dir, api) = storage_api();
    api.storage(HostApiOperation::DbUserGet)
        .expect("storage")
        .upsert_user(&UserPatch {
            user_id: 77,
            username: Some("reply_user".to_owned()),
            display_name: Some("Reply User".to_owned()),
            seen_at: "2026-04-21T12:00:00Z".to_owned(),
            warn_count: Some(1),
            shadowbanned: Some(false),
            reputation: Some(4),
            state_json: Some("{\"state\":\"ok\"}".to_owned()),
            updated_at: "2026-04-21T12:00:00Z".to_owned(),
        })
        .expect("seed user");

    let response = api
        .db_user_get(&event, DbUserGetRequest { user_id: 77 })
        .expect("db.user_get succeeds");

    assert_eq!(response.operation, HostApiOperation::DbUserGet);
    assert_eq!(
        response
            .value
            .user
            .expect("user exists")
            .username
            .as_deref(),
        Some("reply_user")
    );
}

#[test]
fn db_user_get_rejects_zero_user_id() {
    let event = manual_event();
    let (_dir, api) = storage_api();

    let error = api
        .db_user_get(&event, DbUserGetRequest { user_id: 0 })
        .expect_err("zero user id must fail");

    assert_eq!(error.kind, HostApiErrorKind::Validation);
    assert_eq!(error.operation, HostApiOperation::DbUserGet);
    assert_eq!(
        error.detail,
        HostApiErrorDetail::InvalidField {
            field: "user_id".to_owned(),
            message: "must be non-zero".to_owned(),
        }
    );
}

#[test]
fn db_user_get_requires_storage_resource() {
    let event = manual_event();
    let api = HostApi::new(false);

    let error = api
        .db_user_get(&event, DbUserGetRequest { user_id: 77 })
        .expect_err("missing storage must fail");

    assert_eq!(error.kind, HostApiErrorKind::Internal);
    assert_eq!(error.operation, HostApiOperation::DbUserGet);
    assert_eq!(
        error.detail,
        HostApiErrorDetail::ResourceUnavailable {
            resource: "storage".to_owned(),
        }
    );
}

#[test]
fn db_user_patch_persists_user_on_happy_path() {
    let event = manual_event();
    let (_dir, api) = storage_api();

    let response = api
        .db_user_patch(
            &event,
            DbUserPatchRequest {
                patch: UserPatch {
                    user_id: 77,
                    username: Some("patched_user".to_owned()),
                    display_name: Some("Patched User".to_owned()),
                    seen_at: "2026-04-21T12:05:00Z".to_owned(),
                    warn_count: Some(2),
                    shadowbanned: Some(false),
                    reputation: Some(9),
                    state_json: Some("{\"state\":\"patched\"}".to_owned()),
                    updated_at: "2026-04-21T12:05:00Z".to_owned(),
                },
            },
        )
        .expect("patch succeeds");

    assert!(!response.dry_run);
    assert_eq!(
        response.value.user.username.as_deref(),
        Some("patched_user")
    );
    assert_eq!(
        api.storage(HostApiOperation::DbUserPatch)
            .expect("storage")
            .get_user(77)
            .expect("query succeeds")
            .expect("user exists")
            .username
            .as_deref(),
        Some("patched_user")
    );
}

#[test]
fn db_user_patch_dry_run_validates_without_mutation() {
    let event = manual_event();
    let (_dir, api) = dry_run_storage_api();

    let response = api
        .db_user_patch(
            &event,
            DbUserPatchRequest {
                patch: UserPatch {
                    user_id: 77,
                    username: Some("dry_run_user".to_owned()),
                    display_name: Some("Dry Run".to_owned()),
                    seen_at: "2026-04-21T12:05:00Z".to_owned(),
                    warn_count: Some(2),
                    shadowbanned: Some(true),
                    reputation: Some(5),
                    state_json: Some("{\"mode\":\"dry\"}".to_owned()),
                    updated_at: "2026-04-21T12:05:00Z".to_owned(),
                },
            },
        )
        .expect("dry-run patch succeeds");

    assert!(response.dry_run);
    assert_eq!(response.value.user.warn_count, 2);
    assert!(api
        .storage(HostApiOperation::DbUserPatch)
        .expect("storage")
        .get_user(77)
        .expect("query succeeds")
        .is_none());
}

#[test]
fn db_user_patch_returns_structured_validation_error() {
    let event = manual_event();
    let (_dir, api) = storage_api();

    let error = api
        .db_user_patch(
            &event,
            DbUserPatchRequest {
                patch: UserPatch {
                    user_id: 0,
                    username: None,
                    display_name: None,
                    seen_at: "".to_owned(),
                    warn_count: Some(-1),
                    shadowbanned: None,
                    reputation: None,
                    state_json: None,
                    updated_at: "".to_owned(),
                },
            },
        )
        .expect_err("invalid patch must fail");

    assert_eq!(error.kind, HostApiErrorKind::Validation);
    assert_eq!(error.operation, HostApiOperation::DbUserPatch);
    assert_eq!(
        error.detail,
        HostApiErrorDetail::InvalidField {
            field: "user_id".to_owned(),
            message: "must be non-zero".to_owned(),
        }
    );
}

#[test]
fn db_user_incr_updates_existing_user() {
    let event = manual_event();
    let (_dir, api) = storage_api();
    api.storage(HostApiOperation::DbUserIncr)
        .expect("storage")
        .upsert_user(&UserPatch {
            user_id: 77,
            username: Some("reply_user".to_owned()),
            display_name: Some("Reply User".to_owned()),
            seen_at: "2026-04-21T12:00:00Z".to_owned(),
            warn_count: Some(1),
            shadowbanned: Some(false),
            reputation: Some(4),
            state_json: None,
            updated_at: "2026-04-21T12:00:00Z".to_owned(),
        })
        .expect("seed user");

    let response = api
        .db_user_incr(
            &event,
            DbUserIncrRequest {
                user_id: 77,
                username: None,
                display_name: Some("Reply User Updated".to_owned()),
                seen_at: "2026-04-21T12:10:00Z".to_owned(),
                updated_at: "2026-04-21T12:10:00Z".to_owned(),
                warn_count_delta: 2,
                reputation_delta: -1,
                shadowbanned: Some(true),
                state_json: Some("{\"escalated\":true}".to_owned()),
            },
        )
        .expect("increment succeeds");

    assert_eq!(response.value.user.warn_count, 3);
    assert_eq!(response.value.user.reputation, 3);
    assert!(response.value.user.shadowbanned);
    assert_eq!(
        api.storage(HostApiOperation::DbUserIncr)
            .expect("storage")
            .get_user(77)
            .expect("query succeeds")
            .expect("user exists")
            .warn_count,
        3
    );
}

#[test]
fn db_user_incr_returns_structured_counter_error() {
    let event = manual_event();
    let (_dir, api) = storage_api();

    let error = api
        .db_user_incr(
            &event,
            DbUserIncrRequest {
                user_id: 77,
                username: None,
                display_name: None,
                seen_at: "2026-04-21T12:10:00Z".to_owned(),
                updated_at: "2026-04-21T12:10:00Z".to_owned(),
                warn_count_delta: -1,
                reputation_delta: 0,
                shadowbanned: None,
                state_json: None,
            },
        )
        .expect_err("negative increment from zero must fail");

    assert_eq!(error.kind, HostApiErrorKind::Validation);
    assert_eq!(
        error.detail,
        HostApiErrorDetail::InvalidCounterChange {
            field: "warn_count".to_owned(),
            current: 0,
            delta: -1,
        }
    );
}

#[test]
fn db_user_incr_dry_run_does_not_mutate_storage() {
    let event = manual_event();
    let (_dir, api) = dry_run_storage_api();

    let response = api
        .db_user_incr(
            &event,
            DbUserIncrRequest {
                user_id: 77,
                username: Some("dry_increment".to_owned()),
                display_name: Some("Dry Increment".to_owned()),
                seen_at: "2026-04-21T12:10:00Z".to_owned(),
                updated_at: "2026-04-21T12:10:00Z".to_owned(),
                warn_count_delta: 2,
                reputation_delta: 4,
                shadowbanned: Some(false),
                state_json: Some("{\"dry\":true}".to_owned()),
            },
        )
        .expect("dry-run increment succeeds");

    assert!(response.dry_run);
    assert_eq!(response.value.user.warn_count, 2);
    assert!(api
        .storage(HostApiOperation::DbUserIncr)
        .expect("storage")
        .get_user(77)
        .expect("query succeeds")
        .is_none());
}

#[test]
fn db_kv_set_dry_run_does_not_mutate_storage() {
    let event = manual_event();
    let (_dir, api) = dry_run_storage_api();

    let response = api
        .db_kv_set(
            &event,
            DbKvSetRequest {
                entry: KvEntry {
                    scope_kind: "chat".to_owned(),
                    scope_id: "-100123".to_owned(),
                    key: "policy".to_owned(),
                    value_json: "{\"mode\":\"strict\"}".to_owned(),
                    updated_at: "2026-04-21T12:00:00Z".to_owned(),
                },
            },
        )
        .expect("dry-run kv set succeeds");

    assert!(response.dry_run);
    assert_eq!(response.value.entry.key, "policy");
    assert!(api
        .storage(HostApiOperation::DbKvSet)
        .expect("storage")
        .get_kv("chat", "-100123", "policy")
        .expect("query succeeds")
        .is_none());
}

#[test]
fn db_kv_get_returns_seeded_entry() {
    let event = manual_event();
    let (_dir, api) = storage_api();
    api.storage(HostApiOperation::DbKvGet)
        .expect("storage")
        .set_kv(&KvEntry {
            scope_kind: "chat".to_owned(),
            scope_id: "-100123".to_owned(),
            key: "policy".to_owned(),
            value_json: "{\"mode\":\"strict\"}".to_owned(),
            updated_at: "2026-04-21T12:00:00Z".to_owned(),
        })
        .expect("seed kv");

    let response = api
        .db_kv_get(
            &event,
            DbKvGetRequest {
                scope_kind: "chat".to_owned(),
                scope_id: "-100123".to_owned(),
                key: "policy".to_owned(),
            },
        )
        .expect("kv get succeeds");

    assert_eq!(
        response.value.entry.expect("entry exists").value_json,
        "{\"mode\":\"strict\"}"
    );
}

#[test]
fn db_kv_get_rejects_blank_key() {
    let event = manual_event();
    let (_dir, api) = storage_api();

    let error = api
        .db_kv_get(
            &event,
            DbKvGetRequest {
                scope_kind: "chat".to_owned(),
                scope_id: "-100123".to_owned(),
                key: "   ".to_owned(),
            },
        )
        .expect_err("blank key must fail");

    assert_eq!(error.kind, HostApiErrorKind::Validation);
    assert_eq!(error.operation, HostApiOperation::DbKvGet);
    assert_eq!(
        error.detail,
        HostApiErrorDetail::InvalidField {
            field: "key".to_owned(),
            message: "must not be blank".to_owned(),
        }
    );
}

#[test]
fn db_kv_set_persists_entry_on_happy_path() {
    let event = manual_event();
    let (_dir, api) = storage_api();

    let response = api
        .db_kv_set(
            &event,
            DbKvSetRequest {
                entry: KvEntry {
                    scope_kind: "chat".to_owned(),
                    scope_id: "-100123".to_owned(),
                    key: "policy".to_owned(),
                    value_json: "{\"mode\":\"strict\"}".to_owned(),
                    updated_at: "2026-04-21T12:00:00Z".to_owned(),
                },
            },
        )
        .expect("kv set succeeds");

    assert!(!response.dry_run);
    assert_eq!(
        api.storage(HostApiOperation::DbKvSet)
            .expect("storage")
            .get_kv("chat", "-100123", "policy")
            .expect("query succeeds")
            .expect("entry exists")
            .value_json,
        "{\"mode\":\"strict\"}"
    );
}
