use std::rc::Rc;

use crate::event::EventContext;
use crate::parser::command::ReasonExpr;
use crate::parser::duration::{DurationParseError, DurationParser, ParsedDuration};
use crate::parser::reason::{ExpandedReason, ReasonAliasRegistry};
use crate::parser::target::{
    ParsedTargetSelector, ResolvedTarget, TargetParseError, TargetSelectorParser, resolve_target,
};
use crate::storage::{
    AuditLogEntry, AuditLogFilter, JobRecord, KvEntry, MessageJournalRecord, StorageConnection,
    StorageError, UserPatch, UserRecord,
};
use crate::unit::{UnitDiagnostic, UnitRegistry, UnitRegistryStatus, UnitStatus};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;
use uuid::Uuid;

const MAX_MSG_WINDOW: usize = 200;
const MAX_MSG_BY_USER_LIMIT: usize = 200;
const MAX_AUDIT_FIND_LIMIT: usize = 200;
const MAX_JOB_DELAY_DAYS: i64 = 365;

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

    pub fn db_user_get(
        &self,
        event: &EventContext,
        request: DbUserGetRequest,
    ) -> Result<HostApiResponse<DbUserGetValue>, HostApiError> {
        validate_event(event, HostApiOperation::DbUserGet)?;
        validate_user_id(request.user_id, HostApiOperation::DbUserGet)?;

        let user = self
            .storage(HostApiOperation::DbUserGet)?
            .get_user(request.user_id)
            .map_err(|source| storage_error(HostApiOperation::DbUserGet, source))?;

        Ok(self.response(HostApiOperation::DbUserGet, DbUserGetValue { user }))
    }

    pub fn db_user_patch(
        &self,
        event: &EventContext,
        request: DbUserPatchRequest,
    ) -> Result<HostApiResponse<DbUserPatchValue>, HostApiError> {
        validate_event(event, HostApiOperation::DbUserPatch)?;
        validate_user_patch(&request.patch, HostApiOperation::DbUserPatch)?;

        let storage = self.storage(HostApiOperation::DbUserPatch)?;
        let current = storage
            .get_user(request.patch.user_id)
            .map_err(|source| storage_error(HostApiOperation::DbUserPatch, source))?;
        let predicted = apply_user_patch(current.as_ref(), &request.patch);

        if !self.dry_run {
            storage
                .upsert_user(&request.patch)
                .map_err(|source| storage_error(HostApiOperation::DbUserPatch, source))?;
        }

        Ok(self.response(
            HostApiOperation::DbUserPatch,
            DbUserPatchValue { user: predicted },
        ))
    }

    pub fn db_user_incr(
        &self,
        event: &EventContext,
        request: DbUserIncrRequest,
    ) -> Result<HostApiResponse<DbUserIncrValue>, HostApiError> {
        validate_event(event, HostApiOperation::DbUserIncr)?;
        validate_user_incr_request(&request, HostApiOperation::DbUserIncr)?;

        let storage = self.storage(HostApiOperation::DbUserIncr)?;
        let current = storage
            .get_user(request.user_id)
            .map_err(|source| storage_error(HostApiOperation::DbUserIncr, source))?;
        let patch =
            user_patch_from_increment(current.as_ref(), &request, HostApiOperation::DbUserIncr)?;
        let predicted = apply_user_patch(current.as_ref(), &patch);

        if !self.dry_run {
            storage
                .upsert_user(&patch)
                .map_err(|source| storage_error(HostApiOperation::DbUserIncr, source))?;
        }

        Ok(self.response(
            HostApiOperation::DbUserIncr,
            DbUserIncrValue { user: predicted },
        ))
    }

    pub fn db_kv_get(
        &self,
        event: &EventContext,
        request: DbKvGetRequest,
    ) -> Result<HostApiResponse<DbKvGetValue>, HostApiError> {
        validate_event(event, HostApiOperation::DbKvGet)?;
        validate_kv_key(
            &request.scope_kind,
            &request.scope_id,
            &request.key,
            HostApiOperation::DbKvGet,
        )?;

        let entry = self
            .storage(HostApiOperation::DbKvGet)?
            .get_kv(&request.scope_kind, &request.scope_id, &request.key)
            .map_err(|source| storage_error(HostApiOperation::DbKvGet, source))?;

        Ok(self.response(HostApiOperation::DbKvGet, DbKvGetValue { entry }))
    }

    pub fn db_kv_set(
        &self,
        event: &EventContext,
        request: DbKvSetRequest,
    ) -> Result<HostApiResponse<DbKvSetValue>, HostApiError> {
        validate_event(event, HostApiOperation::DbKvSet)?;
        validate_kv_entry(&request.entry, HostApiOperation::DbKvSet)?;

        if !self.dry_run {
            self.storage(HostApiOperation::DbKvSet)?
                .set_kv(&request.entry)
                .map_err(|source| storage_error(HostApiOperation::DbKvSet, source))?;
        }

        Ok(self.response(
            HostApiOperation::DbKvSet,
            DbKvSetValue {
                entry: request.entry,
            },
        ))
    }

    pub fn unit_status(
        &self,
        event: &EventContext,
        request: UnitStatusRequest,
    ) -> Result<HostApiResponse<UnitStatusValue>, HostApiError> {
        validate_event(event, HostApiOperation::UnitStatus)?;
        if let Some(unit_id) = request.unit_id.as_deref() {
            validate_non_empty(unit_id, "unit_id", HostApiOperation::UnitStatus)?;
        }

        let registry = self.unit_registry(HostApiOperation::UnitStatus)?;
        let summary = registry.status_summary();
        let unit = if let Some(unit_id) = request.unit_id.clone() {
            let descriptor = registry.get(&unit_id).ok_or_else(|| {
                HostApiError::validation(
                    HostApiOperation::UnitStatus,
                    HostApiErrorDetail::UnknownUnit {
                        unit_id: unit_id.clone(),
                    },
                )
            })?;
            Some(UnitStatusEntry::from_descriptor(descriptor))
        } else {
            None
        };

        Ok(self.response(
            HostApiOperation::UnitStatus,
            UnitStatusValue {
                requested_unit_id: request.unit_id,
                summary,
                unit,
            },
        ))
    }

    pub fn msg_window(
        &self,
        event: &EventContext,
        request: MsgWindowRequest,
    ) -> Result<HostApiResponse<MsgWindowValue>, HostApiError> {
        validate_event(event, HostApiOperation::MsgWindow)?;
        self.require_operation_capability(event, HostApiOperation::MsgWindow)?;
        validate_msg_window_request(&request, HostApiOperation::MsgWindow)?;

        let messages = self
            .storage(HostApiOperation::MsgWindow)?
            .message_window(
                request.chat_id,
                request.anchor_message_id,
                request.up,
                request.down,
                request.include_anchor,
            )
            .map_err(|source| storage_error(HostApiOperation::MsgWindow, source))?;

        Ok(self.response(HostApiOperation::MsgWindow, MsgWindowValue { messages }))
    }

    pub fn msg_by_user(
        &self,
        event: &EventContext,
        request: MsgByUserRequest,
    ) -> Result<HostApiResponse<MsgByUserValue>, HostApiError> {
        validate_event(event, HostApiOperation::MsgByUser)?;
        self.require_operation_capability(event, HostApiOperation::MsgByUser)?;
        validate_msg_by_user_request(&request, HostApiOperation::MsgByUser)?;

        let messages = self
            .storage(HostApiOperation::MsgByUser)?
            .messages_by_user(
                request.chat_id,
                request.user_id,
                &request.since,
                request.limit,
            )
            .map_err(|source| storage_error(HostApiOperation::MsgByUser, source))?;

        Ok(self.response(HostApiOperation::MsgByUser, MsgByUserValue { messages }))
    }

    pub fn job_schedule_after(
        &self,
        event: &EventContext,
        request: JobScheduleAfterRequest,
    ) -> Result<HostApiResponse<JobScheduleAfterValue>, HostApiError> {
        validate_event(event, HostApiOperation::JobScheduleAfter)?;
        self.require_operation_capability(event, HostApiOperation::JobScheduleAfter)?;
        validate_job_schedule_request(&request, HostApiOperation::JobScheduleAfter)?;

        let parsed_delay = self
            .duration_parser
            .parse(request.delay.trim())
            .map_err(|source| {
                HostApiError::parse(
                    HostApiOperation::JobScheduleAfter,
                    HostApiErrorDetail::InvalidDuration {
                        value: request.delay.clone(),
                        source,
                    },
                )
            })?;
        let delay = duration_to_chrono(parsed_delay, HostApiOperation::JobScheduleAfter)?;
        if delay > ChronoDuration::days(MAX_JOB_DELAY_DAYS) {
            return Err(HostApiError::validation(
                HostApiOperation::JobScheduleAfter,
                HostApiErrorDetail::JobTooFarInFuture {
                    delay: request.delay,
                    max_days: MAX_JOB_DELAY_DAYS,
                },
            ));
        }

        let scheduled_at = event.received_at;
        let run_at = scheduled_at + delay;
        let job = JobRecord {
            job_id: format!("job_{}", Uuid::new_v4().simple()),
            executor_unit: request.executor_unit,
            run_at: to_rfc3339(run_at),
            scheduled_at: to_rfc3339(scheduled_at),
            status: "scheduled".to_owned(),
            dedupe_key: request.dedupe_key,
            payload_json: request.payload.to_string(),
            retry_count: 0,
            max_retries: request.max_retries.unwrap_or(0),
            last_error_code: None,
            last_error_text: None,
            audit_action_id: request.audit_action_id,
            created_at: to_rfc3339(scheduled_at),
            updated_at: to_rfc3339(scheduled_at),
        };

        if !self.dry_run {
            self.storage(HostApiOperation::JobScheduleAfter)?
                .insert_job(&job)
                .map_err(|source| storage_error(HostApiOperation::JobScheduleAfter, source))?;
        }

        Ok(self.response(
            HostApiOperation::JobScheduleAfter,
            JobScheduleAfterValue { job },
        ))
    }

    pub fn audit_find(
        &self,
        event: &EventContext,
        request: AuditFindRequest,
    ) -> Result<HostApiResponse<AuditFindValue>, HostApiError> {
        validate_event(event, HostApiOperation::AuditFind)?;
        self.require_operation_capability(event, HostApiOperation::AuditFind)?;
        validate_audit_find_request(&request, HostApiOperation::AuditFind)?;

        let entries = self
            .storage(HostApiOperation::AuditFind)?
            .find_audit_entries(&request.filters, request.limit)
            .map_err(|source| storage_error(HostApiOperation::AuditFind, source))?;

        Ok(self.response(HostApiOperation::AuditFind, AuditFindValue { entries }))
    }

    pub fn audit_compensate(
        &self,
        event: &EventContext,
        request: AuditCompensateRequest,
    ) -> Result<HostApiResponse<AuditCompensateValue>, HostApiError> {
        validate_event(event, HostApiOperation::AuditCompensate)?;
        self.require_operation_capability(event, HostApiOperation::AuditCompensate)?;
        validate_non_empty(
            &request.action_id,
            "action_id",
            HostApiOperation::AuditCompensate,
        )?;

        let storage = self.storage(HostApiOperation::AuditCompensate)?;
        let original = storage
            .get_audit_entry(&request.action_id)
            .map_err(|source| storage_error(HostApiOperation::AuditCompensate, source))?
            .ok_or_else(|| {
                HostApiError::validation(
                    HostApiOperation::AuditCompensate,
                    HostApiErrorDetail::UnknownAuditAction {
                        action_id: request.action_id.clone(),
                    },
                )
            })?;

        if !original.reversible {
            return Err(HostApiError::validation(
                HostApiOperation::AuditCompensate,
                HostApiErrorDetail::InvalidField {
                    field: "action_id".to_owned(),
                    message: format!("audit action `{}` is not reversible", request.action_id),
                },
            ));
        }
        let compensation_recipe = original.compensation_json.as_deref().ok_or_else(|| {
            HostApiError::validation(
                HostApiOperation::AuditCompensate,
                HostApiErrorDetail::InvalidField {
                    field: "action_id".to_owned(),
                    message: format!(
                        "audit action `{}` has no compensation recipe",
                        request.action_id
                    ),
                },
            )
        })?;
        serde_json::from_str::<Value>(compensation_recipe).map_err(|source| {
            HostApiError::validation(
                HostApiOperation::AuditCompensate,
                HostApiErrorDetail::InvalidField {
                    field: "compensation_json".to_owned(),
                    message: format!("invalid compensation recipe: {source}"),
                },
            )
        })?;

        let compensation_idempotency_key = format!("compensate:{}", original.action_id);
        let existing_compensations = storage
            .find_audit_by_idempotency_key(&compensation_idempotency_key)
            .map_err(|source| storage_error(HostApiOperation::AuditCompensate, source))?;
        if !existing_compensations.is_empty() {
            return Err(HostApiError::validation(
                HostApiOperation::AuditCompensate,
                HostApiErrorDetail::InvalidField {
                    field: "action_id".to_owned(),
                    message: format!(
                        "audit action `{}` is already compensated",
                        request.action_id
                    ),
                },
            ));
        }

        let new_action_id = format!("act_{}", Uuid::new_v4().simple());
        let compensation_entry = AuditLogEntry {
            action_id: new_action_id.clone(),
            trace_id: event
                .system
                .trace_id
                .clone()
                .or_else(|| original.trace_id.clone()),
            request_id: None,
            unit_name: event
                .system
                .unit
                .as_ref()
                .map(|unit| unit.id.clone())
                .unwrap_or_else(|| original.unit_name.clone()),
            execution_mode: execution_mode_label(event),
            op: "audit.compensate".to_owned(),
            actor_user_id: event
                .sender
                .as_ref()
                .map(|sender| sender.id)
                .or(original.actor_user_id),
            chat_id: event.chat.as_ref().map(|chat| chat.id).or(original.chat_id),
            target_kind: Some("audit_action".to_owned()),
            target_id: Some(original.action_id.clone()),
            trigger_message_id: event.message.as_ref().map(|message| i64::from(message.id)),
            idempotency_key: Some(compensation_idempotency_key),
            reversible: false,
            compensation_json: None,
            args_json: json!({
                "action_id": original.action_id,
                "recipe": compensation_recipe,
            })
            .to_string(),
            result_json: Some(json!({ "compensated": true }).to_string()),
            created_at: to_rfc3339(event.received_at),
        };

        if !self.dry_run {
            storage
                .append_audit_entry(&compensation_entry)
                .map_err(|source| storage_error(HostApiOperation::AuditCompensate, source))?;
        }

        Ok(self.response(
            HostApiOperation::AuditCompensate,
            AuditCompensateValue {
                compensated: true,
                new_action_id: Some(new_action_id),
            },
        ))
    }

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
        if self.dry_run {
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
        validate_optional_base_url(
            request.base_url.as_deref(),
            HostApiOperation::MlEmbedText,
        )?;
        validate_ml_embed_request(&request)?;

        let value = MlEmbedTextValue {
            base_url: request.base_url,
            model: request.model,
            input_count: request.input.len(),
            transport_ready: false,
        };
        if self.dry_run {
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
        if self.dry_run {
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
        if self.dry_run {
            return Ok(self.response(HostApiOperation::MlModels, value));
        }

        Err(ml_runtime_unavailable(HostApiOperation::MlModels))
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

fn required_capability(operation: HostApiOperation) -> Option<&'static str> {
    match operation {
        HostApiOperation::MsgWindow | HostApiOperation::MsgByUser => Some("msg.history.read"),
        HostApiOperation::JobScheduleAfter => Some("job.schedule"),
        HostApiOperation::AuditFind => Some("audit.read"),
        HostApiOperation::AuditCompensate => Some("audit.compensate"),
        HostApiOperation::MlHealth => Some("ml.health.read"),
        HostApiOperation::MlEmbedText => Some("ml.embed_text"),
        HostApiOperation::MlChatCompletions => Some("ml.chat"),
        HostApiOperation::MlModels => Some("ml.models.read"),
        HostApiOperation::CtxCurrent
        | HostApiOperation::CtxResolveTarget
        | HostApiOperation::CtxParseDuration
        | HostApiOperation::CtxExpandReason
        | HostApiOperation::DbUserGet
        | HostApiOperation::DbUserPatch
        | HostApiOperation::DbUserIncr
        | HostApiOperation::DbKvGet
        | HostApiOperation::DbKvSet
        | HostApiOperation::UnitStatus => None,
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
    validate_non_empty(
        &request.model,
        "model",
        HostApiOperation::MlChatCompletions,
    )?;
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

fn validate_user_id(user_id: i64, operation: HostApiOperation) -> Result<(), HostApiError> {
    if user_id == 0 {
        return Err(HostApiError::validation(
            operation,
            HostApiErrorDetail::InvalidField {
                field: "user_id".to_owned(),
                message: "must be non-zero".to_owned(),
            },
        ));
    }

    Ok(())
}

fn validate_non_empty(
    value: &str,
    field: &'static str,
    operation: HostApiOperation,
) -> Result<(), HostApiError> {
    if value.trim().is_empty() {
        return Err(HostApiError::validation(
            operation,
            HostApiErrorDetail::InvalidField {
                field: field.to_owned(),
                message: "must not be blank".to_owned(),
            },
        ));
    }

    Ok(())
}

fn validate_user_patch(patch: &UserPatch, operation: HostApiOperation) -> Result<(), HostApiError> {
    validate_user_id(patch.user_id, operation)?;
    validate_non_empty(&patch.seen_at, "seen_at", operation)?;
    validate_non_empty(&patch.updated_at, "updated_at", operation)?;
    if let Some(warn_count) = patch.warn_count
        && warn_count < 0
    {
        return Err(HostApiError::validation(
            operation,
            HostApiErrorDetail::InvalidField {
                field: "warn_count".to_owned(),
                message: "must be non-negative".to_owned(),
            },
        ));
    }

    Ok(())
}

fn validate_user_incr_request(
    request: &DbUserIncrRequest,
    operation: HostApiOperation,
) -> Result<(), HostApiError> {
    validate_user_id(request.user_id, operation)?;
    validate_non_empty(&request.seen_at, "seen_at", operation)?;
    validate_non_empty(&request.updated_at, "updated_at", operation)?;
    Ok(())
}

fn validate_kv_key(
    scope_kind: &str,
    scope_id: &str,
    key: &str,
    operation: HostApiOperation,
) -> Result<(), HostApiError> {
    validate_non_empty(scope_kind, "scope_kind", operation)?;
    validate_non_empty(scope_id, "scope_id", operation)?;
    validate_non_empty(key, "key", operation)?;
    Ok(())
}

fn validate_kv_entry(entry: &KvEntry, operation: HostApiOperation) -> Result<(), HostApiError> {
    validate_kv_key(&entry.scope_kind, &entry.scope_id, &entry.key, operation)?;
    validate_non_empty(&entry.value_json, "value_json", operation)?;
    validate_non_empty(&entry.updated_at, "updated_at", operation)?;
    Ok(())
}

fn validate_msg_window_request(
    request: &MsgWindowRequest,
    operation: HostApiOperation,
) -> Result<(), HostApiError> {
    if request.chat_id == 0 {
        return Err(HostApiError::validation(
            operation,
            HostApiErrorDetail::InvalidField {
                field: "chat_id".to_owned(),
                message: "must be non-zero".to_owned(),
            },
        ));
    }
    if request.anchor_message_id <= 0 {
        return Err(HostApiError::validation(
            operation,
            HostApiErrorDetail::InvalidField {
                field: "anchor_message_id".to_owned(),
                message: "must be positive".to_owned(),
            },
        ));
    }
    let total = request.up + request.down + usize::from(request.include_anchor);
    if total > MAX_MSG_WINDOW {
        return Err(HostApiError::validation(
            operation,
            HostApiErrorDetail::MessageWindowTooLarge {
                requested: total,
                max: MAX_MSG_WINDOW,
            },
        ));
    }

    Ok(())
}

fn validate_msg_by_user_request(
    request: &MsgByUserRequest,
    operation: HostApiOperation,
) -> Result<(), HostApiError> {
    if request.chat_id == 0 {
        return Err(HostApiError::validation(
            operation,
            HostApiErrorDetail::InvalidField {
                field: "chat_id".to_owned(),
                message: "must be non-zero".to_owned(),
            },
        ));
    }
    validate_user_id(request.user_id, operation)?;
    validate_non_empty(&request.since, "since", operation)?;
    parse_rfc3339(&request.since, operation, "since")?;
    if request.limit == 0 || request.limit > MAX_MSG_BY_USER_LIMIT {
        return Err(HostApiError::validation(
            operation,
            HostApiErrorDetail::InvalidField {
                field: "limit".to_owned(),
                message: format!("must be between 1 and {MAX_MSG_BY_USER_LIMIT}"),
            },
        ));
    }

    Ok(())
}

fn validate_job_schedule_request(
    request: &JobScheduleAfterRequest,
    operation: HostApiOperation,
) -> Result<(), HostApiError> {
    validate_non_empty(&request.delay, "delay", operation)?;
    validate_non_empty(&request.executor_unit, "executor_unit", operation)?;
    if let Some(dedupe_key) = request.dedupe_key.as_deref() {
        validate_non_empty(dedupe_key, "dedupe_key", operation)?;
    }
    if let Some(audit_action_id) = request.audit_action_id.as_deref() {
        validate_non_empty(audit_action_id, "audit_action_id", operation)?;
    }
    if let Some(max_retries) = request.max_retries
        && max_retries < 0
    {
        return Err(HostApiError::validation(
            operation,
            HostApiErrorDetail::InvalidField {
                field: "max_retries".to_owned(),
                message: "must be non-negative".to_owned(),
            },
        ));
    }

    Ok(())
}

fn validate_audit_find_request(
    request: &AuditFindRequest,
    operation: HostApiOperation,
) -> Result<(), HostApiError> {
    if request.limit == 0 || request.limit > MAX_AUDIT_FIND_LIMIT {
        return Err(HostApiError::validation(
            operation,
            HostApiErrorDetail::InvalidField {
                field: "limit".to_owned(),
                message: format!("must be between 1 and {MAX_AUDIT_FIND_LIMIT}"),
            },
        ));
    }
    if request.filters.action_id.is_none()
        && request.filters.trace_id.is_none()
        && request.filters.request_id.is_none()
        && request.filters.idempotency_key.is_none()
        && request.filters.trigger_message_id.is_none()
        && request.filters.actor_user_id.is_none()
        && request.filters.chat_id.is_none()
        && request.filters.op.is_none()
        && request.filters.target_id.is_none()
        && request.filters.reversible.is_none()
    {
        return Err(HostApiError::validation(
            operation,
            HostApiErrorDetail::MissingAuditFilter,
        ));
    }

    validate_optional_non_empty(&request.filters.action_id, "filters.action_id", operation)?;
    validate_optional_non_empty(&request.filters.trace_id, "filters.trace_id", operation)?;
    validate_optional_non_empty(&request.filters.request_id, "filters.request_id", operation)?;
    validate_optional_non_empty(
        &request.filters.idempotency_key,
        "filters.idempotency_key",
        operation,
    )?;
    validate_optional_non_empty(&request.filters.op, "filters.op", operation)?;
    validate_optional_non_empty(&request.filters.target_id, "filters.target_id", operation)?;

    Ok(())
}

fn validate_optional_non_empty(
    value: &Option<String>,
    field: &'static str,
    operation: HostApiOperation,
) -> Result<(), HostApiError> {
    if let Some(value) = value.as_deref() {
        validate_non_empty(value, field, operation)?;
    }

    Ok(())
}

fn storage_error(operation: HostApiOperation, source: StorageError) -> HostApiError {
    HostApiError::internal(
        operation,
        HostApiErrorDetail::StorageFailure {
            message: source.to_string(),
        },
    )
}

fn duration_to_chrono(
    parsed: ParsedDuration,
    operation: HostApiOperation,
) -> Result<ChronoDuration, HostApiError> {
    ChronoDuration::from_std(parsed.into_std()).map_err(|error| {
        HostApiError::internal(
            operation,
            HostApiErrorDetail::InternalConversionFailure {
                message: error.to_string(),
            },
        )
    })
}

fn to_rfc3339(value: DateTime<Utc>) -> String {
    value.to_rfc3339()
}

fn parse_rfc3339(
    value: &str,
    operation: HostApiOperation,
    field: &'static str,
) -> Result<DateTime<Utc>, HostApiError> {
    DateTime::parse_from_rfc3339(value)
        .map(|parsed| parsed.with_timezone(&Utc))
        .map_err(|error| {
            HostApiError::validation(
                operation,
                HostApiErrorDetail::InvalidField {
                    field: field.to_owned(),
                    message: format!("must be RFC3339 timestamp: {error}"),
                },
            )
        })
}

fn execution_mode_label(event: &EventContext) -> String {
    match event.execution_mode {
        crate::event::ExecutionMode::Realtime => "realtime",
        crate::event::ExecutionMode::Recovery => "recovery",
        crate::event::ExecutionMode::Scheduled => "scheduled",
        crate::event::ExecutionMode::Manual => "manual",
    }
    .to_owned()
}

fn apply_user_patch(current: Option<&UserRecord>, patch: &UserPatch) -> UserRecord {
    let first_seen_at = match current {
        Some(existing) if existing.first_seen_at < patch.seen_at => existing.first_seen_at.clone(),
        _ => patch.seen_at.clone(),
    };
    let last_seen_at = match current {
        Some(existing) if existing.last_seen_at > patch.seen_at => existing.last_seen_at.clone(),
        _ => patch.seen_at.clone(),
    };

    UserRecord {
        user_id: patch.user_id,
        username: patch
            .username
            .clone()
            .or_else(|| current.and_then(|existing| existing.username.clone())),
        display_name: patch
            .display_name
            .clone()
            .or_else(|| current.and_then(|existing| existing.display_name.clone())),
        first_seen_at,
        last_seen_at,
        warn_count: patch
            .warn_count
            .unwrap_or_else(|| current.map_or(0, |existing| existing.warn_count)),
        shadowbanned: patch
            .shadowbanned
            .unwrap_or_else(|| current.is_some_and(|existing| existing.shadowbanned)),
        reputation: patch
            .reputation
            .unwrap_or_else(|| current.map_or(0, |existing| existing.reputation)),
        state_json: patch
            .state_json
            .clone()
            .or_else(|| current.and_then(|existing| existing.state_json.clone())),
        updated_at: patch.updated_at.clone(),
    }
}

fn user_patch_from_increment(
    current: Option<&UserRecord>,
    request: &DbUserIncrRequest,
    operation: HostApiOperation,
) -> Result<UserPatch, HostApiError> {
    let current_warn_count = current.map_or(0, |user| user.warn_count);
    let warn_count = current_warn_count
        .checked_add(request.warn_count_delta)
        .ok_or_else(|| {
            counter_error(
                operation,
                "warn_count",
                current_warn_count,
                request.warn_count_delta,
            )
        })?;
    if warn_count < 0 {
        return Err(counter_error(
            operation,
            "warn_count",
            current_warn_count,
            request.warn_count_delta,
        ));
    }

    let current_reputation = current.map_or(0, |user| user.reputation);
    let reputation = current_reputation
        .checked_add(request.reputation_delta)
        .ok_or_else(|| {
            counter_error(
                operation,
                "reputation",
                current_reputation,
                request.reputation_delta,
            )
        })?;

    Ok(UserPatch {
        user_id: request.user_id,
        username: request.username.clone(),
        display_name: request.display_name.clone(),
        seen_at: request.seen_at.clone(),
        warn_count: Some(warn_count),
        shadowbanned: request.shadowbanned,
        reputation: Some(reputation),
        state_json: request.state_json.clone(),
        updated_at: request.updated_at.clone(),
    })
}

fn counter_error(
    operation: HostApiOperation,
    field: &'static str,
    current: i64,
    delta: i64,
) -> HostApiError {
    HostApiError::validation(
        operation,
        HostApiErrorDetail::InvalidCounterChange {
            field: field.to_owned(),
            current,
            delta,
        },
    )
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub enum HostApiRequest {
    CtxCurrent,
    CtxResolveTarget(CtxResolveTargetRequest),
    CtxParseDuration(CtxParseDurationRequest),
    CtxExpandReason(CtxExpandReasonRequest),
    DbUserGet(DbUserGetRequest),
    DbUserPatch(DbUserPatchRequest),
    DbUserIncr(DbUserIncrRequest),
    DbKvGet(DbKvGetRequest),
    DbKvSet(DbKvSetRequest),
    MsgWindow(MsgWindowRequest),
    MsgByUser(MsgByUserRequest),
    JobScheduleAfter(JobScheduleAfterRequest),
    AuditFind(AuditFindRequest),
    AuditCompensate(AuditCompensateRequest),
    UnitStatus(UnitStatusRequest),
    MlHealth(MlHealthRequest),
    MlEmbedText(MlEmbedTextRequest),
    MlChatCompletions(MlChatCompletionsRequest),
    MlModels(MlModelsRequest),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HostApiValue {
    CtxCurrent(Box<CtxCurrentValue>),
    ResolvedTarget(ResolvedTarget),
    ParsedDuration(ParsedDuration),
    ExpandedReason(ExpandedReason),
    DbUserGet(DbUserGetValue),
    DbUserPatch(DbUserPatchValue),
    DbUserIncr(DbUserIncrValue),
    DbKvGet(DbKvGetValue),
    DbKvSet(DbKvSetValue),
    MsgWindow(MsgWindowValue),
    MsgByUser(MsgByUserValue),
    JobScheduleAfter(JobScheduleAfterValue),
    AuditFind(AuditFindValue),
    AuditCompensate(AuditCompensateValue),
    UnitStatus(UnitStatusValue),
    MlHealth(MlHealthValue),
    MlEmbedText(MlEmbedTextValue),
    MlChatCompletions(MlChatCompletionsValue),
    MlModels(MlModelsValue),
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostApiOperation {
    CtxCurrent,
    CtxResolveTarget,
    CtxParseDuration,
    CtxExpandReason,
    DbUserGet,
    DbUserPatch,
    DbUserIncr,
    DbKvGet,
    DbKvSet,
    MsgWindow,
    MsgByUser,
    JobScheduleAfter,
    AuditFind,
    AuditCompensate,
    UnitStatus,
    MlHealth,
    MlEmbedText,
    MlChatCompletions,
    MlModels,
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

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct DbUserGetRequest {
    pub user_id: i64,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct DbUserPatchRequest {
    pub patch: UserPatch,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct DbUserIncrRequest {
    pub user_id: i64,
    pub username: Option<String>,
    pub display_name: Option<String>,
    pub seen_at: String,
    pub updated_at: String,
    pub warn_count_delta: i64,
    pub reputation_delta: i64,
    pub shadowbanned: Option<bool>,
    pub state_json: Option<String>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct DbKvGetRequest {
    pub scope_kind: String,
    pub scope_id: String,
    pub key: String,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct DbKvSetRequest {
    pub entry: KvEntry,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct MsgWindowRequest {
    pub chat_id: i64,
    pub anchor_message_id: i64,
    pub up: usize,
    pub down: usize,
    pub include_anchor: bool,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct MsgByUserRequest {
    pub chat_id: i64,
    pub user_id: i64,
    pub since: String,
    pub limit: usize,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct JobScheduleAfterRequest {
    pub delay: String,
    pub executor_unit: String,
    pub payload: Value,
    pub dedupe_key: Option<String>,
    pub max_retries: Option<i64>,
    pub audit_action_id: Option<String>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct AuditFindRequest {
    pub filters: AuditLogFilter,
    pub limit: usize,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct AuditCompensateRequest {
    pub action_id: String,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct UnitStatusRequest {
    pub unit_id: Option<String>,
}

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
pub struct DbUserGetValue {
    pub user: Option<UserRecord>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct DbUserPatchValue {
    pub user: UserRecord,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct DbUserIncrValue {
    pub user: UserRecord,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct DbKvGetValue {
    pub entry: Option<KvEntry>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct DbKvSetValue {
    pub entry: KvEntry,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct MsgWindowValue {
    pub messages: Vec<MessageJournalRecord>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct MsgByUserValue {
    pub messages: Vec<MessageJournalRecord>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct JobScheduleAfterValue {
    pub job: JobRecord,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct AuditFindValue {
    pub entries: Vec<AuditLogEntry>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct AuditCompensateValue {
    pub compensated: bool,
    pub new_action_id: Option<String>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct UnitStatusValue {
    pub requested_unit_id: Option<String>,
    pub summary: UnitRegistryStatus,
    pub unit: Option<UnitStatusEntry>,
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

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct UnitStatusEntry {
    pub unit_id: String,
    pub status: UnitStatus,
    pub enabled: Option<bool>,
    pub diagnostics: Vec<UnitDiagnostic>,
}

impl UnitStatusEntry {
    fn from_descriptor(descriptor: &crate::unit::UnitDescriptor) -> Self {
        Self {
            unit_id: descriptor.id.clone(),
            status: descriptor.status,
            enabled: descriptor
                .manifest
                .as_ref()
                .map(|manifest| manifest.unit.enabled),
            diagnostics: descriptor.diagnostics.clone(),
        }
    }
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

    fn denied(operation: HostApiOperation, detail: HostApiErrorDetail) -> Self {
        Self {
            operation,
            kind: HostApiErrorKind::Denied,
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
    #[error("invalid field `{field}`: {message}")]
    InvalidField { field: String, message: String },
    #[error("invalid counter change for `{field}`: current={current}, delta={delta}")]
    InvalidCounterChange {
        field: String,
        current: i64,
        delta: i64,
    },
    #[error("message window too large: requested {requested}, max {max}")]
    MessageWindowTooLarge { requested: usize, max: usize },
    #[error("scheduled job delay `{delay}` exceeds max {max_days} days")]
    JobTooFarInFuture { delay: String, max_days: i64 },
    #[error("audit.find requires at least one filter")]
    MissingAuditFilter,
    #[error("unknown unit `{unit_id}`")]
    UnknownUnit { unit_id: String },
    #[error("operation denied for unit `{unit_id}`: missing capability `{capability}`")]
    CapabilityDenied { capability: String, unit_id: String },
    #[error("unknown audit action `{action_id}`")]
    UnknownAuditAction { action_id: String },
    #[error("required host resource `{resource}` is unavailable")]
    ResourceUnavailable { resource: String },
    #[error("storage failure: {message}")]
    StorageFailure { message: String },
    #[error("internal conversion failed: {message}")]
    InternalConversionFailure { message: String },
    #[error("reason expansion unexpectedly returned no result")]
    ReasonExpansionUnavailable,
}

#[cfg(test)]
mod tests {
    use super::{
        AuditCompensateRequest, AuditFindRequest, CtxExpandReasonRequest, CtxParseDurationRequest,
        CtxResolveTargetRequest, DbKvGetRequest, DbKvSetRequest, DbUserGetRequest,
        DbUserIncrRequest, DbUserPatchRequest, HostApi, HostApiError, HostApiErrorDetail,
        HostApiErrorKind, HostApiOperation, HostApiRequest, HostApiValue, JobScheduleAfterRequest,
        MlChatCompletionsRequest, MlChatMessage, MlEmbedTextRequest, MlHealthRequest,
        MlModelsRequest, MsgByUserRequest, MsgWindowRequest, UnitStatusEntry, UnitStatusRequest,
    };
    use crate::event::{
        ChatContext, EventContext, EventNormalizer, ExecutionMode, ManualInvocationInput,
        ReplyContext, SystemContext, SystemOrigin, UnitContext, UpdateType,
    };
    use crate::parser::command::ReasonExpr;
    use crate::parser::duration::{DurationParseError, DurationUnit, ParsedDuration};
    use crate::parser::reason::{ExpandedReason, ReasonAliasDefinition, ReasonAliasRegistry};
    use crate::parser::target::{ParsedTargetSelector, TargetParseError, TargetSource};
    use crate::storage::{
        AuditLogEntry, AuditLogFilter, KvEntry, MessageJournalRecord, Storage, UserPatch,
    };
    use crate::unit::{
        CapabilitiesSpec, ServiceSpec, TriggerSpec, UnitDefinition, UnitManifest, UnitRegistry,
        UnitStatus,
    };
    use chrono::{TimeZone, Utc};
    use serde_json::json;
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

    fn storage_api() -> (TempDir, HostApi) {
        let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        let path = dir.path().join("host-api.sqlite3");
        let storage = Storage::new(path)
            .init()
            .unwrap_or_else(|error| panic!("storage init failed: {error}"));
        (dir, HostApi::new(false).with_storage(storage))
    }

    fn dry_run_storage_api() -> (TempDir, HostApi) {
        let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        let path = dir.path().join("host-api.sqlite3");
        let storage = Storage::new(path)
            .init()
            .unwrap_or_else(|error| panic!("storage init failed: {error}"));
        (dir, HostApi::new(true).with_storage(storage))
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

    fn unit_registry_api() -> HostApi {
        let active = UnitManifest::new(
            UnitDefinition::new("moderation.warn"),
            TriggerSpec::command(["warn"]),
            ServiceSpec::new("cargo run"),
        );
        let mut disabled = UnitManifest::new(
            UnitDefinition::new("moderation.mute"),
            TriggerSpec::command(["mute"]),
            ServiceSpec::new("cargo run"),
        );
        disabled.unit.enabled = false;

        let report = UnitRegistry::load_manifests(vec![active, disabled]);
        assert!(report.is_fully_valid());

        HostApi::new(false).with_unit_registry(report.registry)
    }

    fn seed_message_journal(api: &HostApi) {
        let storage = api
            .storage(HostApiOperation::MsgWindow)
            .expect("storage available");
        for (message_id, user_id, text, date_utc) in [
            (
                81229_i64,
                Some(99887766_i64),
                Some("spam 1"),
                "2026-04-21T11:59:00Z",
            ),
            (
                81230,
                Some(99887766),
                Some("spam 2"),
                "2026-04-21T11:59:10Z",
            ),
            (
                81231,
                Some(99887766),
                Some("spam 3"),
                "2026-04-21T11:59:20Z",
            ),
            (
                81232,
                Some(99887766),
                Some("spam 4"),
                "2026-04-21T11:59:30Z",
            ),
            (
                81233,
                Some(99887766),
                Some("spam 5"),
                "2026-04-21T11:59:40Z",
            ),
            (81234, Some(42), Some("admin note"), "2026-04-21T12:05:00Z"),
        ] {
            storage
                .append_message_journal(&MessageJournalRecord {
                    chat_id: -100123,
                    message_id,
                    user_id,
                    date_utc: date_utc.to_owned(),
                    update_type: "message".to_owned(),
                    text: text.map(str::to_owned),
                    normalized_text: text.map(str::to_owned),
                    has_media: false,
                    reply_to_message_id: None,
                    file_ids_json: None,
                    meta_json: None,
                })
                .expect("seed message journal");
        }
    }

    fn seed_audit_entries(api: &HostApi) {
        let storage = api
            .storage(HostApiOperation::AuditFind)
            .expect("storage available");
        for entry in [
            AuditLogEntry {
                action_id: "act_1".to_owned(),
                trace_id: Some("trace-1".to_owned()),
                request_id: Some("req-1".to_owned()),
                unit_name: "moderation.test".to_owned(),
                execution_mode: "manual".to_owned(),
                op: "mute".to_owned(),
                actor_user_id: Some(42),
                chat_id: Some(-100123),
                target_kind: Some("user".to_owned()),
                target_id: Some("99887766".to_owned()),
                trigger_message_id: Some(81231),
                idempotency_key: Some("idem-1".to_owned()),
                reversible: true,
                compensation_json: Some(
                    "{\"kind\":\"host_op\",\"op\":\"tg.unrestrict\"}".to_owned(),
                ),
                args_json: "{\"duration\":\"7d\"}".to_owned(),
                result_json: Some("{\"ok\":true}".to_owned()),
                created_at: "2026-04-21T12:00:00Z".to_owned(),
            },
            AuditLogEntry {
                action_id: "act_2".to_owned(),
                trace_id: Some("trace-2".to_owned()),
                request_id: Some("req-2".to_owned()),
                unit_name: "moderation.test".to_owned(),
                execution_mode: "manual".to_owned(),
                op: "del".to_owned(),
                actor_user_id: Some(42),
                chat_id: Some(-100123),
                target_kind: Some("message".to_owned()),
                target_id: Some("81231".to_owned()),
                trigger_message_id: Some(81231),
                idempotency_key: Some("idem-2".to_owned()),
                reversible: false,
                compensation_json: None,
                args_json: "{\"count\":1}".to_owned(),
                result_json: Some("{\"deleted\":1}".to_owned()),
                created_at: "2026-04-21T12:01:00Z".to_owned(),
            },
        ] {
            storage
                .append_audit_entry(&entry)
                .expect("seed audit entry");
        }
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
    fn db_user_get_returns_typed_user_value() {
        let event = manual_event();
        let (_dir, api) = storage_api();
        api.storage(HostApiOperation::DbUserGet)
            .expect("storage")
            .upsert_user(&UserPatch {
                user_id: 77,
                username: Some("reply_user".to_owned()),
                display_name: Some("Reply User".to_owned()),
                seen_at: "2026-04-21T12:00:00Z".to_owned(),
                warn_count: Some(1),
                shadowbanned: Some(false),
                reputation: Some(4),
                state_json: Some("{\"state\":\"ok\"}".to_owned()),
                updated_at: "2026-04-21T12:00:00Z".to_owned(),
            })
            .expect("seed user");

        let response = api
            .db_user_get(&event, DbUserGetRequest { user_id: 77 })
            .expect("db.user_get succeeds");

        assert_eq!(response.operation, HostApiOperation::DbUserGet);
        assert_eq!(
            response
                .value
                .user
                .expect("user exists")
                .username
                .as_deref(),
            Some("reply_user")
        );
    }

    #[test]
    fn db_user_get_rejects_zero_user_id() {
        let event = manual_event();
        let (_dir, api) = storage_api();

        let error = api
            .db_user_get(&event, DbUserGetRequest { user_id: 0 })
            .expect_err("zero user id must fail");

        assert_eq!(error.kind, HostApiErrorKind::Validation);
        assert_eq!(error.operation, HostApiOperation::DbUserGet);
        assert_eq!(
            error.detail,
            HostApiErrorDetail::InvalidField {
                field: "user_id".to_owned(),
                message: "must be non-zero".to_owned(),
            }
        );
    }

    #[test]
    fn db_user_get_requires_storage_resource() {
        let event = manual_event();
        let api = HostApi::new(false);

        let error = api
            .db_user_get(&event, DbUserGetRequest { user_id: 77 })
            .expect_err("missing storage must fail");

        assert_eq!(error.kind, HostApiErrorKind::Internal);
        assert_eq!(error.operation, HostApiOperation::DbUserGet);
        assert_eq!(
            error.detail,
            HostApiErrorDetail::ResourceUnavailable {
                resource: "storage".to_owned(),
            }
        );
    }

    #[test]
    fn db_user_patch_persists_user_on_happy_path() {
        let event = manual_event();
        let (_dir, api) = storage_api();

        let response = api
            .db_user_patch(
                &event,
                DbUserPatchRequest {
                    patch: UserPatch {
                        user_id: 77,
                        username: Some("patched_user".to_owned()),
                        display_name: Some("Patched User".to_owned()),
                        seen_at: "2026-04-21T12:05:00Z".to_owned(),
                        warn_count: Some(2),
                        shadowbanned: Some(false),
                        reputation: Some(9),
                        state_json: Some("{\"state\":\"patched\"}".to_owned()),
                        updated_at: "2026-04-21T12:05:00Z".to_owned(),
                    },
                },
            )
            .expect("patch succeeds");

        assert!(!response.dry_run);
        assert_eq!(
            response.value.user.username.as_deref(),
            Some("patched_user")
        );
        assert_eq!(
            api.storage(HostApiOperation::DbUserPatch)
                .expect("storage")
                .get_user(77)
                .expect("query succeeds")
                .expect("user exists")
                .username
                .as_deref(),
            Some("patched_user")
        );
    }

    #[test]
    fn db_user_patch_dry_run_validates_without_mutation() {
        let event = manual_event();
        let (_dir, api) = dry_run_storage_api();

        let response = api
            .db_user_patch(
                &event,
                DbUserPatchRequest {
                    patch: UserPatch {
                        user_id: 77,
                        username: Some("dry_run_user".to_owned()),
                        display_name: Some("Dry Run".to_owned()),
                        seen_at: "2026-04-21T12:05:00Z".to_owned(),
                        warn_count: Some(2),
                        shadowbanned: Some(true),
                        reputation: Some(5),
                        state_json: Some("{\"mode\":\"dry\"}".to_owned()),
                        updated_at: "2026-04-21T12:05:00Z".to_owned(),
                    },
                },
            )
            .expect("dry-run patch succeeds");

        assert!(response.dry_run);
        assert_eq!(response.value.user.warn_count, 2);
        assert!(
            api.storage(HostApiOperation::DbUserPatch)
                .expect("storage")
                .get_user(77)
                .expect("query succeeds")
                .is_none()
        );
    }

    #[test]
    fn db_user_patch_returns_structured_validation_error() {
        let event = manual_event();
        let (_dir, api) = storage_api();

        let error = api
            .db_user_patch(
                &event,
                DbUserPatchRequest {
                    patch: UserPatch {
                        user_id: 0,
                        username: None,
                        display_name: None,
                        seen_at: "".to_owned(),
                        warn_count: Some(-1),
                        shadowbanned: None,
                        reputation: None,
                        state_json: None,
                        updated_at: "".to_owned(),
                    },
                },
            )
            .expect_err("invalid patch must fail");

        assert_eq!(error.kind, HostApiErrorKind::Validation);
        assert_eq!(error.operation, HostApiOperation::DbUserPatch);
        assert_eq!(
            error.detail,
            HostApiErrorDetail::InvalidField {
                field: "user_id".to_owned(),
                message: "must be non-zero".to_owned(),
            }
        );
    }

    #[test]
    fn db_user_incr_updates_existing_user() {
        let event = manual_event();
        let (_dir, api) = storage_api();
        api.storage(HostApiOperation::DbUserIncr)
            .expect("storage")
            .upsert_user(&UserPatch {
                user_id: 77,
                username: Some("reply_user".to_owned()),
                display_name: Some("Reply User".to_owned()),
                seen_at: "2026-04-21T12:00:00Z".to_owned(),
                warn_count: Some(1),
                shadowbanned: Some(false),
                reputation: Some(4),
                state_json: None,
                updated_at: "2026-04-21T12:00:00Z".to_owned(),
            })
            .expect("seed user");

        let response = api
            .db_user_incr(
                &event,
                DbUserIncrRequest {
                    user_id: 77,
                    username: None,
                    display_name: Some("Reply User Updated".to_owned()),
                    seen_at: "2026-04-21T12:10:00Z".to_owned(),
                    updated_at: "2026-04-21T12:10:00Z".to_owned(),
                    warn_count_delta: 2,
                    reputation_delta: -1,
                    shadowbanned: Some(true),
                    state_json: Some("{\"escalated\":true}".to_owned()),
                },
            )
            .expect("increment succeeds");

        assert_eq!(response.value.user.warn_count, 3);
        assert_eq!(response.value.user.reputation, 3);
        assert!(response.value.user.shadowbanned);
        assert_eq!(
            api.storage(HostApiOperation::DbUserIncr)
                .expect("storage")
                .get_user(77)
                .expect("query succeeds")
                .expect("user exists")
                .warn_count,
            3
        );
    }

    #[test]
    fn db_user_incr_returns_structured_counter_error() {
        let event = manual_event();
        let (_dir, api) = storage_api();

        let error = api
            .db_user_incr(
                &event,
                DbUserIncrRequest {
                    user_id: 77,
                    username: None,
                    display_name: None,
                    seen_at: "2026-04-21T12:10:00Z".to_owned(),
                    updated_at: "2026-04-21T12:10:00Z".to_owned(),
                    warn_count_delta: -1,
                    reputation_delta: 0,
                    shadowbanned: None,
                    state_json: None,
                },
            )
            .expect_err("negative increment from zero must fail");

        assert_eq!(error.kind, HostApiErrorKind::Validation);
        assert_eq!(
            error.detail,
            HostApiErrorDetail::InvalidCounterChange {
                field: "warn_count".to_owned(),
                current: 0,
                delta: -1,
            }
        );
    }

    #[test]
    fn db_user_incr_dry_run_does_not_mutate_storage() {
        let event = manual_event();
        let (_dir, api) = dry_run_storage_api();

        let response = api
            .db_user_incr(
                &event,
                DbUserIncrRequest {
                    user_id: 77,
                    username: Some("dry_increment".to_owned()),
                    display_name: Some("Dry Increment".to_owned()),
                    seen_at: "2026-04-21T12:10:00Z".to_owned(),
                    updated_at: "2026-04-21T12:10:00Z".to_owned(),
                    warn_count_delta: 2,
                    reputation_delta: 4,
                    shadowbanned: Some(false),
                    state_json: Some("{\"dry\":true}".to_owned()),
                },
            )
            .expect("dry-run increment succeeds");

        assert!(response.dry_run);
        assert_eq!(response.value.user.warn_count, 2);
        assert!(
            api.storage(HostApiOperation::DbUserIncr)
                .expect("storage")
                .get_user(77)
                .expect("query succeeds")
                .is_none()
        );
    }

    #[test]
    fn db_kv_set_dry_run_does_not_mutate_storage() {
        let event = manual_event();
        let (_dir, api) = dry_run_storage_api();

        let response = api
            .db_kv_set(
                &event,
                DbKvSetRequest {
                    entry: KvEntry {
                        scope_kind: "chat".to_owned(),
                        scope_id: "-100123".to_owned(),
                        key: "policy".to_owned(),
                        value_json: "{\"mode\":\"strict\"}".to_owned(),
                        updated_at: "2026-04-21T12:00:00Z".to_owned(),
                    },
                },
            )
            .expect("dry-run kv set succeeds");

        assert!(response.dry_run);
        assert_eq!(response.value.entry.key, "policy");
        assert!(
            api.storage(HostApiOperation::DbKvSet)
                .expect("storage")
                .get_kv("chat", "-100123", "policy")
                .expect("query succeeds")
                .is_none()
        );
    }

    #[test]
    fn db_kv_get_returns_seeded_entry() {
        let event = manual_event();
        let (_dir, api) = storage_api();
        api.storage(HostApiOperation::DbKvGet)
            .expect("storage")
            .set_kv(&KvEntry {
                scope_kind: "chat".to_owned(),
                scope_id: "-100123".to_owned(),
                key: "policy".to_owned(),
                value_json: "{\"mode\":\"strict\"}".to_owned(),
                updated_at: "2026-04-21T12:00:00Z".to_owned(),
            })
            .expect("seed kv");

        let response = api
            .db_kv_get(
                &event,
                DbKvGetRequest {
                    scope_kind: "chat".to_owned(),
                    scope_id: "-100123".to_owned(),
                    key: "policy".to_owned(),
                },
            )
            .expect("kv get succeeds");

        assert_eq!(
            response.value.entry.expect("entry exists").value_json,
            "{\"mode\":\"strict\"}"
        );
    }

    #[test]
    fn db_kv_get_rejects_blank_key() {
        let event = manual_event();
        let (_dir, api) = storage_api();

        let error = api
            .db_kv_get(
                &event,
                DbKvGetRequest {
                    scope_kind: "chat".to_owned(),
                    scope_id: "-100123".to_owned(),
                    key: "   ".to_owned(),
                },
            )
            .expect_err("blank key must fail");

        assert_eq!(error.kind, HostApiErrorKind::Validation);
        assert_eq!(error.operation, HostApiOperation::DbKvGet);
        assert_eq!(
            error.detail,
            HostApiErrorDetail::InvalidField {
                field: "key".to_owned(),
                message: "must not be blank".to_owned(),
            }
        );
    }

    #[test]
    fn db_kv_set_persists_entry_on_happy_path() {
        let event = manual_event();
        let (_dir, api) = storage_api();

        let response = api
            .db_kv_set(
                &event,
                DbKvSetRequest {
                    entry: KvEntry {
                        scope_kind: "chat".to_owned(),
                        scope_id: "-100123".to_owned(),
                        key: "policy".to_owned(),
                        value_json: "{\"mode\":\"strict\"}".to_owned(),
                        updated_at: "2026-04-21T12:00:00Z".to_owned(),
                    },
                },
            )
            .expect("kv set succeeds");

        assert!(!response.dry_run);
        assert_eq!(
            api.storage(HostApiOperation::DbKvSet)
                .expect("storage")
                .get_kv("chat", "-100123", "policy")
                .expect("query succeeds")
                .expect("entry exists")
                .value_json,
            "{\"mode\":\"strict\"}"
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
        assert!(
            api.storage(HostApiOperation::JobScheduleAfter)
                .expect("storage")
                .get_job(&response.value.job.job_id)
                .expect("job lookup succeeds")
                .is_none()
        );
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
        assert!(
            api.storage(HostApiOperation::JobScheduleAfter)
                .expect("storage")
                .get_job(&response.value.job.job_id)
                .expect("lookup succeeds")
                .is_some()
        );
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
        let (_dir, api) =
            storage_api_with_registry(&["audit.compensate", "audit.read"], &[], false);
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
        assert!(
            api.storage(HostApiOperation::AuditCompensate)
                .expect("storage")
                .get_audit_entry(&new_action_id)
                .expect("lookup succeeds")
                .is_none()
        );
    }

    #[test]
    fn audit_compensate_rejects_already_compensated_action() {
        let event = manual_event();
        let (_dir, api) =
            storage_api_with_registry(&["audit.compensate", "audit.read"], &[], false);
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
        let (_dir, api) =
            storage_api_with_registry(&["audit.compensate", "audit.read"], &[], false);
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
        let (_dir, api) =
            storage_api_with_registry(&["audit.compensate", "audit.read"], &[], false);
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

        assert_eq!(error.kind, HostApiErrorKind::Denied);
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

        assert_eq!(error.kind, HostApiErrorKind::Internal);
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
}
