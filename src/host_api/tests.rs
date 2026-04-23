use super::test_support::{
    manual_event, seed_audit_entries, seed_message_journal, storage_api, storage_api_with_registry,
    unit_registry_api,
};
use super::{
    AuditCompensateRequest, AuditFindRequest, CtxExpandReasonRequest, CtxParseDurationRequest,
    CtxResolveTargetRequest, HostApi, HostApiError, HostApiErrorDetail, HostApiErrorKind,
    HostApiOperation, HostApiRequest, HostApiValue, JobScheduleAfterRequest, MsgByUserRequest,
    MsgWindowRequest, UnitStatusEntry, UnitStatusRequest,
};
use crate::event::{EventContext, ExecutionMode, SystemContext, SystemOrigin, UpdateType};
use crate::parser::command::ReasonExpr;
use crate::parser::duration::{DurationParseError, DurationUnit, ParsedDuration};
use crate::parser::reason::{ExpandedReason, ReasonAliasDefinition, ReasonAliasRegistry};
use crate::parser::target::{ParsedTargetSelector, TargetParseError, TargetSource};
use crate::storage::{AuditLogEntry, AuditLogFilter};
use crate::unit::{
    ServiceSpec, TriggerSpec, UnitDefinition, UnitManifest, UnitRegistry, UnitStatus,
};
use serde_json::json;

mod tests_db {
    include!("tests_db.rs");
}

#[test]
fn ctx_current_returns_cloned_event_with_operation_metadata() {
    let event = manual_event();
    let api = HostApi::new(false);

    let response = api.ctx_current(&event).expect("ctx.current succeeds");

    assert_eq!(response.operation, HostApiOperation::CtxCurrent);
    assert!(!response.dry_run);
    assert_eq!(response.value.event.event_id, event.event_id);
    assert_eq!(response.value.event.execution_mode, ExecutionMode::Manual);
}

#[test]
fn call_surface_routes_ctx_current_request() {
    let event = manual_event();
    let api = HostApi::new(false);

    let response = api
        .call(&event, HostApiRequest::CtxCurrent)
        .expect("typed call succeeds");

    assert_eq!(response.operation, HostApiOperation::CtxCurrent);
    assert!(!response.dry_run);
    match response.value {
        HostApiValue::CtxCurrent(value) => assert_eq!(value.event.event_id, event.event_id),
        other => panic!("unexpected host api value: {other:?}"),
    }
}

#[test]
fn ctx_resolve_target_uses_parser_and_reply_fallback() {
    let event = manual_event();
    let api = HostApi::new(false);

    let explicit = api
        .ctx_resolve_target(
            &event,
            CtxResolveTargetRequest {
                positional: Some("@spam_user".to_owned()),
                selector_flag: None,
                implicit: None,
            },
        )
        .expect("explicit target resolves");
    assert_eq!(explicit.value.source, TargetSource::ExplicitPositional);
    assert_eq!(
        explicit.value.selector,
        ParsedTargetSelector::Username {
            username: "spam_user".to_owned(),
        }
    );

    let reply = api
        .ctx_resolve_target(
            &event,
            CtxResolveTargetRequest {
                positional: None,
                selector_flag: None,
                implicit: None,
            },
        )
        .expect("reply fallback resolves");
    assert_eq!(reply.value.source, TargetSource::ReplyContext);
    assert_eq!(
        reply.value.selector,
        ParsedTargetSelector::UserId { user_id: 77 }
    );
}

#[test]
fn ctx_resolve_target_returns_structured_parse_error() {
    let event = manual_event();
    let api = HostApi::new(false);

    let error = api
        .ctx_resolve_target(
            &event,
            CtxResolveTargetRequest {
                positional: Some("@bad-name".to_owned()),
                selector_flag: None,
                implicit: None,
            },
        )
        .expect_err("invalid target must fail");

    assert_eq!(error.kind, HostApiErrorKind::Parse);
    assert_eq!(error.operation, HostApiOperation::CtxResolveTarget);
    assert_eq!(
        error.detail,
        HostApiErrorDetail::InvalidTarget {
            value: "@bad-name".to_owned(),
            source: TargetParseError::InvalidUsername("@bad-name".to_owned()),
        }
    );
}

#[test]
fn ctx_parse_duration_returns_typed_value() {
    let event = manual_event();
    let api = HostApi::new(false);

    let response = api
        .ctx_parse_duration(
            &event,
            CtxParseDurationRequest {
                input: "15m".to_owned(),
            },
        )
        .expect("duration parses");

    assert_eq!(response.operation, HostApiOperation::CtxParseDuration);
    assert_eq!(
        response.value,
        ParsedDuration {
            value: 15,
            unit: DurationUnit::Minutes,
        }
    );
}

