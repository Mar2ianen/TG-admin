use crate::host_api::test_support::{manual_event, seed_audit_entries, storage_api_with_registry};
use crate::host_api::{
    AuditCompensateRequest, AuditFindRequest, HostApiErrorDetail, HostApiErrorKind,
    HostApiOperation, JobScheduleAfterRequest,
};
use crate::storage::{AuditLogEntry, AuditLogFilter};
use serde_json::json;

#[test]
fn job_schedule_after_dry_run_validates_without_mutation() {
    let event = manual_event();
    let (_dir, api) = storage_api_with_registry(&["job.schedule"], &[], true);

    let response = api
        .job_schedule_after(
            &event,
            JobScheduleAfterRequest {
                delay: "7d".to_owned(),
                executor_unit: "moderation.mute_release".to_owned(),
                payload: json!({"kind":"host_op","op":"tg.send_ui"}),
                dedupe_key: Some("mute:99887766".to_owned()),
                max_retries: Some(2),
                audit_action_id: Some("act_1".to_owned()),
            },
        )
        .expect("dry-run schedule succeeds");

    assert!(response.dry_run);
    assert_eq!(response.value.job.status, "scheduled");
    assert!(api
        .storage(HostApiOperation::JobScheduleAfter)
        .expect("storage")
        .get_job(&response.value.job.job_id)
        .expect("job lookup succeeds")
        .is_none());
}

#[test]
fn job_schedule_after_rejects_too_distant_delay() {
    let event = manual_event();
    let (_dir, api) = storage_api_with_registry(&["job.schedule"], &[], false);

    let error = api
        .job_schedule_after(
            &event,
            JobScheduleAfterRequest {
                delay: "53w".to_owned(),
                executor_unit: "moderation.mute_release".to_owned(),
                payload: json!({"kind":"host_op"}),
                dedupe_key: None,
                max_retries: None,
                audit_action_id: None,
            },
        )
        .expect_err("delay beyond 365 days must fail");

    assert_eq!(error.kind, HostApiErrorKind::Validation);
    assert_eq!(
        error.detail,
        HostApiErrorDetail::JobTooFarInFuture {
            delay: "53w".to_owned(),
            max_days: 365,
        }
    );
}

#[test]
fn job_schedule_after_persists_job_on_happy_path() {
    let event = manual_event();
    let (_dir, api) = storage_api_with_registry(&["job.schedule"], &[], false);

    let response = api
        .job_schedule_after(
            &event,
            JobScheduleAfterRequest {
                delay: "2h".to_owned(),
                executor_unit: "moderation.mute_release".to_owned(),
                payload: json!({"kind":"host_op","op":"tg.send_ui"}),
                dedupe_key: Some("mute:99887766".to_owned()),
                max_retries: Some(2),
                audit_action_id: Some("act_1".to_owned()),
            },
        )
        .expect("job schedule succeeds");

    assert!(!response.dry_run);
    assert_eq!(response.value.job.executor_unit, "moderation.mute_release");
    assert!(api
        .storage(HostApiOperation::JobScheduleAfter)
        .expect("storage")
        .get_job(&response.value.job.job_id)
        .expect("lookup succeeds")
        .is_some());
}

#[test]
fn job_schedule_after_denies_when_capability_is_missing() {
    let event = manual_event();
    let (_dir, api) = storage_api_with_registry(&["audit.read"], &[], false);

    let error = api
        .job_schedule_after(
            &event,
            JobScheduleAfterRequest {
                delay: "2h".to_owned(),
                executor_unit: "moderation.mute_release".to_owned(),
                payload: json!({"kind":"host_op"}),
                dedupe_key: None,
                max_retries: None,
                audit_action_id: None,
            },
        )
        .expect_err("missing capability must fail");

    assert_eq!(error.kind, HostApiErrorKind::Denied);
    assert_eq!(
        error.detail,
        HostApiErrorDetail::CapabilityDenied {
            capability: "job.schedule".to_owned(),
            unit_id: "moderation.test".to_owned(),
        }
    );
}

#[test]
fn audit_find_returns_matching_entries() {
    let event = manual_event();
    let (_dir, api) = storage_api_with_registry(&["audit.read"], &[], false);
    seed_audit_entries(&api);

    let response = api
        .audit_find(
            &event,
            AuditFindRequest {
                filters: AuditLogFilter {
                    trigger_message_id: Some(81231),
                    ..AuditLogFilter::default()
                },
                limit: 10,
            },
        )
        .expect("audit.find succeeds");

    assert_eq!(response.operation, HostApiOperation::AuditFind);
    assert_eq!(response.value.entries.len(), 2);
    assert_eq!(response.value.entries[0].action_id, "act_2");
}

