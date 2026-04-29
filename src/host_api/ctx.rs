use super::{
    CtxCurrentValue, CtxExpandReasonRequest, CtxParseDurationRequest, CtxResolveTargetRequest,
    HostApi, HostApiError, HostApiErrorDetail, HostApiOperation, HostApiResponse, validate_event,
};
use crate::event::EventContext;
use crate::parser::duration::{ParsedDuration, parse_duration};
use crate::parser::reason::ExpandedReason;
use crate::parser::target::{ResolvedTarget, parse_target_selector, resolve_target};

impl HostApi {
    pub(crate) fn ctx_current(
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

    pub(crate) fn ctx_resolve_target(
        &self,
        event: &EventContext,
        request: CtxResolveTargetRequest,
    ) -> Result<HostApiResponse<ResolvedTarget>, HostApiError> {
        validate_event(event, HostApiOperation::CtxResolveTarget)?;

        let positional = request
            .positional
            .as_deref()
            .map(|value| {
                parse_target_selector(value).map_err(|source| {
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
                parse_target_selector(value).map_err(|source| {
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
        let resolved = resolve_target(positional, selector_flag, event, |_| {
            request.implicit.clone()
        })
        .ok_or_else(|| {
            HostApiError::validation(
                HostApiOperation::CtxResolveTarget,
                HostApiErrorDetail::NoResolvableTarget,
            )
        })?;

        Ok(self.response(HostApiOperation::CtxResolveTarget, resolved))
    }

    pub(crate) fn ctx_parse_duration(
        &self,
        event: &EventContext,
        request: CtxParseDurationRequest,
    ) -> Result<HostApiResponse<ParsedDuration>, HostApiError> {
        validate_event(event, HostApiOperation::CtxParseDuration)?;

        let parsed = parse_duration(request.input.trim()).map_err(|source| {
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

    pub(crate) fn ctx_expand_reason(
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
}
