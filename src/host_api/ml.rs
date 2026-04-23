use super::{
    validate_event, validate_non_empty, HostApi, HostApiError, HostApiErrorDetail,
    HostApiOperation, HostApiResponse,
};
use crate::event::EventContext;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct MlHealthRequest {
    pub base_url: Option<String>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct MlEmbedTextRequest {
    pub base_url: Option<String>,
    pub input: Vec<String>,
    pub model: Option<String>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct MlChatCompletionsRequest {
    pub base_url: Option<String>,
    pub model: String,
    pub messages: Vec<MlChatMessage>,
    pub max_tokens: Option<u32>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct MlModelsRequest {
    pub base_url: Option<String>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct MlChatMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct MlHealthValue {
    pub base_url: Option<String>,
    pub transport_ready: bool,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct MlEmbedTextValue {
    pub base_url: Option<String>,
    pub model: Option<String>,
    pub input_count: usize,
    pub transport_ready: bool,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct MlChatCompletionsValue {
    pub base_url: Option<String>,
    pub model: String,
    pub message_count: usize,
    pub max_tokens: Option<u32>,
    pub transport_ready: bool,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct MlModelsValue {
    pub base_url: Option<String>,
    pub transport_ready: bool,
}

impl HostApi {
    pub fn ml_health(
        &self,
        event: &EventContext,
        request: MlHealthRequest,
    ) -> Result<HostApiResponse<MlHealthValue>, HostApiError> {
        validate_event(event, HostApiOperation::MlHealth)?;
        self.require_operation_capability(event, HostApiOperation::MlHealth)?;
        validate_optional_base_url(request.base_url.as_deref(), HostApiOperation::MlHealth)?;

        let value = MlHealthValue {
            base_url: request.base_url,
            transport_ready: false,
        };
        if self.dry_run() {
            return Ok(self.response(HostApiOperation::MlHealth, value));
        }

        Err(ml_runtime_unavailable(HostApiOperation::MlHealth))
    }

    pub fn ml_embed_text(
        &self,
        event: &EventContext,
        request: MlEmbedTextRequest,
    ) -> Result<HostApiResponse<MlEmbedTextValue>, HostApiError> {
        validate_event(event, HostApiOperation::MlEmbedText)?;
        self.require_operation_capability(event, HostApiOperation::MlEmbedText)?;
        validate_optional_base_url(request.base_url.as_deref(), HostApiOperation::MlEmbedText)?;
        validate_ml_embed_request(&request)?;

        let value = MlEmbedTextValue {
            base_url: request.base_url,
            model: request.model,
            input_count: request.input.len(),
            transport_ready: false,
        };
        if self.dry_run() {
            return Ok(self.response(HostApiOperation::MlEmbedText, value));
        }

        Err(ml_runtime_unavailable(HostApiOperation::MlEmbedText))
    }

    pub fn ml_chat_completions(
        &self,
        event: &EventContext,
        request: MlChatCompletionsRequest,
    ) -> Result<HostApiResponse<MlChatCompletionsValue>, HostApiError> {
        validate_event(event, HostApiOperation::MlChatCompletions)?;
        self.require_operation_capability(event, HostApiOperation::MlChatCompletions)?;
        validate_optional_base_url(
            request.base_url.as_deref(),
            HostApiOperation::MlChatCompletions,
        )?;
        validate_ml_chat_request(&request)?;

        let value = MlChatCompletionsValue {
            base_url: request.base_url,
            model: request.model,
            message_count: request.messages.len(),
            max_tokens: request.max_tokens,
            transport_ready: false,
        };
        if self.dry_run() {
            return Ok(self.response(HostApiOperation::MlChatCompletions, value));
        }

        Err(ml_runtime_unavailable(HostApiOperation::MlChatCompletions))
    }

    pub fn ml_models(
        &self,
        event: &EventContext,
        request: MlModelsRequest,
    ) -> Result<HostApiResponse<MlModelsValue>, HostApiError> {
        validate_event(event, HostApiOperation::MlModels)?;
        self.require_operation_capability(event, HostApiOperation::MlModels)?;
        validate_optional_base_url(request.base_url.as_deref(), HostApiOperation::MlModels)?;

        let value = MlModelsValue {
            base_url: request.base_url,
            transport_ready: false,
        };
        if self.dry_run() {
            return Ok(self.response(HostApiOperation::MlModels, value));
        }

        Err(ml_runtime_unavailable(HostApiOperation::MlModels))
    }
}

fn validate_optional_base_url(
    base_url: Option<&str>,
    operation: HostApiOperation,
) -> Result<(), HostApiError> {
    if let Some(value) = base_url {
        validate_non_empty(value, "base_url", operation)?;
    }

    Ok(())
}

fn validate_ml_embed_request(request: &MlEmbedTextRequest) -> Result<(), HostApiError> {
    if request.input.is_empty() {
        return Err(HostApiError::validation(
            HostApiOperation::MlEmbedText,
            HostApiErrorDetail::InvalidField {
                field: "input".to_owned(),
                message: "at least one input string is required".to_owned(),
            },
        ));
    }

    for value in &request.input {
        validate_non_empty(value, "input", HostApiOperation::MlEmbedText)?;
    }
    if let Some(model) = request.model.as_deref() {
        validate_non_empty(model, "model", HostApiOperation::MlEmbedText)?;
    }

    Ok(())
}

fn validate_ml_chat_request(request: &MlChatCompletionsRequest) -> Result<(), HostApiError> {
    validate_non_empty(&request.model, "model", HostApiOperation::MlChatCompletions)?;
    if request.messages.is_empty() {
        return Err(HostApiError::validation(
            HostApiOperation::MlChatCompletions,
            HostApiErrorDetail::InvalidField {
                field: "messages".to_owned(),
                message: "at least one chat message is required".to_owned(),
            },
        ));
    }

    for message in &request.messages {
        validate_non_empty(
            &message.role,
            "messages.role",
            HostApiOperation::MlChatCompletions,
        )?;
        validate_non_empty(
            &message.content,
            "messages.content",
            HostApiOperation::MlChatCompletions,
        )?;
    }

    Ok(())
}

fn ml_runtime_unavailable(operation: HostApiOperation) -> HostApiError {
    HostApiError::internal(
        operation,
        HostApiErrorDetail::ResourceUnavailable {
            resource: "ml_server_transport".to_owned(),
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{
        ChatContext, EventContext, EventNormalizer, ExecutionMode, ManualInvocationInput,
        ReplyContext, SystemContext, SystemOrigin, UnitContext, UpdateType,
    };
    use crate::host_api::{HostApiRequest, HostApiValue};
    use crate::storage::Storage;
    use crate::unit::{
        CapabilitiesSpec, ServiceSpec, TriggerSpec, UnitDefinition, UnitManifest, UnitRegistry,
    };
    use chrono::{TimeZone, Utc};
    use tempfile::TempDir;

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

    fn storage_api_with_registry(
        allow: &[&str],
        deny: &[&str],
        dry_run: bool,
    ) -> (TempDir, HostApi) {
        let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        let path = dir.path().join("host-api.sqlite3");
        let storage = Storage::new(path)
            .init()
            .unwrap_or_else(|error| panic!("storage init failed: {error}"));

        let mut manifest = UnitManifest::new(
            UnitDefinition::new("moderation.test"),
            TriggerSpec::command(["warn"]),
            ServiceSpec::new("cargo run"),
        );
        manifest.capabilities = CapabilitiesSpec {
            allow: allow.iter().map(|value| (*value).to_owned()).collect(),
            deny: deny.iter().map(|value| (*value).to_owned()).collect(),
        };
        let registry = UnitRegistry::load_manifests(vec![manifest]).registry;

        let api = HostApi::new(dry_run)
            .with_storage(storage)
            .with_unit_registry(registry);
        (dir, api)
    }

    #[test]
    fn ml_embed_text_dry_run_returns_planned_contract_value() {
        let event = manual_event();
        let (_dir, api) = storage_api_with_registry(&["ml.embed_text"], &[], true);

        let response = api
            .ml_embed_text(
                &event,
                MlEmbedTextRequest {
                    base_url: Some("http://localhost:11434".to_owned()),
                    input: vec!["hello".to_owned(), "world".to_owned()],
                    model: Some("sentence-transformers/all-MiniLM-L6-v2".to_owned()),
                },
            )
            .expect("dry-run ml embed succeeds");

        assert_eq!(response.operation, HostApiOperation::MlEmbedText);
        assert!(response.dry_run);
        assert_eq!(response.value.input_count, 2);
        assert!(!response.value.transport_ready);
    }

    #[test]
    fn ml_chat_completion_denies_without_capability() {
        let event = manual_event();
        let (_dir, api) = storage_api_with_registry(&["ml.embed_text"], &[], false);

        let error = api
            .ml_chat_completions(
                &event,
                MlChatCompletionsRequest {
                    base_url: None,
                    model: "meta-llama/llama-3.1-70b-instruct".to_owned(),
                    messages: vec![MlChatMessage {
                        role: "user".to_owned(),
                        content: "Hi".to_owned(),
                    }],
                    max_tokens: Some(32),
                },
            )
            .expect_err("missing capability must fail");

        assert_eq!(error.kind, super::super::HostApiErrorKind::Denied);
        assert_eq!(
            error.detail,
            HostApiErrorDetail::CapabilityDenied {
                capability: "ml.chat".to_owned(),
                unit_id: "moderation.test".to_owned(),
            }
        );
    }

    #[test]
    fn ml_health_returns_structured_unavailable_error_when_transport_is_not_wired() {
        let event = manual_event();
        let (_dir, api) = storage_api_with_registry(&["ml.health.read"], &[], false);

        let error = api
            .ml_health(
                &event,
                MlHealthRequest {
                    base_url: Some("http://localhost:11434".to_owned()),
                },
            )
            .expect_err("unwired ml transport must fail");

        assert_eq!(error.kind, super::super::HostApiErrorKind::Internal);
        assert_eq!(error.operation, HostApiOperation::MlHealth);
        assert_eq!(
            error.detail,
            HostApiErrorDetail::ResourceUnavailable {
                resource: "ml_server_transport".to_owned(),
            }
        );
    }

    #[test]
    fn ml_models_request_routes_through_generic_host_api_call() {
        let event = manual_event();
        let (_dir, api) = storage_api_with_registry(&["ml.models.read"], &[], true);

        let response = api
            .call(
                &event,
                HostApiRequest::MlModels(MlModelsRequest { base_url: None }),
            )
            .expect("generic call succeeds");

        assert_eq!(response.operation, HostApiOperation::MlModels);
        assert!(response.dry_run);
        match response.value {
            HostApiValue::MlModels(value) => assert!(!value.transport_ready),
            other => panic!("unexpected host api value: {other:?}"),
        }
    }

    #[test]
    fn invalid_event_maps_to_validation_error_for_ml_health() {
        let mut event = EventContext::new(
            "evt_invalid",
            UpdateType::Message,
            ExecutionMode::Realtime,
            SystemContext::synthetic(SystemOrigin::Manual),
        );
        event.message = None;

        let api = HostApi::new(false);
        let error = api
            .ml_health(&event, MlHealthRequest { base_url: None })
            .expect_err("invalid event must fail");

        assert_eq!(error.kind, super::super::HostApiErrorKind::Validation);
        assert_eq!(error.operation, HostApiOperation::MlHealth);
    }
}
