use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub type ChatId = i64;
pub type UserId = i64;
pub type MessageId = i32;

#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParseMode {
    #[default]
    PlainText,
    MarkdownV2,
    Html,
}

#[derive(Debug, Clone, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct TelegramUiMarkup {
    pub inline_keyboard: Vec<Vec<TelegramUiButton>>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct TelegramUiButton {
    pub text: String,
    pub callback_data: Option<String>,
    pub url: Option<String>,
}

#[derive(Debug, Clone, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct TelegramPermissions {
    pub can_send_messages: Option<bool>,
    pub can_send_audios: Option<bool>,
    pub can_send_documents: Option<bool>,
    pub can_send_photos: Option<bool>,
    pub can_send_videos: Option<bool>,
    pub can_send_video_notes: Option<bool>,
    pub can_send_voice_notes: Option<bool>,
    pub can_send_polls: Option<bool>,
    pub can_send_other_messages: Option<bool>,
    pub can_add_web_page_previews: Option<bool>,
    pub can_change_info: Option<bool>,
    pub can_invite_users: Option<bool>,
    pub can_pin_messages: Option<bool>,
    pub can_manage_topics: Option<bool>,
}

#[derive(Debug, Clone, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ModerationReason {
    pub code: Option<String>,
    pub text: Option<String>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op")]
pub enum TelegramRequest {
    #[serde(rename = "tg.send_ui")]
    SendUi(TelegramSendUiRequest),
    #[serde(rename = "tg.send_message")]
    SendMessage(TelegramSendMessageRequest),
    #[serde(rename = "tg.edit_ui")]
    EditUi(TelegramEditUiRequest),
    #[serde(rename = "tg.delete")]
    Delete(TelegramDeleteRequest),
    #[serde(rename = "tg.delete_many")]
    DeleteMany(TelegramDeleteManyRequest),
    #[serde(rename = "tg.restrict")]
    Restrict(TelegramRestrictRequest),
    #[serde(rename = "tg.unrestrict")]
    Unrestrict(TelegramUnrestrictRequest),
    #[serde(rename = "tg.ban")]
    Ban(TelegramBanRequest),
    #[serde(rename = "tg.unban")]
    Unban(TelegramUnbanRequest),
    #[serde(rename = "tg.answer_callback")]
    AnswerCallback(TelegramAnswerCallbackRequest),
}

impl TelegramRequest {
    pub fn operation(&self) -> TelegramOperation {
        match self {
            Self::SendUi(_) => TelegramOperation::SendUi,
            Self::SendMessage(_) => TelegramOperation::SendMessage,
            Self::EditUi(_) => TelegramOperation::EditUi,
            Self::Delete(_) => TelegramOperation::Delete,
            Self::DeleteMany(_) => TelegramOperation::DeleteMany,
            Self::Restrict(_) => TelegramOperation::Restrict,
            Self::Unrestrict(_) => TelegramOperation::Unrestrict,
            Self::Ban(_) => TelegramOperation::Ban,
            Self::Unban(_) => TelegramOperation::Unban,
            Self::AnswerCallback(_) => TelegramOperation::AnswerCallback,
        }
    }

