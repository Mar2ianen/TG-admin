use std::rc::Rc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;
use uuid::Uuid;

use crate::event::{EventContext, ExecutionMode};
use crate::parser::command::{CommandAst, DeleteCommand, MessageCommand, ParsedCommandLine};
use crate::parser::dispatch::{
    CommandDispatchParseError, CommandDispatchResult, CommandDispatchSkip, EventCommandDispatcher,
};
use crate::parser::reason::{
    ExpandedCommandAst, ExpandedCommandLine, ExpandedModerationCommand, ExpandedMuteCommand,
    ExpandedReason, ReasonAliasRegistry,
};
use crate::parser::target::{ParsedTargetSelector, ResolvedTarget, TargetSource};
use crate::storage::{
    AuditLogEntry, AuditLogFilter, JobRecord, ProcessedUpdateRecord,
    PROCESSED_UPDATE_STATUS_COMPLETED, PROCESSED_UPDATE_STATUS_PENDING, StorageConnection,
    StorageError, UserPatch,
};
use crate::tg::{
    MessageId, TelegramBanRequest, TelegramExecution, TelegramExecutionOptions, TelegramGateway,
    TelegramPermissions, TelegramRequest, TelegramRestrictRequest, TelegramSendMessageRequest,
    TelegramUnbanRequest, TelegramUnrestrictRequest,
};
use crate::unit::UnitRegistry;

#[derive(Debug, Clone)]
pub struct ModerationEngine {
    dry_run: bool,
    storage: Rc<StorageConnection>,
    unit_registry: Option<Rc<UnitRegistry>>,
    dispatcher: EventCommandDispatcher,
    gateway: TelegramGateway,
    admin_user_ids: Vec<i64>,
    processed_update_guard: bool,
}

impl ModerationEngine {
    pub fn new(storage: StorageConnection, gateway: TelegramGateway) -> Self {
        Self {
            dry_run: false,
            storage: Rc::new(storage),
            unit_registry: None,
            dispatcher: EventCommandDispatcher::new(),
            gateway,
            admin_user_ids: Vec::new(),
            processed_update_guard: true,
        }
    }

