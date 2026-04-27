use crate::moderation::ModerationError;
use serde_json::{Value, json};

pub fn error_to_template(err: &ModerationError) -> (&'static str, Value) {
    match err {
        ModerationError::AuthorizationDenied { .. } => ("moderation/access_denied", json!({})),
        ModerationError::BotPermissionDenied => ("moderation/bot_not_admin", json!({})),
        ModerationError::TargetProtected { target_name } => (
            "moderation/protected_target",
            json!({ "target_name": target_name }),
        ),
        _ => ("moderation/generic_error", json!({})),
    }
}
