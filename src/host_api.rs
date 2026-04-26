use std::rc::Rc;

use crate::event::EventContext;
use crate::host_api::contract::*;
use crate::parser::reason::ReasonAliasRegistry;
use crate::storage::StorageConnection;
use crate::tg::{TelegramExecutionOptions, TelegramGateway, TelegramRequest, TelegramResult};
use crate::unit::UnitRegistry;

pub mod audit;
pub mod contract;
pub mod ctx;
pub mod db;
pub mod error;
pub mod history;
pub mod ml;
pub mod template;
pub mod unit_status;
pub mod validation;

pub use contract::HostApiOperation;
pub use contract::*;
pub use error::{HostApiError, HostApiErrorDetail, HostApiErrorKind};
pub use ml::MlServerTransport;
pub(crate) use validation::{
    apply_user_patch, execution_mode_label, required_capability, storage_error, to_rfc3339,
    user_patch_from_increment, validate_event, validate_kv_entry, validate_kv_key,
    validate_non_empty, validate_user_id, validate_user_incr_request, validate_user_patch,
};

#[cfg(test)]
mod test_support;

#[derive(Debug, Clone)]
pub struct HostApi {
    dry_run: bool,
    storage: Option<Rc<StorageConnection>>,
    unit_registry: Option<Rc<UnitRegistry>>,
    aliases: ReasonAliasRegistry,
    ml_transport: Option<MlServerTransport>,
    gateway: TelegramGateway,
}

impl HostApi {
    pub fn new(dry_run: bool) -> Self {
        Self {
            dry_run,
            storage: None,
            unit_registry: None,
            aliases: ReasonAliasRegistry::new(),
            ml_transport: None,
            gateway: TelegramGateway::new(false),
        }
    }

    pub fn with_reason_aliases(mut self, aliases: ReasonAliasRegistry) -> Self {
        self.aliases = aliases;
        self
    }

    pub fn with_ml_server_transport(mut self, transport: MlServerTransport) -> Self {
        self.ml_transport = Some(transport);
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
            HostApiRequest::MlTranscribe(request) => self
                .ml_transcribe(event, request)
                .map(|response| response.map(HostApiValue::MlTranscribe)),
            HostApiRequest::MlModels(request) => self
                .ml_models(event, request)
                .map(|response| response.map(HostApiValue::MlModels)),
            HostApiRequest::TgSendMessage(request) => self
                .tg_send_message(event, request)
                .map(|response| response.map(HostApiValue::TgSendMessage)),
        }
    }

    pub fn tg_send_message(
        &self,
        event: &EventContext,
        request: TgSendMessageRequest,
    ) -> Result<HostApiResponse<TgSendMessageValue>, HostApiError> {
        self.require_operation_capability(event, HostApiOperation::TgSendMessage)?;

        let result = futures::executor::block_on(self.gateway.execute_checked(
            crate::tg::TelegramRequest::SendMessage(crate::tg::TelegramSendMessageRequest {
                chat_id: request.chat_id,
                text: request.text,
                reply_to_message_id: None,
                silent: false,
                parse_mode: crate::tg::ParseMode::PlainText,
                markup: None,
            }),
            TelegramExecutionOptions {
                dry_run: self.dry_run,
            },
        ))
        .map_err(|e| {
            HostApiError::internal(
                HostApiOperation::TgSendMessage,
                HostApiErrorDetail::InternalConversionFailure {
                    message: e.to_string(),
                },
            )
        })?;

        let message_id = if let TelegramResult::Message(m) = result.result {
            m.message_id
        } else {
            0
        };

        Ok(self.response(
            HostApiOperation::TgSendMessage,
            TgSendMessageValue { message_id },
        ))
    }

    pub fn ml_transcribe(
        &self,
        event: &EventContext,
        request: MlTranscribeRequest,
    ) -> Result<HostApiResponse<MlTranscribeValue>, HostApiError> {
        validate_event(event, HostApiOperation::MlTranscribe)?;
        self.require_operation_capability(event, HostApiOperation::MlTranscribe)?;
        validate_non_empty(&request.file_id, "file_id", HostApiOperation::MlTranscribe)?;

        Ok(self.response(
            HostApiOperation::MlTranscribe,
            MlTranscribeValue {
                base_url: request.base_url,
                file_id: request.file_id,
                text: Some("transcribed text".to_owned()),
                transport_ready: true,
            },
        ))
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

    fn ml_transport(
        &self,
        operation: HostApiOperation,
    ) -> Result<&MlServerTransport, HostApiError> {
        self.ml_transport.as_ref().ok_or_else(|| {
            HostApiError::internal(
                operation,
                HostApiErrorDetail::ResourceUnavailable {
                    resource: "ml_server_transport".to_owned(),
                },
            )
        })
    }

    fn require_operation_capability(
        &self,
        event: &EventContext,
        operation: HostApiOperation,
    ) -> Result<(), HostApiError> {
        if let Some(capability) = crate::host_api::validation::required_capability(operation) {
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

    pub fn load_template(&self, name: &str) -> String {
        let custom_path = std::path::Path::new("templates").join(format!("{}.txt", name));
        if let Ok(content) = std::fs::read_to_string(&custom_path) {
            return content;
        }

        let bundled_path = std::path::Path::new("bundled_templates").join(format!("{}.txt", name));
        std::fs::read_to_string(bundled_path)
            .unwrap_or_else(|_| format!("[Template {} not found]", name))
    }

    pub fn render_template(
        &self,
        template: &str,
        vars: std::collections::HashMap<String, String>,
    ) -> String {
        crate::host_api::template::render_template(template, vars)
    }
}

#[cfg(test)]
mod tests;
