use crate::event::UnitContext;
use crate::parser::dispatch::{CommandDispatchParseError, CommandDispatchSkip};
use crate::storage::{AuditLogEntry, JobRecord, ProcessedUpdateRecord, StorageError};
use crate::tg::{MessageId, TelegramExecution};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

#[derive(Debug, Clone)]
pub struct ModerationUnitPolicy {
    pub unit: UnitContext,
}

impl ModerationUnitPolicy {
    pub fn new(unit: UnitContext) -> Self {
        Self { unit }
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
    pub fn new(dry_run: bool) -> Self {
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
pub enum CompensationRecipe {
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

pub(crate) struct AuditEntrySpec<'a> {
    pub op: &'a str,
    pub target: &'a ExecutionTarget,
    pub reversible: bool,
    pub compensation: Option<CompensationRecipe>,
    pub args_json: Value,
    pub result_json: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionTarget {
    pub kind: String,
    pub id: String,
    pub user_id: Option<i64>,
    pub username: Option<String>,
    pub label: String,
}

impl ExecutionTarget {
    pub fn message_anchor(message_id: MessageId) -> Self {
        Self {
            kind: "message".to_owned(),
            id: message_id.to_string(),
            user_id: None,
            username: None,
            label: format!("message:{message_id}"),
        }
    }

    pub fn audit_target_json(&self) -> Value {
        serde_json::json!({
            "kind": self.kind,
            "id": self.id,
            "user_id": self.user_id,
            "username": self.username,
            "label": self.label,
        })
    }
}