    pub fn with_storage_handle(mut self, storage: Rc<StorageConnection>) -> Self {
        self.storage = storage;
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

    pub fn with_reason_aliases(mut self, aliases: ReasonAliasRegistry) -> Self {
        self.dispatcher = EventCommandDispatcher::with_aliases(aliases);
        self
    }

    pub fn with_dry_run(mut self, dry_run: bool) -> Self {
        self.dry_run = dry_run;
        self
    }

    pub fn with_admin_user_ids<I>(mut self, admin_user_ids: I) -> Self
    where
        I: IntoIterator<Item = i64>,
    {
        self.admin_user_ids = admin_user_ids.into_iter().collect();
        self
    }

    pub fn without_processed_update_guard(mut self) -> Self {
        self.processed_update_guard = false;
        self
    }

    pub async fn handle_event(
        &self,
        event: &EventContext,
    ) -> Result<ModerationEventResult, ModerationError> {
        event
            .validate_invariants()
            .map_err(|source| ModerationError::InvalidEvent(source.to_string()))?;

        if let Some(record) = self.claim_processed_update(event)? {
            if record.status == PROCESSED_UPDATE_STATUS_COMPLETED {
                return Ok(ModerationEventResult::Replayed(record));
            }

            return Err(ModerationError::ProcessingInterrupted(record.event_id));
        }

        let result = match self.dispatcher.dispatch(event) {
            CommandDispatchResult::Skipped(skip) => ModerationEventResult::Skipped(skip),
            CommandDispatchResult::ParseError(error) => ModerationEventResult::ParseError(error),
            CommandDispatchResult::Parsed(dispatched) => {
                let execution = self
                    .execute_command_line(event, &dispatched.parsed, &dispatched.expanded)
                    .await?;
                ModerationEventResult::Executed(execution)
            }
        };

        self.mark_processed_update(event)?;

        Ok(result)
    }

    fn claim_processed_update(
        &self,
        event: &EventContext,
    ) -> Result<Option<ProcessedUpdateRecord>, ModerationError> {
        if self.dry_run || !self.processed_update_guard {
            return Ok(None);
        }

        let Some(update_id) = event.update_id else {
            return Ok(None);
        };

        let existing = self
            .storage
            .mark_processed_update(&ProcessedUpdateRecord {
                update_id: update_id as i64,
                event_id: event.event_id.clone(),
                processed_at: event.received_at.to_rfc3339(),
                execution_mode: execution_mode_name(event.execution_mode).to_owned(),
                status: PROCESSED_UPDATE_STATUS_PENDING.to_owned(),
            })
            .map_err(ModerationError::Storage)?;

        Ok(existing)
    }

    fn mark_processed_update(&self, event: &EventContext) -> Result<(), ModerationError> {
        if self.dry_run || !self.processed_update_guard {
            return Ok(());
        }

        let Some(update_id) = event.update_id else {
            return Ok(());
        };

        let _ = self
            .storage
            .complete_processed_update(update_id as i64, &event.received_at.to_rfc3339())
            .map_err(ModerationError::Storage)?;

        Ok(())
    }

    async fn execute_command_line(
        &self,
        event: &EventContext,
        parsed: &ParsedCommandLine,
        expanded: &ExpandedCommandLine,
    ) -> Result<ModerationExecution, ModerationError> {
        self.require_admin(event)?;
        let effective_dry_run = self.dry_run || command_dry_run(&parsed.command);
        if matches!(
            (&expanded.command, &expanded.pipe),
            (ExpandedCommandAst::Mute(_), Some(_))
        ) {
            self.require_capability(event, "job.schedule")?;
        }
        let mut execution = match &expanded.command {
            ExpandedCommandAst::Warn(command) => {
                self.execute_warn(event, command, effective_dry_run).await?
            }
            ExpandedCommandAst::Mute(command) => {
                self.execute_mute(event, command, effective_dry_run).await?
            }
            ExpandedCommandAst::Ban(command) => {
                self.execute_ban(event, command, effective_dry_run).await?
            }
            ExpandedCommandAst::Del(command) => {
                self.execute_delete(event, command, effective_dry_run)
                    .await?
            }
            ExpandedCommandAst::Undo(_) => self.execute_undo(event, effective_dry_run).await?,
            ExpandedCommandAst::Msg(command) => {
                self.execute_message(event, command, effective_dry_run)
                    .await?
            }
        };

        if let (ExpandedCommandAst::Mute(command), Some(pipe)) = (&expanded.command, &expanded.pipe)
        {
            let scheduled_job = self.schedule_pipe(event, command, pipe, effective_dry_run)?;
            execution.jobs.push(scheduled_job);
        }

        Ok(execution)
    }

    async fn execute_warn(
        &self,
        event: &EventContext,
        command: &ExpandedModerationCommand,
        dry_run: bool,
    ) -> Result<ModerationExecution, ModerationError> {
        let target = describe_target(event, &command.command.target)?;
        let now = event.received_at.to_rfc3339();
        let previous_user = match target.user_id {
            Some(user_id) => self
                .storage
                .get_user(user_id)
                .map_err(ModerationError::Storage)?,
            None => None,
        };

        if let Some(user_id) = target.user_id {
            let previous = previous_user.as_ref().map_or(0, |user| user.warn_count);
            if !dry_run {
                self.storage
                    .upsert_user(&UserPatch {
                        user_id,
                        username: target.username.clone(),
                        display_name: None,
                        seen_at: now.clone(),
                        warn_count: Some(previous.saturating_add(1)),
                        shadowbanned: None,
                        reputation: None,
                        state_json: None,
                        updated_at: now.clone(),
                    })
                    .map_err(ModerationError::Storage)?;
            }
        }

        let mut execution = ModerationExecution::new(dry_run);
        if command.command.flags.public_notice {
            self.require_capability(event, "tg.write_message")?;
            let message = self
                .gateway
                .execute_checked(
                    TelegramRequest::SendMessage(TelegramSendMessageRequest {
                        chat_id: require_chat_id(event)?,
                        text: build_notice_text(
                            "warned",
                            &target.label,
                            command.expanded_reason.as_ref(),
                        ),
                        reply_to_message_id: trigger_message_id(event),
                        silent: command.command.flags.silent,
                        parse_mode: crate::tg::ParseMode::PlainText,
                        markup: None,
                    }),
                    TelegramExecutionOptions { dry_run },
                )
                .await
                .map_err(ModerationError::Telegram)?;
            execution.telegram.push(message);
        }

        let audit = self.build_audit_entry(
            event,
            AuditEntrySpec {
                op: "warn",
                target: &target,
                reversible: true,
                compensation: Some(CompensationRecipe::WarnRevert {
                user_id: target.user_id,
                previous_warn_count: previous_user.as_ref().map_or(0, |user| user.warn_count),
                }),
                args_json: json!({
                "target": target.audit_target_json(),
                "reason": reason_value(command.expanded_reason.as_ref()),
                "flags": command.command.flags,
                }),
                result_json: Some(json!({
                "warn_count": previous_user.as_ref().map_or(1, |user| user.warn_count.saturating_add(1)),
                })),
            },
        );
        if !dry_run {
            self.storage
                .append_audit_entry(&audit)
                .map_err(ModerationError::Storage)?;
        }
        execution.audit_entries.push(audit);

        Ok(execution)
    }

    async fn execute_mute(
        &self,
        event: &EventContext,
        command: &ExpandedMuteCommand,
        dry_run: bool,
    ) -> Result<ModerationExecution, ModerationError> {
        self.require_capability(event, "tg.moderate.restrict")?;
        let target = describe_target(event, &command.command.target)?;
        let user_id = require_user_id(&target, "mute")?;
        let chat_id = require_chat_id(event)?;
        let until = add_duration(event.received_at, command.command.duration)?;
        let reason = moderation_reason(command.expanded_reason.as_ref());
        let request = TelegramRequest::Restrict(TelegramRestrictRequest {
            chat_id,
            user_id,
            permissions: muted_permissions(),
            until: Some(until),
            reason: reason.clone(),
            silent: command.command.flags.silent,
            idempotency_key: Some(format!("mute:{}:{}", event.event_id, user_id)),
        });
        let telegram = self
            .gateway
            .execute_checked(request, TelegramExecutionOptions { dry_run })
            .await
            .map_err(ModerationError::Telegram)?;

        let mut execution = ModerationExecution::new(dry_run);
        execution.telegram.push(telegram.clone());

        if command.command.flags.public_notice {
            self.require_capability(event, "tg.write_message")?;
            let notice = self
                .gateway
                .execute_checked(
                    TelegramRequest::SendMessage(TelegramSendMessageRequest {
                        chat_id,
                        text: build_notice_text(
                            "muted",
                            &target.label,
                            command.expanded_reason.as_ref(),
                        ),
                        reply_to_message_id: trigger_message_id(event),
                        silent: false,
                        parse_mode: crate::tg::ParseMode::PlainText,
                        markup: None,
                    }),
                    TelegramExecutionOptions { dry_run },
                )
                .await
                .map_err(ModerationError::Telegram)?;
            execution.telegram.push(notice);
        }

        let audit = self.build_audit_entry(
            event,
            AuditEntrySpec {
                op: "mute",
                target: &target,
                reversible: true,
                compensation: Some(CompensationRecipe::Unrestrict {
                    chat_id,
                    user_id,
                    reason,
                }),
                args_json: json!({
                "target": target.audit_target_json(),
                "duration": command.command.duration,
                "reason": reason_value(command.expanded_reason.as_ref()),
                "flags": command.command.flags,
                }),
                result_json: Some(json!({
                "until": until,
                "telegram": telegram.result,
                })),
            },
        );
        if !dry_run {
            self.storage
                .append_audit_entry(&audit)
                .map_err(ModerationError::Storage)?;
        }
        execution.audit_entries.push(audit);

        Ok(execution)
    }

    async fn execute_ban(
        &self,
        event: &EventContext,
        command: &ExpandedModerationCommand,
        dry_run: bool,
    ) -> Result<ModerationExecution, ModerationError> {
        self.require_capability(event, "tg.moderate.ban")?;
        let target = describe_target(event, &command.command.target)?;
        let user_id = require_user_id(&target, "ban")?;
        let chat_id = require_chat_id(event)?;
        let reason = moderation_reason(command.expanded_reason.as_ref());
        let telegram = self
            .gateway
            .execute_checked(
                TelegramRequest::Ban(TelegramBanRequest {
                    chat_id,
                    user_id,
                    until: None,
                    delete_history: command.command.flags.delete_history,
                    reason: reason.clone(),
                    silent: command.command.flags.silent,
                    idempotency_key: Some(format!("ban:{}:{}", event.event_id, user_id)),
                }),
                TelegramExecutionOptions { dry_run },
            )
            .await
            .map_err(ModerationError::Telegram)?;

        let mut execution = ModerationExecution::new(dry_run);
        execution.telegram.push(telegram.clone());
        let audit = self.build_audit_entry(
            event,
            AuditEntrySpec {
                op: "ban",
                target: &target,
                reversible: true,
                compensation: Some(CompensationRecipe::Unban {
                    chat_id,
                    user_id,
                    reason,
                }),
                args_json: json!({
                "target": target.audit_target_json(),
                "reason": reason_value(command.expanded_reason.as_ref()),
                "flags": command.command.flags,
                }),
                result_json: Some(json!({
                "telegram": telegram.result,
                })),
            },
        );
        if !dry_run {
            self.storage
                .append_audit_entry(&audit)
                .map_err(ModerationError::Storage)?;
        }
        execution.audit_entries.push(audit);

        Ok(execution)
    }

    async fn execute_delete(
        &self,
        event: &EventContext,
        command: &DeleteCommand,
        dry_run: bool,
    ) -> Result<ModerationExecution, ModerationError> {
        self.require_capability(event, "tg.moderate.delete")?;
        let chat_id = require_chat_id(event)?;
        let (anchor_message_id, implicit_user_id) = delete_anchor(event, &command.target)?;
        let mut messages = self
            .storage
            .message_window(
                chat_id,
                i64::from(anchor_message_id),
                usize::from(command.window.up),
                usize::from(command.window.down),
                true,
            )
            .map_err(ModerationError::Storage)?;
        if let Some(since) = command.since {
            let threshold = event
                .received_at
                .checked_sub_signed(chrono::Duration::from_std(since.into_std()).map_err(
                    |error| ModerationError::Validation(format!("duration overflow: {error}")),
                )?)
                .ok_or_else(|| ModerationError::Validation("invalid delete window".to_owned()))?;
            messages.retain(|message| {
                DateTime::parse_from_rfc3339(&message.date_utc)
                    .map(|value| value.with_timezone(&Utc) >= threshold)
                    .unwrap_or(false)
            });
        }

        let requested_user_id = command
            .user_filter
            .as_ref()
            .map(resolve_numeric_user_filter)
            .transpose()?;
        let effective_user_id = requested_user_id.or(implicit_user_id);
        if let Some(user_id) = effective_user_id {
            messages.retain(|message| message.user_id == Some(user_id));
        }
        let message_ids = messages
            .iter()
            .map(|message| i32::try_from(message.message_id))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| {
                ModerationError::Validation("message_id exceeds telegram range".to_owned())
            })?;
        if message_ids.is_empty() {
            return Err(ModerationError::Validation(
                "delete selection is empty after filters".to_owned(),
            ));
        }

        let telegram = self
            .gateway
            .execute_checked(
                TelegramRequest::DeleteMany(crate::tg::TelegramDeleteManyRequest {
                    chat_id,
                    message_ids: message_ids.clone(),
                    idempotency_key: Some(format!(
                        "delete:{}:{}",
                        event.event_id, anchor_message_id
                    )),
                }),
                TelegramExecutionOptions { dry_run },
            )
            .await
            .map_err(ModerationError::Telegram)?;

        let mut execution = ModerationExecution::new(dry_run);
        execution.telegram.push(telegram.clone());
        let target = ExecutionTarget::message_anchor(anchor_message_id);
        let audit = self.build_audit_entry(
            event,
            AuditEntrySpec {
                op: "del",
                target: &target,
                reversible: false,
                compensation: None,
                args_json: json!({
                "anchor_message_id": anchor_message_id,
                "window": command.window,
                "user_filter": effective_user_id,
                "since": command.since,
                "flags": command.flags,
                }),
                result_json: Some(json!({
                "deleted": message_ids,
                "telegram": telegram.result,
                })),
            },
        );
        if !dry_run {
            self.storage
                .append_audit_entry(&audit)
                .map_err(ModerationError::Storage)?;
        }
        execution.audit_entries.push(audit);

        Ok(execution)
    }