#[test]
fn ctx_parse_duration_returns_structured_error() {
    let event = manual_event();
    let api = HostApi::new(false);

    let error = api
        .ctx_parse_duration(
            &event,
            CtxParseDurationRequest {
                input: "30".to_owned(),
            },
        )
        .expect_err("missing unit must fail");

    assert_eq!(error.kind, HostApiErrorKind::Parse);
    assert_eq!(
        error.detail,
        HostApiErrorDetail::InvalidDuration {
            value: "30".to_owned(),
            source: DurationParseError::MissingUnit,
        }
    );
}

#[test]
fn ctx_expand_reason_uses_alias_registry() {
    let event = manual_event();
    let mut aliases = ReasonAliasRegistry::new();
    aliases.insert(
        "spam",
        ReasonAliasDefinition::new("spam or scam promotion")
            .with_rule_code("2.8")
            .with_title("Spam"),
    );
    let api = HostApi::new(false).with_reason_aliases(aliases);

    let response = api
        .ctx_expand_reason(
            &event,
            CtxExpandReasonRequest {
                reason: ReasonExpr::Alias("spam".to_owned()),
            },
        )
        .expect("reason expands");

    assert_eq!(response.operation, HostApiOperation::CtxExpandReason);
    assert_eq!(
        response.value,
        ExpandedReason::Alias {
            alias: "spam".to_owned(),
            definition: ReasonAliasDefinition {
                canonical: "spam or scam promotion".to_owned(),
                rule_code: Some("2.8".to_owned()),
                title: Some("Spam".to_owned()),
            },
        }
    );
}

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

#[test]
fn unit_status_returns_summary_and_specific_entry() {
    let event = manual_event();
    let api = unit_registry_api();

    let response = api
        .unit_status(
            &event,
            UnitStatusRequest {
                unit_id: Some("moderation.warn".to_owned()),
            },
        )
        .expect("unit status succeeds");

    assert_eq!(response.operation, HostApiOperation::UnitStatus);
    assert_eq!(response.value.summary.total_units, 2);
    assert_eq!(response.value.summary.active_units, 1);
    assert_eq!(response.value.summary.disabled_units, 1);
    assert_eq!(
        response.value.unit,
        Some(UnitStatusEntry {
            unit_id: "moderation.warn".to_owned(),
            status: UnitStatus::Active,
            enabled: Some(true),
            diagnostics: Vec::new(),
        })
    );
}

#[test]
fn unit_status_returns_structured_not_found_error() {
    let event = manual_event();
    let api = unit_registry_api();

    let error = api
        .unit_status(
            &event,
            UnitStatusRequest {
                unit_id: Some("missing.unit".to_owned()),
            },
        )
        .expect_err("unknown unit must fail");

    assert_eq!(error.kind, HostApiErrorKind::Validation);
    assert_eq!(
        error.detail,
        HostApiErrorDetail::UnknownUnit {
            unit_id: "missing.unit".to_owned(),
        }
    );
}

#[test]
fn unit_status_preserves_dry_run_metadata() {
    let active = UnitManifest::new(
        UnitDefinition::new("moderation.warn"),
        TriggerSpec::command(["warn"]),
        ServiceSpec::new("cargo run"),
    );
    let report = UnitRegistry::load_manifests(vec![active]);
    let api = HostApi::new(true).with_unit_registry(report.registry);
    let event = manual_event();

    let response = api
        .unit_status(&event, UnitStatusRequest { unit_id: None })
        .expect("unit status succeeds");

    assert!(response.dry_run);
    assert_eq!(response.value.summary.total_units, 1);
}

#[test]
fn call_surface_routes_db_and_unit_requests() {
    let event = manual_event();
    let api = unit_registry_api();

    let response = api
        .call(
            &event,
            HostApiRequest::UnitStatus(UnitStatusRequest { unit_id: None }),
        )
        .expect("typed call succeeds");

    match response.value {
        HostApiValue::UnitStatus(value) => assert_eq!(value.summary.total_units, 2),
        other => panic!("unexpected host api value: {other:?}"),
    }
}

#[test]
fn dry_run_is_preserved_in_ctx_responses() {
    let event = manual_event();
    let api = HostApi::new(true);

    let response = api
        .ctx_parse_duration(
            &event,
            CtxParseDurationRequest {
                input: "1h".to_owned(),
            },
        )
        .expect("ctx op still succeeds in dry run");

    assert!(response.dry_run);
    assert_eq!(response.operation, HostApiOperation::CtxParseDuration);
}

#[test]
fn invalid_event_maps_to_validation_error() {
    let mut event = EventContext::new(
        "evt_invalid",
        UpdateType::Message,
        ExecutionMode::Realtime,
        SystemContext::synthetic(SystemOrigin::Manual),
    );
    event.message = None;

    let api = HostApi::new(false);
    let error = api
        .ctx_current(&event)
        .expect_err("invalid event must fail");

    assert_eq!(error.kind, HostApiErrorKind::Validation);
    assert_eq!(error.operation, HostApiOperation::CtxCurrent);
    assert!(
        matches!(
            error,
            HostApiError {
                detail: HostApiErrorDetail::InvalidEventContext { .. },
                ..
            }
        ),
        "unexpected error shape: {error:?}"
    );
}
