use super::super::test_support::{
    manual_event, seed_message_journal, storage_api, storage_api_with_registry,
};
use super::super::{
    HostApiErrorDetail, HostApiErrorKind, HostApiOperation, MsgByUserRequest, MsgWindowRequest,
};

#[test]
fn msg_window_returns_anchor_window() {
    let event = manual_event();
    let (_dir, api) = storage_api_with_registry(&["msg.history.read"], &[], false);
    seed_message_journal(&api);

    let response = api
        .msg_window(
            &event,
            MsgWindowRequest {
                chat_id: -100123,
                anchor_message_id: 81231,
                up: 2,
                down: 2,
                include_anchor: true,
            },
        )
        .expect("msg window succeeds");

    assert_eq!(response.operation, HostApiOperation::MsgWindow);
    assert_eq!(response.value.messages.len(), 5);
    assert_eq!(response.value.messages[2].message_id, 81231);
}

#[test]
fn msg_window_rejects_oversized_request() {
    let event = manual_event();
    let (_dir, api) = storage_api_with_registry(&["msg.history.read"], &[], false);

    let error = api
        .msg_window(
            &event,
            MsgWindowRequest {
                chat_id: -100123,
                anchor_message_id: 81231,
                up: 200,
                down: 1,
                include_anchor: true,
            },
        )
        .expect_err("oversized msg window must fail");

    assert_eq!(error.kind, HostApiErrorKind::Validation);
    assert_eq!(
        error.detail,
        HostApiErrorDetail::MessageWindowTooLarge {
            requested: 202,
            max: 200,
        }
    );
}

#[test]
fn msg_window_denies_when_capability_is_missing() {
    let event = manual_event();
    let (_dir, api) = storage_api_with_registry(&["audit.read"], &[], false);

    let error = api
        .msg_window(
            &event,
            MsgWindowRequest {
                chat_id: -100123,
                anchor_message_id: 81231,
                up: 1,
                down: 1,
                include_anchor: true,
            },
        )
        .expect_err("missing capability must fail");

    assert_eq!(error.kind, HostApiErrorKind::Denied);
    assert_eq!(error.operation, HostApiOperation::MsgWindow);
    assert_eq!(
        error.detail,
        HostApiErrorDetail::CapabilityDenied {
            capability: "msg.history.read".to_owned(),
            unit_id: "moderation.test".to_owned(),
        }
    );
}

#[test]
fn msg_window_fails_closed_when_unit_registry_is_unavailable() {
    let event = manual_event();
    let (_dir, api) = storage_api();

    let error = api
        .msg_window(
            &event,
            MsgWindowRequest {
                chat_id: -100123,
                anchor_message_id: 81231,
                up: 1,
                down: 1,
                include_anchor: true,
            },
        )
        .expect_err("missing registry must fail closed");

    assert_eq!(error.kind, HostApiErrorKind::Internal);
    assert_eq!(error.operation, HostApiOperation::MsgWindow);
    assert_eq!(
        error.detail,
        HostApiErrorDetail::ResourceUnavailable {
            resource: "unit_registry".to_owned(),
        }
    );
}

#[test]
fn msg_window_preserves_dry_run_metadata_for_reads() {
    let event = manual_event();
    let (_dir, api) = storage_api_with_registry(&["msg.history.read"], &[], true);
    seed_message_journal(&api);

    let response = api
        .msg_window(
            &event,
            MsgWindowRequest {
                chat_id: -100123,
                anchor_message_id: 81231,
                up: 1,
                down: 1,
                include_anchor: true,
            },
        )
        .expect("msg window succeeds");

    assert!(response.dry_run);
    assert_eq!(response.value.messages.len(), 3);
}

#[test]
fn msg_by_user_returns_recent_messages_for_user() {
    let event = manual_event();
    let (_dir, api) = storage_api_with_registry(&["msg.history.read"], &[], false);
    seed_message_journal(&api);

    let response = api
        .msg_by_user(
            &event,
            MsgByUserRequest {
                chat_id: -100123,
                user_id: 99887766,
                since: "2026-04-21T11:59:05Z".to_owned(),
                limit: 3,
            },
        )
        .expect("msg.by_user succeeds");

    assert_eq!(response.operation, HostApiOperation::MsgByUser);
    assert_eq!(response.value.messages.len(), 3);
    assert_eq!(response.value.messages[0].message_id, 81233);
}

#[test]
fn msg_by_user_rejects_invalid_since_timestamp() {
    let event = manual_event();
    let (_dir, api) = storage_api_with_registry(&["msg.history.read"], &[], false);

    let error = api
        .msg_by_user(
            &event,
            MsgByUserRequest {
                chat_id: -100123,
                user_id: 99887766,
                since: "yesterday".to_owned(),
                limit: 3,
            },
        )
        .expect_err("invalid since must fail");

    assert_eq!(error.kind, HostApiErrorKind::Validation);
    assert_eq!(error.operation, HostApiOperation::MsgByUser);
    assert!(
        matches!(
            error.detail,
            HostApiErrorDetail::InvalidField { ref field, .. } if field == "since"
        ),
        "unexpected error detail: {:?}",
        error.detail
    );
}

#[test]
fn msg_by_user_denies_when_capability_is_missing() {
    let event = manual_event();
    let (_dir, api) = storage_api_with_registry(&["audit.read"], &[], false);

    let error = api
        .msg_by_user(
            &event,
            MsgByUserRequest {
                chat_id: -100123,
                user_id: 99887766,
                since: "2026-04-21T11:59:05Z".to_owned(),
                limit: 3,
            },
        )
        .expect_err("missing capability must fail");

    assert_eq!(error.kind, HostApiErrorKind::Denied);
    assert_eq!(
        error.detail,
        HostApiErrorDetail::CapabilityDenied {
            capability: "msg.history.read".to_owned(),
            unit_id: "moderation.test".to_owned(),
        }
    );
}

#[test]
fn msg_by_user_preserves_dry_run_metadata_for_reads() {
    let event = manual_event();
    let (_dir, api) = storage_api_with_registry(&["msg.history.read"], &[], true);
    seed_message_journal(&api);

    let response = api
        .msg_by_user(
            &event,
            MsgByUserRequest {
                chat_id: -100123,
                user_id: 99887766,
                since: "2026-04-21T11:59:05Z".to_owned(),
                limit: 2,
            },
        )
        .expect("msg.by_user succeeds");

    assert!(response.dry_run);
    assert_eq!(response.value.messages.len(), 2);
}