    async fn execute_undo(
        &self,
        event: &EventContext,
        dry_run: bool,
    ) -> Result<ModerationExecution, ModerationError> {
        self.require_capability(event, "audit.compensate")?;
        let chat_id = require_chat_id(event)?;
        let reference_message_id = undo_reference_message_id(event)?;
        let original = self
            .storage
            .find_audit_entries(
                &AuditLogFilter {
                    chat_id: Some(chat_id),
                    trigger_message_id: Some(i64::from(reference_message_id)),
                    reversible: Some(true),
                    ..AuditLogFilter::default()
                },
                20,
            )
            .map_err(ModerationError::Storage)?
            .into_iter()
            .find(|entry| entry.op != "undo")
            .ok_or_else(|| {
                ModerationError::Validation(format!(
                    "no reversible audit entry found for trigger message {}",
                    reference_message_id
                ))
            })?;
        let undo_idempotency_key = format!("undo:{}", original.action_id);

        let already_undone = !self
            .storage
            .find_audit_entries(
                &AuditLogFilter {
                    idempotency_key: Some(undo_idempotency_key.clone()),
                    ..AuditLogFilter::default()
                },
                1,
            )
            .map_err(ModerationError::Storage)?
            .is_empty();
        if already_undone {
            return Err(ModerationError::Validation(format!(
                "action {} is already compensated",
                original.action_id
            )));
        }

        let recipe: CompensationRecipe =
            serde_json::from_str(original.compensation_json.as_deref().ok_or_else(|| {
                ModerationError::Validation("audit entry is not reversible".to_owned())
            })?)
            .map_err(|error| {
                ModerationError::Validation(format!("invalid compensation recipe: {error}"))
            })?;

        let mut execution = ModerationExecution::new(dry_run);
        let target = execute_compensation(self, event, &recipe, dry_run, &mut execution).await?;

        let audit = self.build_audit_entry(
            event,
            AuditEntrySpec {
                op: "undo",
                target: &target,
                reversible: false,
                compensation: None,
                args_json: json!({
                "action_id": original.action_id,
                "recipe": recipe,
                }),
                result_json: Some(json!({
                "undone_action_id": original.action_id,
                })),
            },
        );
        let audit = AuditLogEntry {
            idempotency_key: Some(undo_idempotency_key),
            ..audit
        };
        if !dry_run {
            self.storage
                .append_audit_entry(&audit)
                .map_err(ModerationError::Storage)?;
        }
        execution.audit_entries.push(audit);

        Ok(execution)
    }

    async fn execute_message(
        &self,
        event: &EventContext,
        command: &MessageCommand,
        dry_run: bool,
    ) -> Result<ModerationExecution, ModerationError> {
        self.require_capability(event, "tg.write_message")?;
        let telegram = self
            .gateway
            .execute_checked(
                TelegramRequest::SendMessage(TelegramSendMessageRequest {
                    chat_id: require_chat_id(event)?,
                    text: command.text.clone(),
                    reply_to_message_id: trigger_message_id(event),
                    silent: false,
                    parse_mode: crate::tg::ParseMode::PlainText,
                    markup: None,
                }),
                TelegramExecutionOptions { dry_run },
            )
            .await
            .map_err(ModerationError::Telegram)?;
        Ok(ModerationExecution {
            dry_run,
            telegram: vec![telegram],
            audit_entries: Vec::new(),
            jobs: Vec::new(),
        })
    }

    fn schedule_pipe(
        &self,
        event: &EventContext,
        command: &ExpandedMuteCommand,
        pipe: &ExpandedCommandLine,
        dry_run: bool,
    ) -> Result<JobRecord, ModerationError> {
        self.require_capability(event, "job.schedule")?;
        let ExpandedCommandAst::Msg(message) = &pipe.command else {
            return Err(ModerationError::UnsupportedCommand(
                "only /msg pipe is supported in phase 6".to_owned(),
            ));
        };
        let run_at = add_duration(event.received_at, command.command.duration)?.to_rfc3339();
        let now = event.received_at.to_rfc3339();
        let job = JobRecord {
            job_id: format!("job_{}", Uuid::new_v4().simple()),
            executor_unit: "moderation.pipe.message".to_owned(),
            run_at,
            scheduled_at: now.clone(),
            status: "scheduled".to_owned(),
            dedupe_key: Some(format!(
                "pipe:{}:{}",
                event.event_id,
                hash_text(&message.text)
            )),
            payload_json: json!({
                "kind": "tg.send_message",
                "chat_id": require_chat_id(event)?,
                "text": message.text,
            })
            .to_string(),
            retry_count: 0,
            max_retries: 0,
            last_error_code: None,
            last_error_text: None,
            audit_action_id: None,
            created_at: now.clone(),
            updated_at: now,
        };
        if !dry_run {
            self.storage
                .insert_job(&job)
                .map_err(ModerationError::Storage)?;
        }
        Ok(job)
    }

