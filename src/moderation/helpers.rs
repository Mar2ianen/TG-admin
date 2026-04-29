use super::types::{ExecutionTarget, ModerationError};
use crate::event::{EventContext, ExecutionMode};
use crate::parser::command::CommandAst;
use crate::parser::duration::ParsedDuration;
use crate::parser::reason::ExpandedReason;
use crate::parser::target::{ParsedTargetSelector, ResolvedTarget, TargetSource};
use crate::tg::{ModerationReason, TelegramPermissions};
use chrono::{DateTime, Utc};
use serde_json::Value;

pub fn execution_mode_name(mode: ExecutionMode) -> &'static str {
    match mode {
        ExecutionMode::Realtime => "realtime",
        ExecutionMode::Manual => "manual",
        ExecutionMode::Scheduled => "scheduled",
        ExecutionMode::Recovery => "recovery",
    }
}

pub fn command_dry_run(command: &CommandAst) -> bool {
    match command {
        CommandAst::Warn(command) | CommandAst::Ban(command) => command.flags.dry_run,
        CommandAst::Mute(command) => command.flags.dry_run,
        CommandAst::Del(command) => command.flags.dry_run,
        CommandAst::Undo(command) => command.dry_run,
        CommandAst::Msg(_) => false,
        CommandAst::Help(_) => false,
        CommandAst::Ping(_) => false,
    }
}

pub fn add_duration(
    received_at: DateTime<Utc>,
    duration: ParsedDuration,
) -> Result<DateTime<Utc>, ModerationError> {
    let chrono_duration = chrono::Duration::from_std(duration.into_std())
        .map_err(|error| ModerationError::Validation(format!("duration overflow: {error}")))?;

    received_at
        .checked_add_signed(chrono_duration)
        .ok_or_else(|| {
            ModerationError::Validation("mute duration exceeds supported range".to_owned())
        })
}

pub fn muted_permissions() -> TelegramPermissions {
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

pub fn reason_text(reason: &ExpandedReason) -> String {
    match reason {
        ExpandedReason::RuleCode { code } => code.clone(),
        ExpandedReason::Alias { definition, .. } => definition.canonical.clone(),
        ExpandedReason::UnknownAlias { alias } => alias.clone(),
        ExpandedReason::Quoted { text } | ExpandedReason::FreeText { text } => text.clone(),
    }
}

pub fn reason_value(reason: Option<&ExpandedReason>) -> Value {
    reason
        .and_then(|value| serde_json::to_value(value).ok())
        .unwrap_or(Value::Null)
}

pub fn moderation_reason(reason: Option<&ExpandedReason>) -> Option<ModerationReason> {
    reason.map(|reason| ModerationReason {
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

pub fn build_notice_text(action: &str, target: &str, reason: Option<&ExpandedReason>) -> String {
    let reason = reason
        .map(reason_text)
        .unwrap_or_else(|| "without a recorded reason".to_owned());
    format!("{action} {target}: {reason}")
}

pub fn undo_reference_message_id(event: &EventContext) -> Option<i32> {
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
}

pub fn trigger_message_id(event: &EventContext) -> Option<i32> {
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

pub fn require_user_id(target: &ExecutionTarget, op: &str) -> Result<i64, ModerationError> {
    target.user_id.ok_or_else(|| {
        ModerationError::Validation(format!(
            "`/{op}` requires a numeric target user id or reply context"
        ))
    })
}

pub fn require_chat_id(event: &EventContext) -> Result<i64, ModerationError> {
    event
        .chat
        .as_ref()
        .map(|chat| chat.id)
        .ok_or_else(|| ModerationError::Validation("chat context is required".to_owned()))
}

pub fn resolve_numeric_user_filter(
    selector: &ParsedTargetSelector,
) -> Result<i64, ModerationError> {
    match selector {
        ParsedTargetSelector::UserId { user_id } => Ok(*user_id),
        _ => Err(ModerationError::Validation(
            "delete user filter must resolve to numeric user_id".to_owned(),
        )),
    }
}

pub fn delete_anchor(
    event: &EventContext,
    target: &ResolvedTarget,
) -> Result<(i32, Option<i64>), ModerationError> {
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

pub fn describe_target(
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
                    .or_else(|| user_id.map(|value: i64| value.to_string()))
                    .unwrap_or_else(|| raw.to_string()),
            })
        }
    }
}

pub fn hash_text(input: &str) -> u64 {
    use std::hash::{Hash, Hasher};

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    input.hash(&mut hasher);
    hasher.finish()
}

pub fn render_template(template: &str, bindings: &[(&str, &str)]) -> String {
    let mut rendered = template.trim().to_owned();
    for (key, value) in bindings {
        rendered = rendered.replace(&format!("{{{key}}}"), value);
    }
    rendered
}