#[test]
fn audit_find_requires_at_least_one_filter() {
    let event = manual_event();
    let (_dir, api) = storage_api_with_registry(&["audit.read"], &[], false);

    let error = api
        .audit_find(
            &event,
            AuditFindRequest {
                filters: AuditLogFilter::default(),
                limit: 10,
            },
        )
        .expect_err("audit.find without filters must fail");

    assert_eq!(error.kind, HostApiErrorKind::Validation);
    assert_eq!(error.detail, HostApiErrorDetail::MissingAuditFilter);
}

#[test]
fn audit_find_denies_when_capability_is_missing() {
    let event = manual_event();
    let (_dir, api) = storage_api_with_registry(&["job.schedule"], &[], false);

    let error = api
        .audit_find(
            &event,
            AuditFindRequest {
                filters: AuditLogFilter {
                    trace_id: Some("trace-1".to_owned()),
                    ..AuditLogFilter::default()
                },
                limit: 10,
            },
        )
        .expect_err("missing capability must fail");

    assert_eq!(error.kind, HostApiErrorKind::Denied);
    assert_eq!(error.operation, HostApiOperation::AuditFind);
    assert_eq!(
        error.detail,
        HostApiErrorDetail::CapabilityDenied {
            capability: "audit.read".to_owned(),
            unit_id: "moderation.test".to_owned(),
        }
    );
}

#[test]
fn audit_find_preserves_dry_run_metadata_for_reads() {
    let event = manual_event();
    let (_dir, api) = storage_api_with_registry(&["audit.read"], &[], true);
    seed_audit_entries(&api);

    let response = api
        .audit_find(
            &event,
            AuditFindRequest {
                filters: AuditLogFilter {
                    trigger_message_id: Some(81231),
                    ..AuditLogFilter::default()
                },
                limit: 10,
            },
        )
        .expect("audit.find succeeds");

    assert!(response.dry_run);
    assert_eq!(response.value.entries.len(), 2);
}

#[test]
fn audit_compensate_appends_compensation_entry() {
    let event = manual_event();
    let (_dir, api) = storage_api_with_registry(&["audit.compensate", "audit.read"], &[], false);
    seed_audit_entries(&api);

    let response = api
        .audit_compensate(
            &event,
            AuditCompensateRequest {
                action_id: "act_1".to_owned(),
            },
        )
        .expect("audit.compensate succeeds");

    assert!(response.value.compensated);
    let new_action_id = response
        .value
        .new_action_id
        .clone()
        .expect("new action id returned");
    let inserted = api
        .storage(HostApiOperation::AuditCompensate)
        .expect("storage")
        .get_audit_entry(&new_action_id)
        .expect("lookup succeeds")
        .expect("compensation entry exists");
    assert_eq!(inserted.op, "audit.compensate");
    assert_eq!(inserted.target_id.as_deref(), Some("act_1"));
}

#[test]
fn audit_compensate_dry_run_does_not_append_entry() {
    let event = manual_event();
    let (_dir, api) = storage_api_with_registry(&["audit.compensate", "audit.read"], &[], true);
    seed_audit_entries(&api);

    let response = api
        .audit_compensate(
            &event,
            AuditCompensateRequest {
                action_id: "act_1".to_owned(),
            },
        )
        .expect("dry-run compensate succeeds");

    assert!(response.dry_run);
    let new_action_id = response
        .value
        .new_action_id
        .clone()
        .expect("predicted action id returned");
    assert!(api
        .storage(HostApiOperation::AuditCompensate)
        .expect("storage")
        .get_audit_entry(&new_action_id)
        .expect("lookup succeeds")
        .is_none());
}

#[test]
fn audit_compensate_rejects_already_compensated_action() {
    let event = manual_event();
    let (_dir, api) = storage_api_with_registry(&["audit.compensate", "audit.read"], &[], false);
    seed_audit_entries(&api);

    let first = api
        .audit_compensate(
            &event,
            AuditCompensateRequest {
                action_id: "act_1".to_owned(),
            },
        )
        .expect("first compensation succeeds");
    assert!(first.value.compensated);

    let error = api
        .audit_compensate(
            &event,
            AuditCompensateRequest {
                action_id: "act_1".to_owned(),
            },
        )
        .expect_err("second compensation must fail");

    assert_eq!(error.kind, HostApiErrorKind::Validation);
    assert_eq!(
        error.detail,
        HostApiErrorDetail::InvalidField {
            field: "action_id".to_owned(),
            message: "audit action `act_1` is already compensated".to_owned(),
        }
    );

    let compensations = api
        .storage(HostApiOperation::AuditCompensate)
        .expect("storage")
        .find_audit_by_idempotency_key("compensate:act_1")
        .expect("lookup succeeds");
    assert_eq!(compensations.len(), 1);
}