    fn require_capability(
        &self,
        event: &EventContext,
        capability: &'static str,
    ) -> Result<(), ModerationError> {
        let unit = event
            .system
            .unit
            .as_ref()
            .ok_or_else(|| ModerationError::CapabilityDenied {
                capability: capability.to_owned(),
                unit_id: "runtime".to_owned(),
            })?;
        let registry =
            self.unit_registry
                .as_deref()
                .ok_or_else(|| ModerationError::CapabilityDenied {
                    capability: capability.to_owned(),
                    unit_id: unit.id.clone(),
                })?;
        let descriptor = registry
            .get(&unit.id)
            .ok_or_else(|| ModerationError::UnknownUnit(unit.id.clone()))?;
        let manifest = descriptor
            .manifest
            .as_ref()
            .ok_or_else(|| ModerationError::UnknownUnit(unit.id.clone()))?;

        if manifest
            .capabilities
            .deny
            .iter()
            .any(|value| value == capability)
        {
            return Err(ModerationError::CapabilityDenied {
                capability: capability.to_owned(),
                unit_id: unit.id.clone(),
            });
        }
        if !manifest.capabilities.allow.is_empty()
            && !manifest
                .capabilities
                .allow
                .iter()
                .any(|value| value == capability)
        {
            return Err(ModerationError::CapabilityDenied {
                capability: capability.to_owned(),
                unit_id: unit.id.clone(),
            });
        }

        Ok(())
    }

    fn require_admin(&self, event: &EventContext) -> Result<(), ModerationError> {
        if event.is_synthetic() && event.sender.is_none() {
            return Ok(());
        }

        let Some(sender) = event.sender.as_ref() else {
            return Err(ModerationError::AuthorizationDenied { user_id: None });
        };

        if sender.is_admin || self.admin_user_ids.contains(&sender.id) {
            return Ok(());
        }

        Err(ModerationError::AuthorizationDenied {
            user_id: Some(sender.id),
        })
    }

