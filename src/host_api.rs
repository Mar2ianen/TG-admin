use crate::event::EventContext;
use crate::parser::command::ReasonExpr;
use crate::parser::duration::{DurationParseError, DurationParser, ParsedDuration};
use crate::parser::reason::{ExpandedReason, ReasonAliasRegistry};
use crate::parser::target::{
    ParsedTargetSelector, ResolvedTarget, TargetParseError, TargetSelectorParser, resolve_target,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone)]
pub struct HostApi {
    dry_run: bool,
    target_parser: TargetSelectorParser,
    duration_parser: DurationParser,
    aliases: ReasonAliasRegistry,
}

impl HostApi {
    pub fn new(dry_run: bool) -> Self {
        Self {
            dry_run,
            target_parser: TargetSelectorParser::new(),
            duration_parser: DurationParser::new(),
            aliases: ReasonAliasRegistry::new(),
        }
    }

    pub fn with_reason_aliases(mut self, aliases: ReasonAliasRegistry) -> Self {
        self.aliases = aliases;
        self
    }

    pub fn dry_run(&self) -> bool {
        self.dry_run
    }

    pub fn call(
        &self,
        event: &EventContext,
        request: HostApiRequest,
    ) -> Result<HostApiResponse<HostApiValue>, HostApiError> {
        match request {
            HostApiRequest::CtxCurrent => self
                .ctx_current(event)
                .map(|response| response.map(|value| HostApiValue::CtxCurrent(Box::new(value)))),
            HostApiRequest::CtxResolveTarget(request) => self
                .ctx_resolve_target(event, request)
                .map(|response| response.map(HostApiValue::ResolvedTarget)),
            HostApiRequest::CtxParseDuration(request) => self
                .ctx_parse_duration(event, request)
                .map(|response| response.map(HostApiValue::ParsedDuration)),
            HostApiRequest::CtxExpandReason(request) => self
                .ctx_expand_reason(event, request)
                .map(|response| response.map(HostApiValue::ExpandedReason)),
        }
    }

    pub fn ctx_current(
        &self,
        event: &EventContext,
    ) -> Result<HostApiResponse<CtxCurrentValue>, HostApiError> {
        validate_event(event, HostApiOperation::CtxCurrent)?;

        Ok(self.response(
            HostApiOperation::CtxCurrent,
            CtxCurrentValue {
                event: event.clone(),
            },
        ))
    }

    pub fn ctx_resolve_target(
        &self,
        event: &EventContext,
        request: CtxResolveTargetRequest,
    ) -> Result<HostApiResponse<ResolvedTarget>, HostApiError> {
        validate_event(event, HostApiOperation::CtxResolveTarget)?;

        let positional = request
            .positional
            .as_deref()
            .map(|value| {
                self.target_parser.parse(value).map_err(|source| {
                    HostApiError::parse(
                        HostApiOperation::CtxResolveTarget,
                        HostApiErrorDetail::InvalidTarget {
                            value: value.to_owned(),
                            source,
                        },
                    )
                })
            })
            .transpose()?;
        let selector_flag = request
            .selector_flag
            .as_deref()
            .map(|value| {
                self.target_parser.parse(value).map_err(|source| {
                    HostApiError::parse(
                        HostApiOperation::CtxResolveTarget,
                        HostApiErrorDetail::InvalidTarget {
                            value: value.to_owned(),
                            source,
                        },
                    )
                })
            })
            .transpose()?;
        let resolved = resolve_target(positional, selector_flag, event, |_| request.implicit.clone())
            .ok_or_else(|| {
                HostApiError::validation(
                    HostApiOperation::CtxResolveTarget,
                    HostApiErrorDetail::NoResolvableTarget,
                )
            })?;

        Ok(self.response(HostApiOperation::CtxResolveTarget, resolved))
    }

    pub fn ctx_parse_duration(
        &self,
        event: &EventContext,
        request: CtxParseDurationRequest,
    ) -> Result<HostApiResponse<ParsedDuration>, HostApiError> {
        validate_event(event, HostApiOperation::CtxParseDuration)?;

        let parsed = self
            .duration_parser
            .parse(request.input.trim())
            .map_err(|source| {
                HostApiError::parse(
                    HostApiOperation::CtxParseDuration,
                    HostApiErrorDetail::InvalidDuration {
                        value: request.input,
                        source,
                    },
                )
            })?;

        Ok(self.response(HostApiOperation::CtxParseDuration, parsed))
    }

    pub fn ctx_expand_reason(
        &self,
        event: &EventContext,
        request: CtxExpandReasonRequest,
    ) -> Result<HostApiResponse<ExpandedReason>, HostApiError> {
        validate_event(event, HostApiOperation::CtxExpandReason)?;

        let expanded = self
            .aliases
            .expand_reason(Some(&request.reason))
            .ok_or_else(|| {
                HostApiError::internal(
                    HostApiOperation::CtxExpandReason,
                    HostApiErrorDetail::ReasonExpansionUnavailable,
                )
            })?;

        Ok(self.response(HostApiOperation::CtxExpandReason, expanded))
    }

    fn response<T>(&self, operation: HostApiOperation, value: T) -> HostApiResponse<T> {
        HostApiResponse {
            operation,
            dry_run: self.dry_run,
            value,
        }
    }
}

