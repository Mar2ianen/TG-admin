use std::rc::Rc;

use crate::event::EventContext;
use crate::parser::duration::DurationParser;
use crate::parser::reason::ReasonAliasRegistry;
use crate::parser::target::TargetSelectorParser;
use crate::storage::StorageConnection;
use crate::unit::UnitRegistry;

mod audit;
mod contract;
mod ctx;
mod db;
mod error;
mod history;
mod ml;
mod unit_status;
mod validation;

pub use contract::{
    AuditCompensateRequest, AuditCompensateValue, AuditFindRequest, AuditFindValue,
    CtxCurrentValue, CtxExpandReasonRequest, CtxParseDurationRequest, CtxResolveTargetRequest,
    DbKvGetRequest, DbKvGetValue, DbKvSetRequest, DbKvSetValue, DbUserGetRequest, DbUserGetValue,
    DbUserIncrRequest, DbUserIncrValue, DbUserPatchRequest, DbUserPatchValue, HostApiOperation,
    HostApiRequest, HostApiResponse, HostApiValue, JobScheduleAfterRequest, JobScheduleAfterValue,
    MsgByUserRequest, MsgByUserValue, MsgWindowRequest, MsgWindowValue, UnitStatusEntry,
    UnitStatusRequest, UnitStatusValue,
};
pub use error::{HostApiError, HostApiErrorDetail, HostApiErrorKind};
pub use ml::{
    MlChatCompletionsRequest, MlChatCompletionsValue, MlChatMessage, MlEmbedTextRequest,
    MlEmbedTextValue, MlHealthRequest, MlHealthValue, MlModelsRequest, MlModelsValue,
};
pub(crate) use validation::{
    apply_user_patch, execution_mode_label, required_capability, storage_error, to_rfc3339,
    user_patch_from_increment, validate_event, validate_kv_entry, validate_kv_key,
    validate_non_empty, validate_user_id, validate_user_incr_request, validate_user_patch,
};

#[derive(Debug, Clone)]
pub struct HostApi {
    dry_run: bool,
    storage: Option<Rc<StorageConnection>>,
    unit_registry: Option<Rc<UnitRegistry>>,
    target_parser: TargetSelectorParser,
    duration_parser: DurationParser,
    aliases: ReasonAliasRegistry,
}

impl HostApi {
    pub fn new(dry_run: bool) -> Self {
        Self {
            dry_run,
            storage: None,
            unit_registry: None,
            target_parser: TargetSelectorParser::new(),
            duration_parser: DurationParser::new(),
            aliases: ReasonAliasRegistry::new(),
        }
    }

    pub fn with_reason_aliases(mut self, aliases: ReasonAliasRegistry) -> Self {
        self.aliases = aliases;
        self
    }

    pub fn with_storage(mut self, storage: StorageConnection) -> Self {
        self.storage = Some(Rc::new(storage));
        self
    }

    pub fn with_storage_handle(mut self, storage: Rc<StorageConnection>) -> Self {
        self.storage = Some(storage);
        self
    }

    pub fn with_unit_registry(mut self, registry: UnitRegistry) -> Self {
        self.unit_registry = Some(Rc::new(registry));
        self
    }

