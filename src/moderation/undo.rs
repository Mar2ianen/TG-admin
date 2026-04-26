use super::ModerationEngine;
use super::types::{
    CompensationRecipe, ExecutionTarget, ModerationError, ModerationExecution, ModerationUnitPolicy,
};
use crate::event::EventContext;
use crate::storage::UserPatch;
use crate::tg::{
    TelegramExecutionOptions, TelegramRequest, TelegramUnbanRequest, TelegramUnrestrictRequest,
};

pub async fn execute_compensation(
    engine: &ModerationEngine,
    event: &EventContext,
    recipe: &CompensationRecipe,
    dry_run: bool,
    execution: &mut ModerationExecution,
    unit_policy: Option<&ModerationUnitPolicy>,
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
            engine.require_capability(event, unit_policy, "tg.moderate.restrict")?;
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
            engine.require_capability(event, unit_policy, "tg.moderate.ban")?;
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
