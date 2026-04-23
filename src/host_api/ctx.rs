use super::{
    CtxCurrentValue, CtxResolveTargetRequest, HostApi, HostApiError, HostApiErrorDetail,
    HostApiOperation, HostApiResponse, validate_event,
};
use crate::event::EventContext;
use crate::parser::target::{ResolvedTarget, resolve_target};

impl HostApi {
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
}