    pub fn idempotency_key(&self) -> Option<&str> {
        match self {
            Self::Delete(request) => request.idempotency_key.as_deref(),
            Self::DeleteMany(request) => request.idempotency_key.as_deref(),
            Self::Restrict(request) => request.idempotency_key.as_deref(),
            Self::Unrestrict(request) => request.idempotency_key.as_deref(),
            Self::Ban(request) => request.idempotency_key.as_deref(),
            Self::Unban(request) => request.idempotency_key.as_deref(),
            Self::SendUi(_) | Self::SendMessage(_) | Self::EditUi(_) | Self::AnswerCallback(_) => {
                None
            }
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
pub enum TelegramOperation {
    SendUi,
    SendMessage,
    EditUi,
    Delete,
    DeleteMany,
    Restrict,
    Unrestrict,
    Ban,
    Unban,
    AnswerCallback,
}

impl TelegramOperation {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SendUi => "tg.send_ui",
            Self::SendMessage => "tg.send_message",
            Self::EditUi => "tg.edit_ui",
            Self::Delete => "tg.delete",
            Self::DeleteMany => "tg.delete_many",
            Self::Restrict => "tg.restrict",
            Self::Unrestrict => "tg.unrestrict",
            Self::Ban => "tg.ban",
            Self::Unban => "tg.unban",
            Self::AnswerCallback => "tg.answer_callback",
        }
    }

    pub fn requires_idempotency(self) -> bool {
        matches!(
            self,
            Self::Delete
                | Self::DeleteMany
                | Self::Restrict
                | Self::Unrestrict
                | Self::Ban
                | Self::Unban
        )
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct TelegramSendUiRequest {
    pub chat_id: ChatId,
    pub template: String,
    #[serde(default)]
    pub data: Value,
    pub reply_to_message_id: Option<MessageId>,
    #[serde(default)]
    pub silent: bool,
    #[serde(default)]
    pub parse_mode: ParseMode,
    pub markup: Option<TelegramUiMarkup>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct TelegramSendMessageRequest {
    pub chat_id: ChatId,
    pub text: String,
    pub reply_to_message_id: Option<MessageId>,
    #[serde(default)]
    pub silent: bool,
    #[serde(default)]
    pub parse_mode: ParseMode,
    pub markup: Option<TelegramUiMarkup>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct TelegramEditUiRequest {
    pub chat_id: ChatId,
    pub message_id: MessageId,
    pub template: String,
    #[serde(default)]
    pub data: Value,
    #[serde(default)]
    pub parse_mode: ParseMode,
    pub markup: Option<TelegramUiMarkup>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct TelegramDeleteRequest {
    pub chat_id: ChatId,
    pub message_id: MessageId,
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct TelegramDeleteManyRequest {
    pub chat_id: ChatId,
    pub message_ids: Vec<MessageId>,
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct TelegramRestrictRequest {
    pub chat_id: ChatId,
    pub user_id: UserId,
    #[serde(default)]
    pub permissions: TelegramPermissions,
    pub until: Option<DateTime<Utc>>,
    pub reason: Option<ModerationReason>,
    #[serde(default)]
    pub silent: bool,
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct TelegramUnrestrictRequest {
    pub chat_id: ChatId,
    pub user_id: UserId,
    pub reason: Option<ModerationReason>,
    #[serde(default)]
    pub silent: bool,
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct TelegramBanRequest {
    pub chat_id: ChatId,
    pub user_id: UserId,
    pub until: Option<DateTime<Utc>>,
    #[serde(default)]
    pub delete_history: bool,
    pub reason: Option<ModerationReason>,
    #[serde(default)]
    pub silent: bool,
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct TelegramUnbanRequest {
    pub chat_id: ChatId,
    pub user_id: UserId,
    #[serde(default)]
    pub only_if_banned: bool,
    pub reason: Option<ModerationReason>,
    #[serde(default)]
    pub silent: bool,
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct TelegramAnswerCallbackRequest {
    pub callback_query_id: String,
    pub text: Option<String>,
    #[serde(default)]
    pub show_alert: bool,
    #[serde(default)]
    pub cache_time_seconds: u32,
    pub url: Option<String>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TelegramResult {
    Message(TelegramMessageResult),
    Ui(TelegramUiResult),
    Delete(TelegramDeleteResult),
    Restriction(TelegramRestrictionResult),
    Ban(TelegramBanResult),
    Callback(TelegramCallbackResult),
}

impl TelegramResult {
    pub fn operation_kind(&self) -> TelegramResultKind {
        match self {
            Self::Message(_) => TelegramResultKind::Message,
            Self::Ui(_) => TelegramResultKind::Ui,
            Self::Delete(_) => TelegramResultKind::Delete,
            Self::Restriction(_) => TelegramResultKind::Restriction,
            Self::Ban(_) => TelegramResultKind::Ban,
            Self::Callback(_) => TelegramResultKind::Callback,
        }
    }

    pub fn chat_id(&self) -> Option<ChatId> {
        match self {
            Self::Message(result) => Some(result.chat_id),
            Self::Ui(result) => Some(result.chat_id),
            Self::Delete(result) => Some(result.chat_id),
            Self::Restriction(result) => Some(result.chat_id),
            Self::Ban(result) => Some(result.chat_id),
            Self::Callback(_) => None,
        }
    }

    pub fn message_id(&self) -> Option<MessageId> {
        match self {
            Self::Message(result) => Some(result.message_id),
            Self::Ui(result) => Some(result.message_id),
            Self::Delete(_) | Self::Restriction(_) | Self::Ban(_) | Self::Callback(_) => None,
        }
    }

    pub fn user_id(&self) -> Option<UserId> {
        match self {
            Self::Restriction(result) => Some(result.user_id),
            Self::Ban(result) => Some(result.user_id),
            Self::Message(_) | Self::Ui(_) | Self::Delete(_) | Self::Callback(_) => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TelegramResultKind {
    Message,
    Ui,
    Delete,
    Restriction,
    Ban,
    Callback,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct TelegramExecution {
    pub result: TelegramResult,
    pub metadata: TelegramExecutionMetadata,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct TelegramExecutionMetadata {
    pub operation: TelegramOperation,
    #[serde(default)]
    pub dry_run: bool,
    #[serde(default)]
    pub replayed: bool,
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct TelegramExecutionOptions {
    #[serde(default)]
    pub dry_run: bool,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct TelegramMessageResult {
    pub chat_id: ChatId,
    pub message_id: MessageId,
    #[serde(default)]
    pub raw_passthrough: bool,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct TelegramUiResult {
    pub chat_id: ChatId,
    pub message_id: MessageId,
    pub template: String,
    #[serde(default)]
    pub edited: bool,
    #[serde(default)]
    pub raw_passthrough: bool,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct TelegramDeleteResult {
    pub chat_id: ChatId,
    pub deleted: Vec<MessageId>,
    #[serde(default)]
    pub failed: Vec<MessageId>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct TelegramRestrictionResult {
    pub chat_id: ChatId,
    pub user_id: UserId,
    pub until: Option<DateTime<Utc>>,
    #[serde(default)]
    pub permissions: TelegramPermissions,
    #[serde(default)]
    pub changed: bool,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct TelegramBanResult {
    pub chat_id: ChatId,
    pub user_id: UserId,
    pub until: Option<DateTime<Utc>>,
    #[serde(default)]
    pub delete_history: bool,
    #[serde(default)]
    pub changed: bool,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct TelegramCallbackResult {
    pub callback_query_id: String,
    #[serde(default)]
    pub answered: bool,
    #[serde(default)]
    pub show_alert: bool,
    pub text: Option<String>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct TelegramError {
    pub operation: TelegramOperation,
    pub kind: TelegramErrorKind,
    pub message: String,
    #[serde(default)]
    pub retryable: bool,
    pub details: Option<Value>,
}

impl TelegramError {
    pub fn new(
        operation: TelegramOperation,
        kind: TelegramErrorKind,
        message: impl Into<String>,
    ) -> Self {
        Self {
            operation,
            kind,
            message: message.into(),
            retryable: false,
            details: None,
        }
    }

    pub fn with_retryable(mut self, retryable: bool) -> Self {
        self.retryable = retryable;
        self
    }

    pub fn with_details(mut self, details: Value) -> Self {
        self.details = Some(details);
        self
    }

    pub fn transport_unavailable(operation: TelegramOperation, message: impl Into<String>) -> Self {
        Self::new(operation, TelegramErrorKind::TransportUnavailable, message)
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TelegramErrorKind {
    Validation,
    Denied,
    NotFound,
    Conflict,
    PermissionDenied,
    RateLimited,
    TransportUnavailable,
    UnsupportedOperation,
    Internal,
}