    fn build_audit_entry(&self, event: &EventContext, spec: AuditEntrySpec<'_>) -> AuditLogEntry {
        AuditLogEntry {
            action_id: format!("act_{}", Uuid::new_v4().simple()),
            trace_id: event.system.trace_id.clone(),
            request_id: Some(event.event_id.clone()),
            unit_name: event
                .system
                .unit
                .as_ref()
                .map(|unit| unit.id.clone())
                .unwrap_or_else(|| "runtime".to_owned()),
            execution_mode: execution_mode_name(event.execution_mode).to_owned(),
            op: spec.op.to_owned(),
            actor_user_id: event.sender.as_ref().map(|sender| sender.id),
            chat_id: event.chat.as_ref().map(|chat| chat.id),
            target_kind: Some(spec.target.kind.clone()),
            target_id: Some(spec.target.id.clone()),
            trigger_message_id: trigger_message_id(event).map(i64::from),
            idempotency_key: Some(format!("{}:{}", spec.op, event.event_id)),
            reversible: spec.reversible,
            compensation_json: spec.compensation.map(|recipe| {
                serde_json::to_string(&recipe).expect("compensation recipe serializes")
            }),
            args_json: spec.args_json.to_string(),
            result_json: spec.result_json.map(|value| value.to_string()),
            created_at: event.received_at.to_rfc3339(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModerationEventResult {
    Executed(ModerationExecution),
    Skipped(CommandDispatchSkip),
    ParseError(CommandDispatchParseError),
    Replayed(ProcessedUpdateRecord),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModerationExecution {
    pub dry_run: bool,
    pub telegram: Vec<TelegramExecution>,
    pub audit_entries: Vec<AuditLogEntry>,
    pub jobs: Vec<JobRecord>,
}

impl ModerationExecution {
    fn new(dry_run: bool) -> Self {
        Self {
            dry_run,
            telegram: Vec::new(),
            audit_entries: Vec::new(),
            jobs: Vec::new(),
        }
    }
}

#[derive(Debug, Error)]
pub enum ModerationError {
    #[error("invalid event context: {0}")]
    InvalidEvent(String),
    #[error("validation error: {0}")]
    Validation(String),
    #[error("command is not supported in phase 6: {0}")]
    UnsupportedCommand(String),
    #[error("unknown unit `{0}`")]
    UnknownUnit(String),
    #[error("operation denied for unit `{unit_id}`: missing capability `{capability}`")]
    CapabilityDenied { capability: String, unit_id: String },
    #[error("actor is not authorized for moderation actions: user_id={user_id:?}")]
    AuthorizationDenied { user_id: Option<i64> },
    #[error("update processing was interrupted for event `{0}`")]
    ProcessingInterrupted(String),
    #[error("storage error")]
    Storage(#[from] StorageError),
    #[error("telegram error: {0}")]
    Telegram(#[from] crate::tg::TelegramError),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum CompensationRecipe {
    WarnRevert {
        user_id: Option<i64>,
        previous_warn_count: i64,
    },
    Unrestrict {
        chat_id: i64,
        user_id: i64,
        reason: Option<crate::tg::ModerationReason>,
    },
    Unban {
        chat_id: i64,
        user_id: i64,
        reason: Option<crate::tg::ModerationReason>,
    },
}

struct AuditEntrySpec<'a> {
    op: &'a str,
    target: &'a ExecutionTarget,
    reversible: bool,
    compensation: Option<CompensationRecipe>,
    args_json: Value,
    result_json: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ExecutionTarget {
    kind: String,
    id: String,
    user_id: Option<i64>,
    username: Option<String>,
    label: String,
}

impl ExecutionTarget {
    fn message_anchor(message_id: MessageId) -> Self {
        Self {
            kind: "message".to_owned(),
            id: message_id.to_string(),
            user_id: None,
            username: None,
            label: format!("message:{message_id}"),
        }
    }

    fn audit_target_json(&self) -> Value {
        json!({
            "kind": self.kind,
            "id": self.id,
            "user_id": self.user_id,
            "username": self.username,
            "label": self.label,
        })
    }
}

async fn execute_compensation(
    engine: &ModerationEngine,
    event: &EventContext,
    recipe: &CompensationRecipe,
    dry_run: bool,
    execution: &mut ModerationExecution,
) -> Result<ExecutionTarget, ModerationError> {
    match recipe {
        CompensationRecipe::WarnRevert {
            user_id,
            previous_warn_count,
        } => {
            let user_id = user_id.ok_or_else(|| {
                ModerationError::Validation("warn action does not have a numeric target".to_owned())
            })?;
            if !dry_run {
                let current = engine
                    .storage
                    .get_user(user_id)
                    .map_err(ModerationError::Storage)?;
                engine
                    .storage
                    .upsert_user(&UserPatch {
                        user_id,
                        username: current.as_ref().and_then(|value| value.username.clone()),
                        display_name: current
                            .as_ref()
                            .and_then(|value| value.display_name.clone()),
                        seen_at: event.received_at.to_rfc3339(),
                        warn_count: Some(*previous_warn_count),
                        shadowbanned: None,
                        reputation: None,
                        state_json: current.as_ref().and_then(|value| value.state_json.clone()),
                        updated_at: event.received_at.to_rfc3339(),
                    })
                    .map_err(ModerationError::Storage)?;
            }
            Ok(ExecutionTarget {
                kind: "user".to_owned(),
                id: user_id.to_string(),
                user_id: Some(user_id),
                username: None,
                label: user_id.to_string(),
            })
        }
        CompensationRecipe::Unrestrict {
            chat_id,
            user_id,
            reason,
        } => {
            engine.require_capability(event, "tg.moderate.restrict")?;
            let telegram = engine
                .gateway
                .execute_checked(
                    TelegramRequest::Unrestrict(TelegramUnrestrictRequest {
                        chat_id: *chat_id,
                        user_id: *user_id,
                        reason: reason.clone(),
                        silent: false,
                        idempotency_key: Some(format!("undo:{}:{user_id}", event.event_id)),
                    }),
                    TelegramExecutionOptions { dry_run },
                )
                .await
                .map_err(ModerationError::Telegram)?;
            execution.telegram.push(telegram);
            Ok(ExecutionTarget {
                kind: "user".to_owned(),
                id: user_id.to_string(),
                user_id: Some(*user_id),
                username: None,
                label: user_id.to_string(),
            })
        }
        CompensationRecipe::Unban {
            chat_id,
            user_id,
            reason,
        } => {
            engine.require_capability(event, "tg.moderate.ban")?;
            let telegram = engine
                .gateway
                .execute_checked(
                    TelegramRequest::Unban(TelegramUnbanRequest {
                        chat_id: *chat_id,
                        user_id: *user_id,
                        only_if_banned: true,
                        reason: reason.clone(),
                        silent: false,
                        idempotency_key: Some(format!("undo-ban:{}:{user_id}", event.event_id)),
                    }),
                    TelegramExecutionOptions { dry_run },
                )
                .await
                .map_err(ModerationError::Telegram)?;
            execution.telegram.push(telegram);
            Ok(ExecutionTarget {
                kind: "user".to_owned(),
                id: user_id.to_string(),
                user_id: Some(*user_id),
                username: None,
                label: user_id.to_string(),
            })
        }
    }
}

fn describe_target(
    event: &EventContext,
    target: &ResolvedTarget,
) -> Result<ExecutionTarget, ModerationError> {
    match &target.selector {
        ParsedTargetSelector::UserId { user_id } => Ok(ExecutionTarget {
            kind: "user".to_owned(),
            id: user_id.to_string(),
            user_id: Some(*user_id),
            username: None,
            label: user_id.to_string(),
        }),
        ParsedTargetSelector::Username { username } => Ok(ExecutionTarget {
            kind: "user".to_owned(),
            id: username.clone(),
            user_id: None,
            username: Some(username.clone()),
            label: format!("@{username}"),
        }),
        ParsedTargetSelector::MessageAnchor { message_id } => {
            Ok(ExecutionTarget::message_anchor(*message_id))
        }
        ParsedTargetSelector::Reply => {
            let reply = event.reply.as_ref().ok_or_else(|| {
                ModerationError::Validation("reply target requires reply context".to_owned())
            })?;
            if let Some(user_id) = reply.sender_user_id {
                Ok(ExecutionTarget {
                    kind: "user".to_owned(),
                    id: user_id.to_string(),
                    user_id: Some(user_id),
                    username: reply.sender_username.clone(),
                    label: reply
                        .sender_username
                        .as_ref()
                        .map(|username| format!("@{username}"))
                        .unwrap_or_else(|| user_id.to_string()),
                })
            } else {
                Ok(ExecutionTarget::message_anchor(reply.message_id))
            }
        }
        ParsedTargetSelector::JsonSelector { raw } => {
            let user_id = raw.get("id").and_then(Value::as_i64);
            let username = raw
                .get("username")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned);
            Ok(ExecutionTarget {
                kind: raw
                    .get("kind")
                    .and_then(Value::as_str)
                    .unwrap_or("json_selector")
                    .to_owned(),
                id: raw.to_string(),
                user_id,
                username: username.clone(),
                label: username
                    .map(|value| format!("@{value}"))
                    .or_else(|| user_id.map(|value| value.to_string()))
                    .unwrap_or_else(|| raw.to_string()),
            })
        }
    }
}

fn delete_anchor(
    event: &EventContext,
    target: &ResolvedTarget,
) -> Result<(MessageId, Option<i64>), ModerationError> {
    match (&target.selector, target.source) {
        (ParsedTargetSelector::MessageAnchor { message_id }, _) => Ok((*message_id, None)),
        (ParsedTargetSelector::Reply, _) => event
            .reply
            .as_ref()
            .map(|reply| (reply.message_id, reply.sender_user_id))
            .ok_or_else(|| {
                ModerationError::Validation("reply delete target requires reply context".to_owned())
            }),
        (ParsedTargetSelector::UserId { user_id }, TargetSource::ReplyContext) => event
            .reply
            .as_ref()
            .map(|reply| (reply.message_id, Some(*user_id)))
            .ok_or_else(|| {
                ModerationError::Validation("reply delete target requires reply context".to_owned())
            }),
        _ => Err(ModerationError::Validation(
            "delete command requires a message anchor or reply context".to_owned(),
        )),
    }
}

fn resolve_numeric_user_filter(selector: &ParsedTargetSelector) -> Result<i64, ModerationError> {
    match selector {
        ParsedTargetSelector::UserId { user_id } => Ok(*user_id),
        _ => Err(ModerationError::Validation(
            "delete user filter must resolve to numeric user_id".to_owned(),
        )),
    }
}

fn require_chat_id(event: &EventContext) -> Result<i64, ModerationError> {
    event
        .chat
        .as_ref()
        .map(|chat| chat.id)
        .ok_or_else(|| ModerationError::Validation("chat context is required".to_owned()))
}

fn require_user_id(target: &ExecutionTarget, op: &str) -> Result<i64, ModerationError> {
    target.user_id.ok_or_else(|| {
        ModerationError::Validation(format!(
            "`/{op}` requires a numeric target user id or reply context"
        ))
    })
}

fn trigger_message_id(event: &EventContext) -> Option<MessageId> {
    event
        .message
        .as_ref()
        .map(|message| message.id)
        .or_else(|| {
            event
                .callback
                .as_ref()
                .and_then(|callback| callback.message_id)
        })
}

fn undo_reference_message_id(event: &EventContext) -> Result<MessageId, ModerationError> {
    event
        .message
        .as_ref()
        .and_then(|message| message.reply_to_message_id)
        .or_else(|| {
            event
                .callback
                .as_ref()
                .and_then(|callback| callback.message_id)
        })
        .ok_or_else(|| {
            ModerationError::Validation(
                "undo requires reply_to_message_id or callback message context".to_owned(),
            )
        })
}

fn build_notice_text(action: &str, target: &str, reason: Option<&ExpandedReason>) -> String {
    let reason = reason
        .map(reason_text)
        .unwrap_or_else(|| "without a recorded reason".to_owned());
    format!("{action} {target}: {reason}")
}

fn moderation_reason(reason: Option<&ExpandedReason>) -> Option<crate::tg::ModerationReason> {
    reason.map(|reason| crate::tg::ModerationReason {
        code: match reason {
            ExpandedReason::RuleCode { code } => Some(code.clone()),
            ExpandedReason::Alias { definition, .. } => definition.rule_code.clone(),
            ExpandedReason::UnknownAlias { .. }
            | ExpandedReason::Quoted { .. }
            | ExpandedReason::FreeText { .. } => None,
        },
        text: Some(reason_text(reason)),
    })
}

fn reason_value(reason: Option<&ExpandedReason>) -> Value {
    reason
        .map(|value| serde_json::to_value(value).expect("expanded reason serializes"))
        .unwrap_or(Value::Null)
}

fn reason_text(reason: &ExpandedReason) -> String {
    match reason {
        ExpandedReason::RuleCode { code } => code.clone(),
        ExpandedReason::Alias { definition, .. } => definition.canonical.clone(),
        ExpandedReason::UnknownAlias { alias } => alias.clone(),
        ExpandedReason::Quoted { text } | ExpandedReason::FreeText { text } => text.clone(),
    }
}

fn muted_permissions() -> TelegramPermissions {
    TelegramPermissions {
        can_send_messages: Some(false),
        can_send_audios: Some(false),
        can_send_documents: Some(false),
        can_send_photos: Some(false),
        can_send_videos: Some(false),
        can_send_video_notes: Some(false),
        can_send_voice_notes: Some(false),
        can_send_polls: Some(false),
        can_send_other_messages: Some(false),
        can_add_web_page_previews: Some(false),
        can_change_info: None,
        can_invite_users: None,
        can_pin_messages: None,
        can_manage_topics: None,
    }
}

fn add_duration(
    received_at: DateTime<Utc>,
    duration: crate::parser::duration::ParsedDuration,
) -> Result<DateTime<Utc>, ModerationError> {
    let chrono_duration = chrono::Duration::from_std(duration.into_std())
        .map_err(|error| ModerationError::Validation(format!("duration overflow: {error}")))?;

    received_at.checked_add_signed(chrono_duration).ok_or_else(|| {
        ModerationError::Validation("mute duration exceeds supported range".to_owned())
    })
}

fn command_dry_run(command: &CommandAst) -> bool {
    match command {
        CommandAst::Warn(command) | CommandAst::Ban(command) => command.flags.dry_run,
        CommandAst::Mute(command) => command.flags.dry_run,
        CommandAst::Del(command) => command.flags.dry_run,
        CommandAst::Undo(command) => command.dry_run,
        CommandAst::Msg(_) => false,
    }
}

fn execution_mode_name(mode: ExecutionMode) -> &'static str {
    match mode {
        ExecutionMode::Realtime => "realtime",
        ExecutionMode::Manual => "manual",
        ExecutionMode::Scheduled => "scheduled",
        ExecutionMode::Recovery => "recovery",
    }
}

fn hash_text(input: &str) -> u64 {
    use std::hash::{Hash, Hasher};

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    input.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::{ModerationEngine, ModerationError, ModerationEventResult};
    use crate::event::{
        ChatContext, EventNormalizer, ManualInvocationInput, MessageContext, ReplyContext,
        SenderContext, TelegramUpdateInput, UnitContext,
    };
    use crate::tg::{
        TelegramDeleteResult, TelegramGateway, TelegramMessageResult, TelegramRequest,
        TelegramResult, TelegramTransport, TelegramUiResult,
    };
    use crate::unit::{
        CapabilitiesSpec, ServiceSpec, TriggerSpec, UnitDefinition, UnitManifest, UnitRegistry,
    };
    use async_trait::async_trait;
    use chrono::{TimeZone, Utc};
    use std::sync::{Arc, Mutex};
    use tempfile::tempdir;

    use crate::storage::{
        AuditLogFilter, MessageJournalRecord, ProcessedUpdateRecord,
        PROCESSED_UPDATE_STATUS_PENDING, Storage,
    };

    #[derive(Debug, Default)]
    struct RecordingTransport {
        requests: Arc<Mutex<Vec<TelegramRequest>>>,
    }

    #[async_trait]
    impl TelegramTransport for RecordingTransport {
        fn name(&self) -> &'static str {
            "recording"
        }

        async fn execute(
            &self,
            request: TelegramRequest,
        ) -> Result<TelegramResult, crate::tg::TelegramError> {
            self.requests
                .lock()
                .expect("requests lock")
                .push(request.clone());

            Ok(match request {
                TelegramRequest::SendMessage(request) => {
                    TelegramResult::Message(TelegramMessageResult {
                        chat_id: request.chat_id,
                        message_id: request.reply_to_message_id.unwrap_or(900).saturating_add(1),
                        raw_passthrough: false,
                    })
                }
                TelegramRequest::DeleteMany(request) => {
                    TelegramResult::Delete(TelegramDeleteResult {
                        chat_id: request.chat_id,
                        deleted: request.message_ids,
                        failed: Vec::new(),
                    })
                }
                TelegramRequest::Restrict(request) => {
                    TelegramResult::Restriction(crate::tg::TelegramRestrictionResult {
                        chat_id: request.chat_id,
                        user_id: request.user_id,
                        until: request.until,
                        permissions: request.permissions,
                        changed: true,
                    })
                }
                TelegramRequest::Unrestrict(request) => {
                    TelegramResult::Restriction(crate::tg::TelegramRestrictionResult {
                        chat_id: request.chat_id,
                        user_id: request.user_id,
                        until: None,
                        permissions: crate::tg::TelegramPermissions::default(),
                        changed: true,
                    })
                }
                TelegramRequest::Ban(request) => {
                    TelegramResult::Ban(crate::tg::TelegramBanResult {
                        chat_id: request.chat_id,
                        user_id: request.user_id,
                        until: request.until,
                        delete_history: request.delete_history,
                        changed: true,
                    })
                }
                TelegramRequest::Unban(request) => {
                    TelegramResult::Ban(crate::tg::TelegramBanResult {
                        chat_id: request.chat_id,
                        user_id: request.user_id,
                        until: None,
                        delete_history: false,
                        changed: true,
                    })
                }
                TelegramRequest::SendUi(request) => TelegramResult::Ui(TelegramUiResult {
                    chat_id: request.chat_id,
                    message_id: request.reply_to_message_id.unwrap_or(700).saturating_add(1),
                    template: request.template,
                    edited: false,
                    raw_passthrough: false,
                }),
                TelegramRequest::EditUi(request) => TelegramResult::Ui(TelegramUiResult {
                    chat_id: request.chat_id,
                    message_id: request.message_id,
                    template: request.template,
                    edited: true,
                    raw_passthrough: false,
                }),
                TelegramRequest::Delete(request) => TelegramResult::Delete(TelegramDeleteResult {
                    chat_id: request.chat_id,
                    deleted: vec![request.message_id],
                    failed: Vec::new(),
                }),
                TelegramRequest::AnswerCallback(request) => {
                    TelegramResult::Callback(crate::tg::TelegramCallbackResult {
                        callback_query_id: request.callback_query_id,
                        answered: true,
                        show_alert: request.show_alert,
                        text: request.text,
                    })
                }
            })
        }
    }

    fn ts() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 4, 22, 11, 0, 0)
            .single()
            .expect("valid timestamp")
    }

    fn chat() -> ChatContext {
        ChatContext {
            id: -100123,
            chat_type: "supergroup".to_owned(),
            title: Some("Moderation HQ".to_owned()),
            username: Some("mod_hq".to_owned()),
            thread_id: None,
        }
    }

    fn sender() -> SenderContext {
        SenderContext {
            id: 42,
            username: Some("admin".to_owned()),
            display_name: Some("Admin".to_owned()),
            is_bot: false,
            is_admin: true,
            role: Some("owner".to_owned()),
        }
    }

    fn non_admin_sender() -> SenderContext {
        SenderContext {
            id: 777,
            username: Some("member".to_owned()),
            display_name: Some("Member".to_owned()),
            is_bot: false,
            is_admin: false,
            role: Some("member".to_owned()),
        }
    }

    fn registry_with_caps(caps: &[&str]) -> UnitRegistry {
        let mut manifest = UnitManifest::new(
            UnitDefinition::new("moderation.test"),
            TriggerSpec::command(["warn", "mute", "del", "undo"]),
            ServiceSpec::new("scripts/moderation/test.rhai"),
        );
        manifest.capabilities = CapabilitiesSpec {
            allow: caps.iter().map(|value| (*value).to_owned()).collect(),
            deny: Vec::new(),
        };
        UnitRegistry::load_manifests(vec![manifest]).registry
    }

    fn engine_with_caps(
        caps: &[&str],
    ) -> (
        tempfile::TempDir,
        Arc<Mutex<Vec<TelegramRequest>>>,
        ModerationEngine,
    ) {
        engine_with_caps_and_admins(caps, [])
    }

    fn engine_with_caps_and_admins<I>(
        caps: &[&str],
        admin_user_ids: I,
    ) -> (
        tempfile::TempDir,
        Arc<Mutex<Vec<TelegramRequest>>>,
        ModerationEngine,
    )
    where
        I: IntoIterator<Item = i64>,
    {
        let dir = tempdir().expect("tempdir");
        let storage = Storage::new(dir.path().join("runtime.sqlite3"))
            .bootstrap()
            .expect("bootstrap")
            .into_connection();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let transport = RecordingTransport {
            requests: Arc::clone(&requests),
        };
        let gateway = TelegramGateway::new(false).with_transport(transport);
        let engine = ModerationEngine::new(storage, gateway)
            .with_unit_registry(registry_with_caps(caps))
            .with_admin_user_ids(admin_user_ids);
        (dir, requests, engine)
    }

    fn engine_without_registry() -> (
        tempfile::TempDir,
        Arc<Mutex<Vec<TelegramRequest>>>,
        ModerationEngine,
    ) {
        let dir = tempdir().expect("tempdir");
        let storage = Storage::new(dir.path().join("runtime.sqlite3"))
            .bootstrap()
            .expect("bootstrap")
            .into_connection();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let transport = RecordingTransport {
            requests: Arc::clone(&requests),
        };
        let gateway = TelegramGateway::new(false).with_transport(transport);
        let engine = ModerationEngine::new(storage, gateway);
        (dir, requests, engine)
    }

    fn manual_event(command_text: &str) -> crate::event::EventContext {
        let normalizer = EventNormalizer::new();
        let mut input = ManualInvocationInput::new(
            UnitContext::new("moderation.test").with_trigger("manual"),
            command_text,
        );
        input.received_at = ts();
        input.chat = Some(chat());
        input.sender = Some(sender());
        normalizer
            .normalize_manual(input)
            .expect("manual event normalizes")
    }

    fn reply_event(
        command_text: &str,
        reply_user_id: i64,
        reply_message_id: i32,
    ) -> crate::event::EventContext {
        let mut event = manual_event(command_text);
        event.reply = Some(ReplyContext {
            message_id: reply_message_id,
            sender_user_id: Some(reply_user_id),
            sender_username: Some("spam_user".to_owned()),
            text: Some("spam".to_owned()),
            has_media: false,
        });
        event.message = Some(MessageContext {
            id: 900,
            date: ts(),
            text: Some(command_text.to_owned()),
            content_kind: Some(crate::event::MessageContentKind::Text),
            entities: vec!["bot_command".to_owned()],
            has_media: false,
            file_ids: Vec::new(),
            reply_to_message_id: Some(reply_message_id),
            media_group_id: None,
        });
        event
    }

    fn reply_event_with_sender(
        command_text: &str,
        reply_user_id: i64,
        reply_message_id: i32,
        sender: SenderContext,
    ) -> crate::event::EventContext {
        let mut event = reply_event(command_text, reply_user_id, reply_message_id);
        event.sender = Some(sender);
        event
    }

    fn seed_journal(engine: &ModerationEngine) {
        for (message_id, user_id) in [(810, Some(99)), (811, Some(77)), (812, Some(99))] {
            engine
                .storage
                .append_message_journal(&MessageJournalRecord {
                    chat_id: -100123,
                    message_id,
                    user_id,
                    date_utc: ts().to_rfc3339(),
                    update_type: "message".to_owned(),
                    text: Some(format!("msg-{message_id}")),
                    normalized_text: None,
                    has_media: false,
                    reply_to_message_id: None,
                    file_ids_json: None,
                    meta_json: None,
                })
                .expect("journal insert");
        }
    }

    #[tokio::test]
    async fn warn_updates_user_and_audit_log() {
        let (_dir, _requests, engine) = engine_with_caps(&[]);
        let event = reply_event("/warn 2.8", 99, 810);

        let result = engine.handle_event(&event).await.expect("warn succeeds");

        let ModerationEventResult::Executed(execution) = result else {
            panic!("expected executed result");
        };
        assert_eq!(execution.audit_entries.len(), 1);
        let user = engine
            .storage
            .get_user(99)
            .expect("user lookup")
            .expect("user exists");
        assert_eq!(user.warn_count, 1);
        assert_eq!(execution.audit_entries[0].op, "warn");
        assert!(execution.audit_entries[0].reversible);
    }

    #[tokio::test]
    async fn mute_executes_restrict_and_schedules_pipe_message() {
        let (_dir, requests, engine) =
            engine_with_caps(&["tg.moderate.restrict", "job.schedule", "tg.write_message"]);
        let event = reply_event(r#"/mute 30m spam | /msg "mute expired""#, 99, 810);

        let result = engine.handle_event(&event).await.expect("mute succeeds");

        let ModerationEventResult::Executed(execution) = result else {
            panic!("expected executed result");
        };
        assert_eq!(execution.telegram.len(), 1);
        assert_eq!(execution.audit_entries[0].op, "mute");
        assert_eq!(execution.jobs.len(), 1);
        let stored_job = engine
            .storage
            .get_job(&execution.jobs[0].job_id)
            .expect("job lookup")
            .expect("job exists");
        assert_eq!(stored_job.executor_unit, "moderation.pipe.message");
        let requests = requests.lock().expect("requests");
        assert!(matches!(requests[0], TelegramRequest::Restrict(_)));
    }

    #[tokio::test]
    async fn mute_pipe_requires_job_schedule_before_side_effects() {
        let (_dir, requests, engine) = engine_with_caps(&["tg.moderate.restrict"]);
        let event = reply_event(r#"/mute 30m spam | /msg "mute expired""#, 99, 810);

        let error = engine
            .handle_event(&event)
            .await
            .expect_err("mute pipe must be denied");

        assert!(matches!(
            error,
            ModerationError::CapabilityDenied {
                capability,
                unit_id,
            } if capability == "job.schedule" && unit_id == "moderation.test"
        ));
        assert!(requests.lock().expect("requests").is_empty());
        assert!(
            engine
                .storage
                .find_audit_entries(&AuditLogFilter::default(), 10)
                .expect("audit lookup")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn delete_window_uses_anchor_and_user_filter() {
        let (_dir, requests, engine) = engine_with_caps(&["tg.moderate.delete"]);
        seed_journal(&engine);
        let event = manual_event("/del msg:811 -up 1 -dn 1 -user 99");

        let result = engine.handle_event(&event).await.expect("delete succeeds");

        let ModerationEventResult::Executed(execution) = result else {
            panic!("expected executed result");
        };
        assert_eq!(execution.audit_entries[0].op, "del");
        let requests = requests.lock().expect("requests");
        let TelegramRequest::DeleteMany(request) = &requests[0] else {
            panic!("expected delete_many request");
        };
        assert_eq!(request.message_ids, vec![810, 812]);
    }

    #[tokio::test]
    async fn undo_compensates_previous_mute() {
        let (_dir, requests, engine) =
            engine_with_caps(&["tg.moderate.restrict", "audit.compensate"]);
        let mute_event = reply_event("/mute 30m spam", 99, 810);
        let mute_result = engine
            .handle_event(&mute_event)
            .await
            .expect("mute succeeds");
        let ModerationEventResult::Executed(mute_execution) = mute_result else {
            panic!("expected executed mute");
        };
        let original_action_id = mute_execution.audit_entries[0].action_id.clone();

        let undo_event = reply_event("/undo", 99, 900);
        let undo_result = engine
            .handle_event(&undo_event)
            .await
            .expect("undo succeeds");

        let ModerationEventResult::Executed(execution) = undo_result else {
            panic!("expected executed undo");
        };
        assert_eq!(execution.audit_entries[0].op, "undo");
        assert_eq!(execution.audit_entries[0].target_id.as_deref(), Some("99"));
        let requests = requests.lock().expect("requests");
        assert!(matches!(requests[0], TelegramRequest::Restrict(_)));
        assert!(matches!(requests[1], TelegramRequest::Unrestrict(_)));
        let undo_entries = engine
            .storage
            .find_audit_entries(
                &AuditLogFilter {
                    op: Some("undo".to_owned()),
                    target_id: Some("99".to_owned()),
                    ..AuditLogFilter::default()
                },
                10,
            )
            .expect("audit lookup");
        assert_eq!(undo_entries.len(), 1);
        assert_ne!(undo_entries[0].action_id, original_action_id);
    }

    #[tokio::test]
    async fn undo_cannot_compensate_same_action_twice() {
        let (_dir, _requests, engine) =
            engine_with_caps(&["tg.moderate.restrict", "audit.compensate"]);
        let mute_event = reply_event("/mute 30m spam", 99, 810);
        let mute_result = engine
            .handle_event(&mute_event)
            .await
            .expect("mute succeeds");
        let ModerationEventResult::Executed(mute_execution) = mute_result else {
            panic!("expected executed mute");
        };

        let undo_event = reply_event("/undo", 99, 900);
        let first_undo = engine
            .handle_event(&undo_event)
            .await
            .expect("first undo succeeds");
        assert!(matches!(first_undo, ModerationEventResult::Executed(_)));

        let error = engine
            .handle_event(&undo_event)
            .await
            .expect_err("second undo must fail");

        assert!(matches!(
            error,
            ModerationError::Validation(message)
            if message == format!("action {} is already compensated", mute_execution.audit_entries[0].action_id)
        ));
    }

    #[tokio::test]
    async fn replayed_update_is_skipped_without_duplicate_transport_calls() {
        let (_dir, requests, engine) = engine_with_caps(&["tg.moderate.delete"]);
        seed_journal(&engine);
        let normalizer = EventNormalizer::new();
        let mut input = TelegramUpdateInput::message(
            1001,
            chat(),
            sender(),
            MessageContext {
                id: 811,
                date: ts(),
                text: Some("/del msg:811".to_owned()),
                content_kind: Some(crate::event::MessageContentKind::Text),
                entities: vec!["bot_command".to_owned()],
                has_media: false,
                file_ids: Vec::new(),
                reply_to_message_id: None,
                media_group_id: None,
            },
        );
        input.event_id = Some("evt_tg_delete".to_owned());
        input.received_at = ts();
        let mut event = normalizer
            .normalize_telegram(input)
            .expect("telegram event normalizes");
        event.system.unit = Some(UnitContext::new("moderation.test").with_trigger("telegram"));

        let first = engine
            .handle_event(&event)
            .await
            .expect("first pass succeeds");
        assert!(matches!(first, ModerationEventResult::Executed(_)));
        let second = engine.handle_event(&event).await.expect("replay succeeds");
        assert!(matches!(second, ModerationEventResult::Replayed(_)));
        assert_eq!(requests.lock().expect("requests").len(), 1);
    }

    #[tokio::test]
    async fn pending_realtime_update_fails_closed_without_reexecution() {
        let (_dir, requests, engine) = engine_with_caps(&["tg.moderate.delete"]);
        seed_journal(&engine);
        let normalizer = EventNormalizer::new();
        let mut input = TelegramUpdateInput::message(
            1002,
            chat(),
            sender(),
            MessageContext {
                id: 811,
                date: ts(),
                text: Some("/del msg:811".to_owned()),
                content_kind: Some(crate::event::MessageContentKind::Text),
                entities: vec!["bot_command".to_owned()],
                has_media: false,
                file_ids: Vec::new(),
                reply_to_message_id: None,
                media_group_id: None,
            },
        );
        input.event_id = Some("evt_tg_delete_pending".to_owned());
        input.received_at = ts();
        let mut event = normalizer
            .normalize_telegram(input)
            .expect("telegram event normalizes");
        event.system.unit = Some(UnitContext::new("moderation.test").with_trigger("telegram"));
        engine
            .storage
            .mark_processed_update(&ProcessedUpdateRecord {
                update_id: 1002,
                event_id: "evt_tg_delete_pending".to_owned(),
                processed_at: ts().to_rfc3339(),
                execution_mode: "realtime".to_owned(),
                status: PROCESSED_UPDATE_STATUS_PENDING.to_owned(),
            })
            .expect("pending mark succeeds");

        let error = engine
            .handle_event(&event)
            .await
            .expect_err("pending update must fail closed");

        assert!(matches!(
            error,
            ModerationError::ProcessingInterrupted(event_id)
            if event_id == "evt_tg_delete_pending"
        ));
        assert!(requests.lock().expect("requests").is_empty());
    }

    #[tokio::test]
    async fn capability_denial_is_structured() {
        let (_dir, _requests, engine) = engine_with_caps(&["audit.compensate"]);
        let event = reply_event("/mute 30m spam", 99, 810);

        let error = engine
            .handle_event(&event)
            .await
            .expect_err("mute must be denied");

        assert!(matches!(
            error,
            ModerationError::CapabilityDenied {
                capability,
                unit_id,
            } if capability == "tg.moderate.restrict" && unit_id == "moderation.test"
        ));
    }

    #[tokio::test]
    async fn capability_gated_operation_fails_closed_without_registry() {
        let (_dir, requests, engine) = engine_without_registry();
        let event = reply_event("/mute 30m spam", 99, 810);

        let error = engine
            .handle_event(&event)
            .await
            .expect_err("mute must be denied without registry");

        assert!(matches!(
            error,
            ModerationError::CapabilityDenied {
                capability,
                unit_id,
            } if capability == "tg.moderate.restrict" && unit_id == "moderation.test"
        ));
        assert!(requests.lock().expect("requests").is_empty());
    }

    #[tokio::test]
    async fn non_admin_sender_cannot_execute_moderation_command() {
        let (_dir, requests, engine) = engine_with_caps(&["tg.moderate.restrict"]);
        let event = reply_event_with_sender("/mute 30m spam", 99, 810, non_admin_sender());

        let error = engine
            .handle_event(&event)
            .await
            .expect_err("non-admin sender must be denied");

        assert!(matches!(
            error,
            ModerationError::AuthorizationDenied { user_id: Some(777) }
        ));
        assert!(requests.lock().expect("requests").is_empty());
    }

    #[tokio::test]
    async fn configured_admin_id_can_execute_even_without_sender_admin_flag() {
        let (_dir, requests, engine) =
            engine_with_caps_and_admins(&["tg.moderate.restrict"], [777]);
        let event = reply_event_with_sender("/mute 30m spam", 99, 810, non_admin_sender());

        let result = engine.handle_event(&event).await.expect("configured admin succeeds");

        assert!(matches!(result, ModerationEventResult::Executed(_)));
        assert_eq!(requests.lock().expect("requests").len(), 1);
    }
}
