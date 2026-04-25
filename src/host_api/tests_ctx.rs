use crate::event::{ExecutionMode, SystemContext, SystemOrigin, UpdateType};
use crate::host_api::test_support::manual_event;
use crate::host_api::{
    CtxExpandReasonRequest, CtxParseDurationRequest, CtxResolveTargetRequest, HostApi,
    HostApiError, HostApiErrorDetail, HostApiErrorKind, HostApiOperation, HostApiRequest,
    HostApiValue,
};
use crate::parser::command::ReasonExpr;
use crate::parser::duration::{DurationParseError, DurationUnit, ParsedDuration};
use crate::parser::reason::{ExpandedReason, ReasonAliasDefinition, ReasonAliasRegistry};
use crate::parser::target::{ParsedTargetSelector, TargetParseError, TargetSource};

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
    let mut event = crate::event::EventContext::new(
        "evt_invalid",
        UpdateType::Message,
        crate::event::ExecutionMode::Realtime,
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