fn validate_event(event: &EventContext, operation: HostApiOperation) -> Result<(), HostApiError> {
    event.validate_invariants().map_err(|source| {
        HostApiError::validation(
            operation,
            HostApiErrorDetail::InvalidEventContext {
                message: source.to_string(),
            },
        )
    })
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub enum HostApiRequest {
    CtxCurrent,
    CtxResolveTarget(CtxResolveTargetRequest),
    CtxParseDuration(CtxParseDurationRequest),
    CtxExpandReason(CtxExpandReasonRequest),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HostApiValue {
    CtxCurrent(Box<CtxCurrentValue>),
    ResolvedTarget(ResolvedTarget),
    ParsedDuration(ParsedDuration),
    ExpandedReason(ExpandedReason),
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostApiOperation {
    CtxCurrent,
    CtxResolveTarget,
    CtxParseDuration,
    CtxExpandReason,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct HostApiResponse<T> {
    pub operation: HostApiOperation,
    pub dry_run: bool,
    pub value: T,
}

impl<T> HostApiResponse<T> {
    fn map<U>(self, map: impl FnOnce(T) -> U) -> HostApiResponse<U> {
        HostApiResponse {
            operation: self.operation,
            dry_run: self.dry_run,
            value: map(self.value),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CtxCurrentValue {
    pub event: EventContext,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct CtxResolveTargetRequest {
    pub positional: Option<String>,
    pub selector_flag: Option<String>,
    pub implicit: Option<ParsedTargetSelector>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct CtxParseDurationRequest {
    pub input: String,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct CtxExpandReasonRequest {
    pub reason: ReasonExpr,
}

#[derive(Debug, Clone, Error, Eq, PartialEq, Serialize, Deserialize)]
#[error("{kind:?} host api error in {operation:?}: {detail}")]
pub struct HostApiError {
    pub operation: HostApiOperation,
    pub kind: HostApiErrorKind,
    pub detail: HostApiErrorDetail,
}

impl HostApiError {
    fn validation(operation: HostApiOperation, detail: HostApiErrorDetail) -> Self {
        Self {
            operation,
            kind: HostApiErrorKind::Validation,
            detail,
        }
    }

    fn parse(operation: HostApiOperation, detail: HostApiErrorDetail) -> Self {
        Self {
            operation,
            kind: HostApiErrorKind::Parse,
            detail,
        }
    }

    fn internal(operation: HostApiOperation, detail: HostApiErrorDetail) -> Self {
        Self {
            operation,
            kind: HostApiErrorKind::Internal,
            detail,
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostApiErrorKind {
    Validation,
    Parse,
    Denied,
    Internal,
}

#[derive(Debug, Clone, Error, Eq, PartialEq, Serialize, Deserialize)]
pub enum HostApiErrorDetail {
    #[error("invalid event context: {message}")]
    InvalidEventContext { message: String },
    #[error("invalid target `{value}`: {source}")]
    InvalidTarget {
        value: String,
        source: TargetParseError,
    },
    #[error("no target could be resolved from request or event context")]
    NoResolvableTarget,
    #[error("invalid duration `{value}`: {source}")]
    InvalidDuration {
        value: String,
        source: DurationParseError,
    },
    #[error("reason expansion unexpectedly returned no result")]
    ReasonExpansionUnavailable,
}

#[cfg(test)]
mod tests {
    use super::{
        CtxExpandReasonRequest, CtxParseDurationRequest, CtxResolveTargetRequest, HostApi,
        HostApiError, HostApiErrorDetail, HostApiErrorKind, HostApiOperation, HostApiRequest,
        HostApiValue,
    };
    use crate::event::{
        ChatContext, EventContext, EventNormalizer, ExecutionMode, ManualInvocationInput,
        ReplyContext, SystemContext, SystemOrigin, UnitContext, UpdateType,
    };
    use crate::parser::command::ReasonExpr;
    use crate::parser::duration::{DurationParseError, DurationUnit, ParsedDuration};
    use crate::parser::reason::{ExpandedReason, ReasonAliasDefinition, ReasonAliasRegistry};
    use crate::parser::target::{ParsedTargetSelector, TargetParseError, TargetSource};
    use chrono::{TimeZone, Utc};

    fn ts() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 4, 21, 12, 0, 0)
            .single()
            .expect("valid timestamp")
    }

    fn manual_event() -> EventContext {
        let normalizer = EventNormalizer::new();
        let mut input = ManualInvocationInput::new(
            UnitContext::new("moderation.test").with_trigger("manual"),
            "/warn @spam spam",
        );
        input.event_id = Some("evt_host_api_manual".to_owned());
        input.received_at = ts();
        input.chat = Some(ChatContext {
            id: -100123,
            chat_type: "supergroup".to_owned(),
            title: Some("Moderation HQ".to_owned()),
            username: Some("mod_hq".to_owned()),
            thread_id: Some(7),
        });
        input.reply = Some(ReplyContext {
            message_id: 99,
            sender_user_id: Some(77),
            sender_username: Some("reply_user".to_owned()),
            text: Some("reply".to_owned()),
            has_media: false,
        });

        normalizer
            .normalize_manual(input)
            .expect("manual event normalizes")
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
        let error = api.ctx_current(&event).expect_err("invalid event must fail");

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
}