    pub fn with_unit_registry_handle(mut self, registry: Rc<UnitRegistry>) -> Self {
        self.unit_registry = Some(registry);
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
            HostApiRequest::DbUserGet(request) => self
                .db_user_get(event, request)
                .map(|response| response.map(HostApiValue::DbUserGet)),
            HostApiRequest::DbUserPatch(request) => self
                .db_user_patch(event, request)
                .map(|response| response.map(HostApiValue::DbUserPatch)),
            HostApiRequest::DbUserIncr(request) => self
                .db_user_incr(event, request)
                .map(|response| response.map(HostApiValue::DbUserIncr)),
            HostApiRequest::DbKvGet(request) => self
                .db_kv_get(event, request)
                .map(|response| response.map(HostApiValue::DbKvGet)),
            HostApiRequest::DbKvSet(request) => self
                .db_kv_set(event, request)
                .map(|response| response.map(HostApiValue::DbKvSet)),
            HostApiRequest::MsgWindow(request) => self
                .msg_window(event, request)
                .map(|response| response.map(HostApiValue::MsgWindow)),
            HostApiRequest::MsgByUser(request) => self
                .msg_by_user(event, request)
                .map(|response| response.map(HostApiValue::MsgByUser)),
            HostApiRequest::JobScheduleAfter(request) => self
                .job_schedule_after(event, request)
                .map(|response| response.map(HostApiValue::JobScheduleAfter)),
            HostApiRequest::AuditFind(request) => self
                .audit_find(event, request)
                .map(|response| response.map(HostApiValue::AuditFind)),
            HostApiRequest::AuditCompensate(request) => self
                .audit_compensate(event, request)
                .map(|response| response.map(HostApiValue::AuditCompensate)),
            HostApiRequest::UnitStatus(request) => self
                .unit_status(event, request)
                .map(|response| response.map(HostApiValue::UnitStatus)),
            HostApiRequest::MlHealth(request) => self
                .ml_health(event, request)
                .map(|response| response.map(HostApiValue::MlHealth)),
            HostApiRequest::MlEmbedText(request) => self
                .ml_embed_text(event, request)
                .map(|response| response.map(HostApiValue::MlEmbedText)),
            HostApiRequest::MlChatCompletions(request) => self
                .ml_chat_completions(event, request)
                .map(|response| response.map(HostApiValue::MlChatCompletions)),
            HostApiRequest::MlModels(request) => self
                .ml_models(event, request)
                .map(|response| response.map(HostApiValue::MlModels)),
        }
    }

    fn storage(&self, operation: HostApiOperation) -> Result<&StorageConnection, HostApiError> {
        self.storage.as_deref().ok_or_else(|| {
            HostApiError::internal(
                operation,
                HostApiErrorDetail::ResourceUnavailable {
                    resource: "storage".to_owned(),
                },
            )
        })
    }

    fn unit_registry(&self, operation: HostApiOperation) -> Result<&UnitRegistry, HostApiError> {
        self.unit_registry.as_deref().ok_or_else(|| {
            HostApiError::internal(
                operation,
                HostApiErrorDetail::ResourceUnavailable {
                    resource: "unit_registry".to_owned(),
                },
            )
        })
    }

    fn response<T>(&self, operation: HostApiOperation, value: T) -> HostApiResponse<T> {
        HostApiResponse {
            operation,
            dry_run: self.dry_run,
            value,
        }
    }

    fn require_operation_capability(
        &self,
        event: &EventContext,
        operation: HostApiOperation,
    ) -> Result<(), HostApiError> {
        if let Some(capability) = required_capability(operation) {
            self.require_capability(event, operation, capability)?;
        }

        Ok(())
    }

    fn require_capability(
        &self,
        event: &EventContext,
        operation: HostApiOperation,
        capability: &'static str,
    ) -> Result<(), HostApiError> {
        let Some(unit) = event.system.unit.as_ref() else {
            return Err(HostApiError::denied(
                operation,
                HostApiErrorDetail::CapabilityDenied {
                    capability: capability.to_owned(),
                    unit_id: "<unknown>".to_owned(),
                },
            ));
        };
        let Some(registry) = self.unit_registry.as_deref() else {
            return Err(HostApiError::internal(
                operation,
                HostApiErrorDetail::ResourceUnavailable {
                    resource: "unit_registry".to_owned(),
                },
            ));
        };
        let descriptor = registry.get(&unit.id).ok_or_else(|| {
            HostApiError::validation(
                operation,
                HostApiErrorDetail::UnknownUnit {
                    unit_id: unit.id.clone(),
                },
            )
        })?;
        let capabilities = descriptor
            .manifest
            .as_ref()
            .map(|manifest| &manifest.capabilities)
            .ok_or_else(|| {
                HostApiError::validation(
                    operation,
                    HostApiErrorDetail::UnknownUnit {
                        unit_id: unit.id.clone(),
                    },
                )
            })?;

        if capabilities.deny.iter().any(|value| value == capability) {
            return Err(HostApiError::denied(
                operation,
                HostApiErrorDetail::CapabilityDenied {
                    capability: capability.to_owned(),
                    unit_id: unit.id.clone(),
                },
            ));
        }
        if !capabilities.allow.is_empty()
            && !capabilities.allow.iter().any(|value| value == capability)
        {
            return Err(HostApiError::denied(
                operation,
                HostApiErrorDetail::CapabilityDenied {
                    capability: capability.to_owned(),
                    unit_id: unit.id.clone(),
                },
            ));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests;