#[test]
fn audit_compensate_rejects_non_reversible_action() {
    let event = manual_event();
    let (_dir, api) = storage_api_with_registry(&["audit.compensate", "audit.read"], &[], false);
    seed_audit_entries(&api);

    let error = api
        .audit_compensate(
            &event,
            AuditCompensateRequest {
                action_id: "act_2".to_owned(),
            },
        )
        .expect_err("non-reversible action must fail");

    assert_eq!(error.kind, HostApiErrorKind::Validation);
    assert_eq!(
        error.detail,
        HostApiErrorDetail::InvalidField {
            field: "action_id".to_owned(),
            message: "audit action `act_2` is not reversible".to_owned(),
        }
    );
}

#[test]
fn audit_compensate_rejects_invalid_compensation_recipe() {
    let event = manual_event();
    let (_dir, api) = storage_api_with_registry(&["audit.compensate", "audit.read"], &[], false);
    seed_audit_entries(&api);
    api.storage(HostApiOperation::AuditCompensate)
        .expect("storage")
        .append_audit_entry(&AuditLogEntry {
            action_id: "act_invalid_recipe".to_owned(),
            trace_id: Some("trace-invalid".to_owned()),
            request_id: None,
            unit_name: "moderation.test".to_owned(),
            execution_mode: "manual".to_owned(),
            op: "mute".to_owned(),
            actor_user_id: Some(42),
            chat_id: Some(-100123),
            target_kind: Some("user".to_owned()),
            target_id: Some("99887766".to_owned()),
            trigger_message_id: Some(81231),
            idempotency_key: Some("idem-invalid".to_owned()),
            reversible: true,
            compensation_json: Some("{not-json}".to_owned()),
            args_json: "{\"duration\":\"7d\"}".to_owned(),
            result_json: Some("{\"ok\":true}".to_owned()),
            created_at: "2026-04-21T12:02:00Z".to_owned(),
        })
        .expect("invalid recipe audit entry");

    let error = api
        .audit_compensate(
            &event,
            AuditCompensateRequest {
                action_id: "act_invalid_recipe".to_owned(),
            },
        )
        .expect_err("invalid recipe must fail");

    assert_eq!(error.kind, HostApiErrorKind::Validation);
    assert!(matches!(
        error.detail,
        HostApiErrorDetail::InvalidField { ref field, ref message }
            if field == "compensation_json"
                && message.contains("invalid compensation recipe")
    ));

    let compensations = api
        .storage(HostApiOperation::AuditCompensate)
        .expect("storage")
        .find_audit_by_idempotency_key("compensate:act_invalid_recipe")
        .expect("lookup succeeds");
    assert!(compensations.is_empty());
}

#[test]
fn capability_denial_uses_structured_error_surface() {
    let event = manual_event();
    let (_dir, api) = storage_api_with_registry(&[], &["audit.compensate"], false);
    seed_audit_entries(&api);

    let error = api
        .audit_compensate(
            &event,
            AuditCompensateRequest {
                action_id: "act_1".to_owned(),
            },
        )
        .expect_err("denied capability must fail");

    assert_eq!(error.kind, HostApiErrorKind::Denied);
    assert_eq!(
        error.detail,
        HostApiErrorDetail::CapabilityDenied {
            capability: "audit.compensate".to_owned(),
            unit_id: "moderation.test".to_owned(),
        }
    );
}

#[test]
fn audit_compensate_returns_structured_unknown_action_error() {
    let event = manual_event();
    let (_dir, api) = storage_api_with_registry(&["audit.compensate"], &[], false);

    let error = api
        .audit_compensate(
            &event,
            AuditCompensateRequest {
                action_id: "missing".to_owned(),
            },
        )
        .expect_err("unknown action must fail");

    assert_eq!(error.kind, HostApiErrorKind::Validation);
    assert_eq!(error.operation, HostApiOperation::AuditCompensate);
    assert_eq!(
        error.detail,
        HostApiErrorDetail::UnknownAuditAction {
            action_id: "missing".to_owned(),
        }
    );
}
