use super::ModerationEngine;
use super::helpers::*;
use super::types::{
    AuditEntrySpec, CompensationRecipe, ExecutionTarget, ModerationError, ModerationExecution,
    ModerationUnitPolicy,
};
use super::undo::execute_compensation;
use crate::event::EventContext;
use crate::parser::command::{DeleteCommand, MessageCommand, ParsedCommandLine};
use crate::parser::reason::{
    ExpandedCommandAst, ExpandedCommandLine, ExpandedModerationCommand, ExpandedMuteCommand,
};
use crate::storage::{AuditLogEntry, AuditLogFilter, JobRecord, UserPatch, UserRecord};
use crate::tg::{
    TelegramBanRequest, TelegramExecutionOptions, TelegramRequest, TelegramRestrictRequest,
    TelegramSendMessageRequest,
};
use chrono::{DateTime, Utc};
use serde_json::json;
use std::collections::HashMap;

const HELP_TEMPLATE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/bundled_templates/ui/help.txt"
));
const BAN_TEMPLATE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/bundled_templates/moderation/ban.txt"
));
const WARN_TEMPLATE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/bundled_templates/moderation/warn.txt"
));
const MUTE_TEMPLATE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/bundled_templates/moderation/mute.txt"
));
const UNDO_SUCCESS_TEMPLATE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/bundled_templates/moderation/undo_success.txt"
));

impl ModerationEngine {
    pub(crate) async fn execute_command_line(
        &self,
        event: &EventContext,
        parsed: &ParsedCommandLine,
        expanded: &ExpandedCommandLine,
        unit_policy: Option<&ModerationUnitPolicy>,
    ) -> Result<ModerationExecution, ModerationError> {
        let is_public_utility = matches!(
            &expanded.command,
            ExpandedCommandAst::Ping(_) | ExpandedCommandAst::Help(_)
        );
        if !is_public_utility {
            self.require_admin(event)?;
            if let Some(chat) = event.chat.as_ref() {
                self.require_bot_admin(chat.id)?;
            }
        }
        let effective_dry_run = self.dry_run || command_dry_run(&parsed.command);
        if matches!(
            (&expanded.command, &expanded.pipe),
            (ExpandedCommandAst::Mute(_), Some(_))
        ) {
            self.require_capability(event, unit_policy, "job.schedule")?;
        }
        let mut execution = match &expanded.command {
            ExpandedCommandAst::Warn(command) => {
                self.execute_warn(event, command, effective_dry_run, unit_policy)
                    .await?
            }
            ExpandedCommandAst::Mute(command) => {
                self.execute_mute(event, command, effective_dry_run, unit_policy)
                    .await?
            }
            ExpandedCommandAst::Ban(command) => {
                self.execute_ban(event, command, effective_dry_run, unit_policy)
                    .await?
            }
            ExpandedCommandAst::Del(command) => {
                self.execute_delete(event, command, effective_dry_run, unit_policy)
                    .await?
            }
            ExpandedCommandAst::Undo(_) => {
                self.execute_undo(event, effective_dry_run, unit_policy)
                    .await?
            }
            ExpandedCommandAst::Msg(command) => {
                self.execute_message(event, command, effective_dry_run, unit_policy)
                    .await?
            }
            ExpandedCommandAst::Help(_) => self.execute_help(event, effective_dry_run).await?,
            ExpandedCommandAst::Ping(_) => self.execute_ping(event, effective_dry_run).await?,
        };

        if let (ExpandedCommandAst::Mute(command), Some(pipe)) = (&expanded.command, &expanded.pipe)
        {
            let scheduled_job =
                self.schedule_pipe(event, command, pipe, effective_dry_run, unit_policy)?;
            execution.jobs.push(scheduled_job);
        }

        Ok(execution)
    }

    pub(crate) async fn execute_warn(
        &self,
        event: &EventContext,
        command: &ExpandedModerationCommand,
        dry_run: bool,
        unit_policy: Option<&ModerationUnitPolicy>,
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
            self.require_capability(event, unit_policy, "tg.write_message")?;
            let mut vars = confirmation_vars(
                event,
                &target,
                previous_user.as_ref(),
                command.expanded_reason.as_ref(),
            );
            vars.insert(
                "warn_count".to_owned(),
                previous_user
                    .as_ref()
                    .map_or(1, |user| user.warn_count.saturating_add(1))
                    .to_string(),
            );
            vars.insert("warn_limit".to_owned(), "∞".to_owned());
            let message = self
                .gateway
                .execute_checked(
                    TelegramRequest::SendMessage(TelegramSendMessageRequest {
                        chat_id: require_chat_id(event)?,
                        text: crate::host_api::template::render_template(WARN_TEMPLATE, vars),
                        reply_to_message_id: None,
                        silent: command.command.flags.silent,
                        parse_mode: crate::tg::ParseMode::Html,
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
            unit_policy,
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

    pub(crate) async fn execute_mute(
        &self,
        event: &EventContext,
        command: &ExpandedMuteCommand,
        dry_run: bool,
        unit_policy: Option<&ModerationUnitPolicy>,
    ) -> Result<ModerationExecution, ModerationError> {
        self.require_capability(event, unit_policy, "tg.moderate.restrict")?;
        let target = describe_target(event, &command.command.target)?;
        let user_id = self.resolve_target_user_id(&target, "mute")?;
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
        let target_user = self
            .storage
            .get_user(user_id)
            .map_err(ModerationError::Storage)?;

        let mut execution = ModerationExecution::new(dry_run);
        execution.telegram.push(telegram.clone());

        if command.command.flags.public_notice {
            self.require_capability(event, unit_policy, "tg.write_message")?;
            let mut vars = confirmation_vars(
                event,
                &target,
                target_user.as_ref(),
                command.expanded_reason.as_ref(),
            );
            vars.insert(
                "duration".to_owned(),
                format_duration(command.command.duration),
            );
            let notice = self
                .gateway
                .execute_checked(
                    TelegramRequest::SendMessage(TelegramSendMessageRequest {
                        chat_id,
                        text: crate::host_api::template::render_template(MUTE_TEMPLATE, vars),
                        reply_to_message_id: None,
                        silent: false,
                        parse_mode: crate::tg::ParseMode::Html,
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
            unit_policy,
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

    pub(crate) async fn execute_ban(
        &self,
        event: &EventContext,
        command: &ExpandedModerationCommand,
        dry_run: bool,
        unit_policy: Option<&ModerationUnitPolicy>,
    ) -> Result<ModerationExecution, ModerationError> {
        self.require_capability(event, unit_policy, "tg.moderate.ban")?;
        let target = describe_target(event, &command.command.target)?;
        let user_id = self.resolve_target_user_id(&target, "ban")?;
        let chat_id = require_chat_id(event)?;
        let reason = moderation_reason(command.expanded_reason.as_ref());
        let target_user = self
            .storage
            .get_user(user_id)
            .map_err(ModerationError::Storage)?;
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
        let confirmation = self
            .gateway
            .execute_checked(
                TelegramRequest::SendMessage(TelegramSendMessageRequest {
                    chat_id,
                    text: crate::host_api::template::render_template(
                        BAN_TEMPLATE,
                        confirmation_vars(
                            event,
                            &target,
                            target_user.as_ref(),
                            command.expanded_reason.as_ref(),
                        ),
                    ),
                    reply_to_message_id: None,
                    silent: false,
                    parse_mode: crate::tg::ParseMode::Html,
                    markup: None,
                }),
                TelegramExecutionOptions { dry_run },
            )
            .await
            .map_err(ModerationError::Telegram)?;
        execution.telegram.push(confirmation);
        let audit = self.build_audit_entry(
            event,
            unit_policy,
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

    pub(crate) async fn execute_delete(
        &self,
        event: &EventContext,
        command: &DeleteCommand,
        dry_run: bool,
        unit_policy: Option<&ModerationUnitPolicy>,
    ) -> Result<ModerationExecution, ModerationError> {
        self.require_capability(event, unit_policy, "tg.moderate.delete")?;
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
            unit_policy,
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

    pub(crate) async fn execute_undo(
        &self,
        event: &EventContext,
        dry_run: bool,
        unit_policy: Option<&ModerationUnitPolicy>,
    ) -> Result<ModerationExecution, ModerationError> {
        self.require_capability(event, unit_policy, "audit.compensate")?;
        let chat_id = require_chat_id(event)?;
        let original = if let Some(reference_message_id) = undo_reference_message_id(event) {
            self.storage
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
                })?
        } else {
            let actor_user_id = event
                .sender
                .as_ref()
                .map(|sender| sender.id)
                .ok_or_else(|| {
                    ModerationError::Validation("undo requires sender context".to_owned())
                })?;
            self.storage
                .find_audit_entries(
                    &AuditLogFilter {
                        chat_id: Some(chat_id),
                        actor_user_id: Some(actor_user_id),
                        reversible: Some(true),
                        ..AuditLogFilter::default()
                    },
                    20,
                )
                .map_err(ModerationError::Storage)?
                .into_iter()
                .find(|entry| entry.op != "undo")
                .ok_or_else(|| {
                    ModerationError::Validation(
                        "no recent reversible moderation action found for undo".to_owned(),
                    )
                })?
        };
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
        let target =
            execute_compensation(self, event, &recipe, dry_run, &mut execution, unit_policy)
                .await?;
        let target_user = target
            .user_id
            .map(|user_id| self.storage.get_user(user_id))
            .transpose()
            .map_err(ModerationError::Storage)?
            .flatten();
        let success = self
            .gateway
            .execute_checked(
                TelegramRequest::SendMessage(TelegramSendMessageRequest {
                    chat_id,
                    text: crate::host_api::template::render_template(
                        UNDO_SUCCESS_TEMPLATE,
                        undo_confirmation_vars(event, &target, target_user.as_ref(), &original.op),
                    ),
                    reply_to_message_id: None,
                    silent: false,
                    parse_mode: crate::tg::ParseMode::Html,
                    markup: None,
                }),
                TelegramExecutionOptions { dry_run },
            )
            .await
            .map_err(ModerationError::Telegram)?;
        execution.telegram.push(success);

        let audit = self.build_audit_entry(
            event,
            unit_policy,
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

    pub(crate) async fn execute_message(
        &self,
        event: &EventContext,
        command: &MessageCommand,
        dry_run: bool,
        unit_policy: Option<&ModerationUnitPolicy>,
    ) -> Result<ModerationExecution, ModerationError> {
        self.require_capability(event, unit_policy, "tg.write_message")?;
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

    fn resolve_target_user_id(
        &self,
        target: &ExecutionTarget,
        op: &str,
    ) -> Result<i64, ModerationError> {
        if let Some(user_id) = target.user_id {
            return Ok(user_id);
        }

        if let Some(username) = target.username.as_deref()
            && let Some(user) = self.storage.get_user_by_username(username)?
        {
            return Ok(user.user_id);
        }

        if let Some(username) = target.username.as_deref() {
            return Err(ModerationError::Validation(format!(
                "`/{op}` does not know @{username} yet; reply to one of that user's messages or wait until the bot sees a fresh message from them"
            )));
        }

        require_user_id(target, op)
    }

    pub(crate) async fn execute_ping(
        &self,
        event: &EventContext,
        dry_run: bool,
    ) -> Result<ModerationExecution, ModerationError> {
        let telegram = self
            .gateway
            .execute_checked(
                TelegramRequest::SendMessage(TelegramSendMessageRequest {
                    chat_id: require_chat_id(event)?,
                    text: "pong".to_owned(),
                    reply_to_message_id: None,
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

    pub(crate) async fn execute_help(
        &self,
        event: &EventContext,
        dry_run: bool,
    ) -> Result<ModerationExecution, ModerationError> {
        let telegram = self
            .gateway
            .execute_checked(
                TelegramRequest::SendMessage(TelegramSendMessageRequest {
                    chat_id: require_chat_id(event)?,
                    text: HELP_TEMPLATE.trim().to_owned(),
                    reply_to_message_id: None,
                    silent: false,
                    parse_mode: crate::tg::ParseMode::Html,
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

    pub(crate) fn schedule_pipe(
        &self,
        event: &EventContext,
        command: &ExpandedMuteCommand,
        pipe: &ExpandedCommandLine,
        dry_run: bool,
        unit_policy: Option<&ModerationUnitPolicy>,
    ) -> Result<JobRecord, ModerationError> {
        use uuid::Uuid;
        self.require_capability(event, unit_policy, "job.schedule")?;
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
        if dry_run {
            return Ok(job);
        }

        self.storage
            .insert_job(&job)
            .map_err(ModerationError::Storage)
    }
}

fn confirmation_vars(
    event: &EventContext,
    target: &ExecutionTarget,
    target_user: Option<&UserRecord>,
    reason: Option<&crate::parser::reason::ExpandedReason>,
) -> HashMap<String, String> {
    let mut vars = HashMap::new();
    let (date, time) = format_event_timestamp(event);
    vars.insert("admin_link".to_owned(), actor_link(event));
    vars.insert("user_link".to_owned(), target_link(target, target_user));
    vars.insert(
        "reason".to_owned(),
        reason
            .map(|value| escape_html(&reason_text(value)))
            .unwrap_or_else(|| "не указана".to_owned()),
    );
    vars.insert("date".to_owned(), date);
    vars.insert("time".to_owned(), time);
    vars.insert(
        "target".to_owned(),
        escape_html(&target_plain_label(target, target_user)),
    );
    vars
}

fn undo_confirmation_vars(
    event: &EventContext,
    target: &ExecutionTarget,
    target_user: Option<&UserRecord>,
    op: &str,
) -> HashMap<String, String> {
    let mut vars = confirmation_vars(event, target, target_user, None);
    vars.insert("op".to_owned(), escape_html(op));
    vars
}

fn actor_link(event: &EventContext) -> String {
    let Some(sender) = event.sender.as_ref() else {
        return "Администратор".to_owned();
    };
    mention_html(
        sender.id,
        sender.username.as_deref(),
        sender.display_name.as_deref(),
    )
}

fn target_link(target: &ExecutionTarget, target_user: Option<&UserRecord>) -> String {
    if let Some(user_id) = target.user_id.or(target_user.map(|user| user.user_id)) {
        return mention_html(
            user_id,
            target_user
                .and_then(|user| user.username.as_deref())
                .or(target.username.as_deref()),
            target_user.and_then(|user| user.display_name.as_deref()),
        );
    }

    escape_html(&target_plain_label(target, target_user))
}

fn target_plain_label(target: &ExecutionTarget, target_user: Option<&UserRecord>) -> String {
    if let Some(username) = target_user
        .and_then(|user| user.username.as_deref())
        .or(target.username.as_deref())
    {
        return format!("@{username}");
    }

    if let Some(display_name) = target_user.and_then(|user| user.display_name.as_deref()) {
        return display_name.to_owned();
    }

    target.label.clone()
}

fn mention_html(user_id: i64, username: Option<&str>, display_name: Option<&str>) -> String {
    let label = match (display_name, username) {
        (Some(display_name), Some(username)) => format!("{display_name} (@{username})"),
        (Some(display_name), None) => display_name.to_owned(),
        (None, Some(username)) => format!("@{username}"),
        (None, None) => user_id.to_string(),
    };
    format!(
        "<a href=\"tg://user?id={user_id}\">{}</a>",
        escape_html(&label)
    )
}

fn format_event_timestamp(event: &EventContext) -> (String, String) {
    (
        event.received_at.format("%Y-%m-%d").to_string(),
        event.received_at.format("%H:%M").to_string(),
    )
}

fn format_duration(duration: crate::parser::duration::ParsedDuration) -> String {
    match duration.unit {
        crate::parser::duration::DurationUnit::Seconds => format!("{} сек.", duration.value),
        crate::parser::duration::DurationUnit::Minutes => format!("{} мин.", duration.value),
        crate::parser::duration::DurationUnit::Hours => format!("{} ч.", duration.value),
        crate::parser::duration::DurationUnit::Days => format!("{} д.", duration.value),
        crate::parser::duration::DurationUnit::Weeks => format!("{} нед.", duration.value),
    }
}

fn escape_html(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
