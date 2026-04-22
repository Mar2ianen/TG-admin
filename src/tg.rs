use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use teloxide_core::payloads::{
    AnswerCallbackQuerySetters, BanChatMemberSetters, RestrictChatMemberSetters,
    SendMessageSetters, UnbanChatMemberSetters,
};
use teloxide_core::errors::{ApiError as TeloxideApiError, RequestError as TeloxideRequestError};
use teloxide_core::prelude::{Request, Requester};
use teloxide_core::types::{
    CallbackQueryId as TeloxideCallbackQueryId, ChatId as TeloxideChatId,
    ChatPermissions as TeloxideChatPermissions,
    InlineKeyboardButton as TeloxideInlineKeyboardButton,
    InlineKeyboardMarkup as TeloxideInlineKeyboardMarkup, MessageId as TeloxideMessageId,
    ParseMode as TeloxideParseMode, ReplyMarkup as TeloxideReplyMarkup,
    ReplyParameters as TeloxideReplyParameters, UserId as TeloxideUserId,
};

pub type ChatId = i64;
pub type UserId = i64;
pub type MessageId = i32;

#[derive(Clone)]
pub struct TelegramGateway {
    polling: bool,
    transport: Arc<dyn TelegramTransport>,
    idempotency_cache: Arc<Mutex<HashMap<String, TelegramResult>>>,
}

impl TelegramGateway {
    pub fn new(polling: bool) -> Self {
        Self {
            polling,
            transport: Arc::new(NoopTelegramTransport),
            idempotency_cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn with_transport<T>(mut self, transport: T) -> Self
    where
        T: TelegramTransport + 'static,
    {
        self.transport = Arc::new(transport);
        self
    }

    pub fn polling(&self) -> bool {
        self.polling
    }

    pub fn transport_name(&self) -> &'static str {
        self.transport.name()
    }

    pub async fn execute(&self, request: TelegramRequest) -> Result<TelegramResult, TelegramError> {
        self.transport.execute(request).await
    }

    pub async fn execute_checked(
        &self,
        request: TelegramRequest,
        options: TelegramExecutionOptions,
    ) -> Result<TelegramExecution, TelegramError> {
        validate_request(&request)?;

        let operation = request.operation();
        let idempotency_key = request.idempotency_key().map(ToOwned::to_owned);

        if options.dry_run {
            return Ok(TelegramExecution {
                result: predict_result(&request),
                metadata: TelegramExecutionMetadata {
                    operation,
                    dry_run: true,
                    replayed: false,
                    idempotency_key,
                },
            });
        }

        if let Some(key) = request.idempotency_key() {
            if let Some(cached) = self
                .idempotency_cache
                .lock()
                .expect("telegram idempotency cache lock poisoned")
                .get(key)
                .cloned()
            {
                return Ok(TelegramExecution {
                    result: cached,
                    metadata: TelegramExecutionMetadata {
                        operation,
                        dry_run: false,
                        replayed: true,
                        idempotency_key,
                    },
                });
            }
        }

        let result = self.transport.execute(request).await?;

        if let Some(key) = idempotency_key.clone() {
            self.idempotency_cache
                .lock()
                .expect("telegram idempotency cache lock poisoned")
                .insert(key, result.clone());
        }

        Ok(TelegramExecution {
            result,
            metadata: TelegramExecutionMetadata {
                operation,
                dry_run: false,
                replayed: false,
                idempotency_key,
            },
        })
    }
}

impl fmt::Debug for TelegramGateway {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TelegramGateway")
            .field("polling", &self.polling)
            .field("transport", &self.transport.name())
            .finish()
    }
}

impl Default for TelegramGateway {
    fn default() -> Self {
        Self::new(true)
    }
}

#[async_trait]
pub trait TelegramTransport: Send + Sync {
    fn name(&self) -> &'static str {
        "custom"
    }

    async fn execute(&self, request: TelegramRequest) -> Result<TelegramResult, TelegramError>;
}

#[derive(Debug, Default)]
pub struct NoopTelegramTransport;

#[async_trait]
impl TelegramTransport for NoopTelegramTransport {
    fn name(&self) -> &'static str {
        "noop"
    }

    async fn execute(&self, request: TelegramRequest) -> Result<TelegramResult, TelegramError> {
        Err(TelegramError::transport_unavailable(
            request.operation(),
            "telegram transport is not configured",
        ))
    }
}

#[derive(Debug, Clone)]
pub struct TeloxideCoreTransport {
    bot: teloxide_core::Bot,
}

impl TeloxideCoreTransport {
    pub fn new(token: impl Into<String>) -> Self {
        Self {
            bot: teloxide_core::Bot::new(token.into()),
        }
    }
}

#[async_trait]
impl TelegramTransport for TeloxideCoreTransport {
    fn name(&self) -> &'static str {
        "teloxide-core"
    }

    async fn execute(&self, request: TelegramRequest) -> Result<TelegramResult, TelegramError> {
        let operation = request.operation();

        match request {
            TelegramRequest::SendMessage(request) => {
                let mut api = self
                    .bot
                    .send_message(TeloxideChatId(request.chat_id), request.text.clone());
                if let Some(parse_mode) = to_teloxide_parse_mode(request.parse_mode) {
                    api = api.parse_mode(parse_mode);
                }
                if request.silent {
                    api = api.disable_notification(true);
                }
                if let Some(reply_to_message_id) = request.reply_to_message_id {
                    api = api.reply_parameters(TeloxideReplyParameters::new(
                        TeloxideMessageId(reply_to_message_id),
                    ));
                }
                if let Some(markup) = request.markup.as_ref() {
                    api = api.reply_markup(to_teloxide_reply_markup(markup)?);
                }

                let message = api.send().await.map_err(|error| {
                    map_teloxide_error(operation, error).with_details(serde_json::json!({
                        "chat_id": request.chat_id,
                    }))
                })?;

                Ok(TelegramResult::Message(TelegramMessageResult {
                    chat_id: message.chat.id.0,
                    message_id: message.id.0,
                    raw_passthrough: true,
                }))
            }
            TelegramRequest::Delete(request) => {
                self.bot
                    .delete_message(
                        TeloxideChatId(request.chat_id),
                        TeloxideMessageId(request.message_id),
                    )
                    .send()
                    .await
                    .map_err(|error| {
                        map_teloxide_error(operation, error).with_details(serde_json::json!({
                            "chat_id": request.chat_id,
                            "message_id": request.message_id,
                        }))
                    })?;

                Ok(TelegramResult::Delete(TelegramDeleteResult {
                    chat_id: request.chat_id,
                    deleted: vec![request.message_id],
                    failed: Vec::new(),
                }))
            }
            TelegramRequest::DeleteMany(request) => {
                let message_ids = request
                    .message_ids
                    .iter()
                    .copied()
                    .map(TeloxideMessageId)
                    .collect::<Vec<_>>();
                self.bot
                    .delete_messages(TeloxideChatId(request.chat_id), message_ids)
                    .send()
                    .await
                    .map_err(|error| {
                        map_teloxide_error(operation, error).with_details(serde_json::json!({
                            "chat_id": request.chat_id,
                            "message_ids": request.message_ids,
                        }))
                    })?;

                Ok(TelegramResult::Delete(TelegramDeleteResult {
                    chat_id: request.chat_id,
                    deleted: request.message_ids,
                    failed: Vec::new(),
                }))
            }
            TelegramRequest::Restrict(request) => {
                let mut api = self.bot.restrict_chat_member(
                    TeloxideChatId(request.chat_id),
                    TeloxideUserId(request.user_id as u64),
                    to_teloxide_permissions(&request.permissions),
                );
                if let Some(until) = request.until {
                    api = api.until_date(until);
                }

                api.send().await.map_err(|error| {
                    map_teloxide_error(operation, error).with_details(serde_json::json!({
                        "chat_id": request.chat_id,
                        "user_id": request.user_id,
                    }))
                })?;

                Ok(TelegramResult::Restriction(TelegramRestrictionResult {
                    chat_id: request.chat_id,
                    user_id: request.user_id,
                    until: request.until,
                    permissions: request.permissions,
                    changed: true,
                }))
            }
            TelegramRequest::Unrestrict(request) => {
                self.bot
                    .restrict_chat_member(
                        TeloxideChatId(request.chat_id),
                        TeloxideUserId(request.user_id as u64),
                        TeloxideChatPermissions::all(),
                    )
                    .send()
                    .await
                    .map_err(|error| {
                        map_teloxide_error(operation, error).with_details(serde_json::json!({
                            "chat_id": request.chat_id,
                            "user_id": request.user_id,
                        }))
                    })?;

                Ok(TelegramResult::Restriction(TelegramRestrictionResult {
                    chat_id: request.chat_id,
                    user_id: request.user_id,
                    until: None,
                    permissions: TelegramPermissions::default(),
                    changed: true,
                }))
            }
            TelegramRequest::Ban(request) => {
                let mut api = self.bot.ban_chat_member(
                    TeloxideChatId(request.chat_id),
                    TeloxideUserId(request.user_id as u64),
                );
                if let Some(until) = request.until {
                    api = api.until_date(until);
                }
                if request.delete_history {
                    api = api.revoke_messages(true);
                }

                api.send().await.map_err(|error| {
                    map_teloxide_error(operation, error).with_details(serde_json::json!({
                        "chat_id": request.chat_id,
                        "user_id": request.user_id,
                    }))
                })?;

                Ok(TelegramResult::Ban(TelegramBanResult {
                    chat_id: request.chat_id,
                    user_id: request.user_id,
                    until: request.until,
                    delete_history: request.delete_history,
                    changed: true,
                }))
            }
            TelegramRequest::Unban(request) => {
                let mut api = self.bot.unban_chat_member(
                    TeloxideChatId(request.chat_id),
                    TeloxideUserId(request.user_id as u64),
                );
                if request.only_if_banned {
                    api = api.only_if_banned(true);
                }

                api.send().await.map_err(|error| {
                    map_teloxide_error(operation, error).with_details(serde_json::json!({
                        "chat_id": request.chat_id,
                        "user_id": request.user_id,
                        "only_if_banned": request.only_if_banned,
                    }))
                })?;

                Ok(TelegramResult::Ban(TelegramBanResult {
                    chat_id: request.chat_id,
                    user_id: request.user_id,
                    until: None,
                    delete_history: false,
                    changed: true,
                }))
            }
            TelegramRequest::AnswerCallback(request) => {
                let mut api = self.bot.answer_callback_query(TeloxideCallbackQueryId(
                    request.callback_query_id.clone(),
                ));
                if let Some(text) = request.text.clone() {
                    api = api.text(text);
                }
                if request.show_alert {
                    api = api.show_alert(true);
                }
                if request.cache_time_seconds > 0 {
                    api = api.cache_time(request.cache_time_seconds);
                }
                if let Some(url) = request.url.as_deref() {
                    let parsed = reqwest::Url::parse(url).map_err(|error| {
                        validation_error(operation, "url", "callback url must be valid").with_details(
                            serde_json::json!({
                                "source": error.to_string(),
                            }),
                        )
                    })?;
                    api = api.url(parsed);
                }

                api.send().await.map_err(|error| {
                    map_teloxide_error(operation, error).with_details(serde_json::json!({
                        "callback_query_id": request.callback_query_id,
                    }))
                })?;

                Ok(TelegramResult::Callback(TelegramCallbackResult {
                    callback_query_id: request.callback_query_id,
                    answered: true,
                    show_alert: request.show_alert,
                    text: request.text,
                }))
            }
            TelegramRequest::SendUi(_) | TelegramRequest::EditUi(_) => Err(TelegramError::new(
                operation,
                TelegramErrorKind::UnsupportedOperation,
                "teloxide-core transport does not support UI template operations yet",
            )),
        }
    }
}

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

fn validate_request(request: &TelegramRequest) -> Result<(), TelegramError> {
    let operation = request.operation();
    if operation.requires_idempotency() && request.idempotency_key().is_none() {
        return Err(
            TelegramError::new(
                operation,
                TelegramErrorKind::Validation,
                "idempotency key is required for this operation",
            )
            .with_details(serde_json::json!({
                "field": "idempotency_key",
            })),
        );
    }

    match request {
        TelegramRequest::SendUi(request) => {
            if request.template.trim().is_empty() {
                return Err(validation_error(operation, "template", "template must not be empty"));
            }
        }
        TelegramRequest::SendMessage(request) => {
            if request.text.trim().is_empty() {
                return Err(validation_error(operation, "text", "text must not be empty"));
            }
        }
        TelegramRequest::EditUi(request) => {
            if request.template.trim().is_empty() {
                return Err(validation_error(operation, "template", "template must not be empty"));
            }
            if request.message_id <= 0 {
                return Err(validation_error(
                    operation,
                    "message_id",
                    "message_id must be positive",
                ));
            }
        }
        TelegramRequest::Delete(request) => {
            if request.message_id <= 0 {
                return Err(validation_error(
                    operation,
                    "message_id",
                    "message_id must be positive",
                ));
            }
        }
        TelegramRequest::DeleteMany(request) => {
            if request.message_ids.is_empty() {
                return Err(validation_error(
                    operation,
                    "message_ids",
                    "message_ids must not be empty",
                ));
            }
            if request.message_ids.iter().any(|message_id| *message_id <= 0) {
                return Err(validation_error(
                    operation,
                    "message_ids",
                    "message_ids must contain only positive ids",
                ));
            }
        }
        TelegramRequest::Restrict(request) => {
            if request.user_id <= 0 {
                return Err(validation_error(operation, "user_id", "user_id must be positive"));
            }
        }
        TelegramRequest::Unrestrict(request) => {
            if request.user_id <= 0 {
                return Err(validation_error(operation, "user_id", "user_id must be positive"));
            }
        }
        TelegramRequest::Ban(request) => {
            if request.user_id <= 0 {
                return Err(validation_error(operation, "user_id", "user_id must be positive"));
            }
        }
        TelegramRequest::Unban(request) => {
            if request.user_id <= 0 {
                return Err(validation_error(operation, "user_id", "user_id must be positive"));
            }
        }
        TelegramRequest::AnswerCallback(request) => {
            if request.callback_query_id.trim().is_empty() {
                return Err(validation_error(
                    operation,
                    "callback_query_id",
                    "callback_query_id must not be empty",
                ));
            }
        }
    }

    Ok(())
}

fn validation_error(
    operation: TelegramOperation,
    field: &'static str,
    message: &'static str,
) -> TelegramError {
    TelegramError::new(operation, TelegramErrorKind::Validation, message).with_details(
        serde_json::json!({
            "field": field,
        }),
    )
}

fn to_teloxide_parse_mode(parse_mode: ParseMode) -> Option<TeloxideParseMode> {
    match parse_mode {
        ParseMode::PlainText => None,
        ParseMode::MarkdownV2 => Some(TeloxideParseMode::MarkdownV2),
        ParseMode::Html => Some(TeloxideParseMode::Html),
    }
}

fn to_teloxide_permissions(permissions: &TelegramPermissions) -> TeloxideChatPermissions {
    let mut mapped = TeloxideChatPermissions::empty();

    if permissions.can_send_messages.unwrap_or(false) {
        mapped |= TeloxideChatPermissions::SEND_MESSAGES;
    }
    if permissions.can_send_audios.unwrap_or(false) {
        mapped |= TeloxideChatPermissions::SEND_AUDIOS;
    }
    if permissions.can_send_documents.unwrap_or(false) {
        mapped |= TeloxideChatPermissions::SEND_DOCUMENTS;
    }
    if permissions.can_send_photos.unwrap_or(false) {
        mapped |= TeloxideChatPermissions::SEND_PHOTOS;
    }
    if permissions.can_send_videos.unwrap_or(false) {
        mapped |= TeloxideChatPermissions::SEND_VIDEOS;
    }
    if permissions.can_send_video_notes.unwrap_or(false) {
        mapped |= TeloxideChatPermissions::SEND_VIDEO_NOTES;
    }
    if permissions.can_send_voice_notes.unwrap_or(false) {
        mapped |= TeloxideChatPermissions::SEND_VOICE_NOTES;
    }
    if permissions.can_send_polls.unwrap_or(false) {
        mapped |= TeloxideChatPermissions::SEND_POLLS;
    }
    if permissions.can_send_other_messages.unwrap_or(false) {
        mapped |= TeloxideChatPermissions::SEND_OTHER_MESSAGES;
    }
    if permissions.can_add_web_page_previews.unwrap_or(false) {
        mapped |= TeloxideChatPermissions::ADD_WEB_PAGE_PREVIEWS;
    }
    if permissions.can_change_info.unwrap_or(false) {
        mapped |= TeloxideChatPermissions::CHANGE_INFO;
    }
    if permissions.can_invite_users.unwrap_or(false) {
        mapped |= TeloxideChatPermissions::INVITE_USERS;
    }
    if permissions.can_pin_messages.unwrap_or(false) {
        mapped |= TeloxideChatPermissions::PIN_MESSAGES;
    }
    if permissions.can_manage_topics.unwrap_or(false) {
        mapped |= TeloxideChatPermissions::MANAGE_TOPICS;
    }

    mapped
}

fn to_teloxide_reply_markup(markup: &TelegramUiMarkup) -> Result<TeloxideReplyMarkup, TelegramError> {
    let inline_keyboard = markup
        .inline_keyboard
        .iter()
        .map(|row| {
            row.iter()
                .map(|button| {
                    if let Some(url) = button.url.as_deref() {
                        reqwest::Url::parse(url)
                            .map(|parsed| {
                                TeloxideInlineKeyboardButton::url(button.text.clone(), parsed)
                            })
                            .map_err(|error| {
                                validation_error(
                                    TelegramOperation::SendMessage,
                                    "markup.url",
                                    "markup url must be valid",
                                )
                                .with_details(serde_json::json!({
                                    "source": error.to_string(),
                                    "text": button.text,
                                }))
                            })
                    } else {
                        Ok(TeloxideInlineKeyboardButton::callback(
                            button.text.clone(),
                            button
                                .callback_data
                                .clone()
                                .unwrap_or_else(|| button.text.clone()),
                        ))
                    }
                })
                .collect::<Result<Vec<_>, TelegramError>>()
        })
        .collect::<Result<Vec<_>, TelegramError>>()?;

    Ok(TeloxideReplyMarkup::InlineKeyboard(
        TeloxideInlineKeyboardMarkup::new(inline_keyboard),
    ))
}

fn map_teloxide_error(
    operation: TelegramOperation,
    error: TeloxideRequestError,
) -> TelegramError {
    match error {
        TeloxideRequestError::RetryAfter(seconds) => TelegramError::new(
            operation,
            TelegramErrorKind::RateLimited,
            format!("telegram rate limited request, retry after {}s", seconds.seconds()),
        )
        .with_retryable(true)
        .with_details(serde_json::json!({
            "retry_after_seconds": seconds.seconds(),
        })),
        TeloxideRequestError::MigrateToChatId(chat_id) => TelegramError::new(
            operation,
            TelegramErrorKind::Conflict,
            format!("telegram chat migrated to {}", chat_id.0),
        )
        .with_retryable(true)
        .with_details(serde_json::json!({
            "migrate_to_chat_id": chat_id.0,
        })),
        TeloxideRequestError::Network(source) => {
            TelegramError::new(
                operation,
                TelegramErrorKind::TransportUnavailable,
                source.to_string(),
            )
            .with_retryable(true)
        }
        TeloxideRequestError::Io(source) => {
            TelegramError::new(
                operation,
                TelegramErrorKind::TransportUnavailable,
                source.to_string(),
            )
            .with_retryable(true)
        }
        TeloxideRequestError::InvalidJson { source, raw } => TelegramError::new(
            operation,
            TelegramErrorKind::Internal,
            source.to_string(),
        )
        .with_retryable(true)
        .with_details(serde_json::json!({
            "raw": raw,
        })),
        TeloxideRequestError::Api(api_error) => map_teloxide_api_error(operation, api_error),
    }
}

fn map_teloxide_api_error(
    operation: TelegramOperation,
    api_error: TeloxideApiError,
) -> TelegramError {
    match api_error {
        TeloxideApiError::ChatNotFound
        | TeloxideApiError::UserNotFound
        | TeloxideApiError::MessageToDeleteNotFound
        | TeloxideApiError::MessageIdInvalid
        | TeloxideApiError::InvalidQueryId => {
            TelegramError::new(operation, TelegramErrorKind::NotFound, api_error.to_string())
        }
        TeloxideApiError::MessageTextIsEmpty
        | TeloxideApiError::CantRestrictSelf
        | TeloxideApiError::MethodNotAvailableInPrivateChats => {
            TelegramError::new(operation, TelegramErrorKind::Validation, api_error.to_string())
        }
        TeloxideApiError::NotEnoughRightsToChangeChatPermissions
        | TeloxideApiError::NotEnoughRightsToRestrict
        | TeloxideApiError::NotEnoughRightsToPostMessages
        | TeloxideApiError::MessageCantBeDeleted => TelegramError::new(
            operation,
            TelegramErrorKind::PermissionDenied,
            api_error.to_string(),
        ),
        TeloxideApiError::BotBlocked
        | TeloxideApiError::BotKicked
        | TeloxideApiError::BotKickedFromSupergroup
        | TeloxideApiError::BotKickedFromChannel
        | TeloxideApiError::UserDeactivated
        | TeloxideApiError::CantInitiateConversation
        | TeloxideApiError::CantTalkWithBots
        | TeloxideApiError::InvalidToken => {
            TelegramError::new(operation, TelegramErrorKind::Denied, api_error.to_string())
        }
        TeloxideApiError::UnknownHost | TeloxideApiError::TerminatedByOtherGetUpdates => {
            TelegramError::new(
                operation,
                TelegramErrorKind::TransportUnavailable,
                api_error.to_string(),
            )
            .with_retryable(true)
        }
        _ => TelegramError::new(operation, TelegramErrorKind::Internal, api_error.to_string()),
    }
}

fn predict_result(request: &TelegramRequest) -> TelegramResult {
    match request {
        TelegramRequest::SendUi(request) => TelegramResult::Ui(TelegramUiResult {
            chat_id: request.chat_id,
            message_id: request.reply_to_message_id.unwrap_or(0).saturating_add(1),
            template: request.template.clone(),
            edited: false,
            raw_passthrough: false,
        }),
        TelegramRequest::SendMessage(request) => TelegramResult::Message(TelegramMessageResult {
            chat_id: request.chat_id,
            message_id: request.reply_to_message_id.unwrap_or(0).saturating_add(1),
            raw_passthrough: false,
        }),
        TelegramRequest::EditUi(request) => TelegramResult::Ui(TelegramUiResult {
            chat_id: request.chat_id,
            message_id: request.message_id,
            template: request.template.clone(),
            edited: true,
            raw_passthrough: false,
        }),
        TelegramRequest::Delete(request) => TelegramResult::Delete(TelegramDeleteResult {
            chat_id: request.chat_id,
            deleted: vec![request.message_id],
            failed: Vec::new(),
        }),
        TelegramRequest::DeleteMany(request) => TelegramResult::Delete(TelegramDeleteResult {
            chat_id: request.chat_id,
            deleted: request.message_ids.clone(),
            failed: Vec::new(),
        }),
        TelegramRequest::Restrict(request) => {
            TelegramResult::Restriction(TelegramRestrictionResult {
                chat_id: request.chat_id,
                user_id: request.user_id,
                until: request.until,
                permissions: request.permissions.clone(),
                changed: true,
            })
        }
        TelegramRequest::Unrestrict(request) => {
            TelegramResult::Restriction(TelegramRestrictionResult {
                chat_id: request.chat_id,
                user_id: request.user_id,
                until: None,
                permissions: TelegramPermissions::default(),
                changed: true,
            })
        }
        TelegramRequest::Ban(request) => TelegramResult::Ban(TelegramBanResult {
            chat_id: request.chat_id,
            user_id: request.user_id,
            until: request.until,
            delete_history: request.delete_history,
            changed: true,
        }),
        TelegramRequest::Unban(request) => TelegramResult::Ban(TelegramBanResult {
            chat_id: request.chat_id,
            user_id: request.user_id,
            until: None,
            delete_history: false,
            changed: true,
        }),
        TelegramRequest::AnswerCallback(request) => {
            TelegramResult::Callback(TelegramCallbackResult {
                callback_query_id: request.callback_query_id.clone(),
                answered: true,
                show_alert: request.show_alert,
                text: request.text.clone(),
            })
        }
    }
}

impl fmt::Display for TelegramError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.operation.as_str(), self.message)
    }
}

impl std::error::Error for TelegramError {}

#[cfg(test)]
mod tests {
    use super::{
        NoopTelegramTransport, ParseMode, TelegramDeleteManyRequest, TelegramErrorKind,
        TelegramExecutionOptions, TelegramGateway, TelegramMessageResult, TelegramOperation,
        TelegramRequest, TelegramResult, TelegramTransport, TelegramUiResult,
    };
    use async_trait::async_trait;
    use serde_json::{json, to_value};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    struct StaticTransport {
        result: TelegramResult,
        calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl TelegramTransport for StaticTransport {
        fn name(&self) -> &'static str {
            "static"
        }

        async fn execute(
            &self,
            _request: TelegramRequest,
        ) -> Result<TelegramResult, super::TelegramError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.result.clone())
        }
    }

    #[test]
    fn gateway_defaults_to_noop_transport() {
        let gateway = TelegramGateway::default();

        assert!(gateway.polling());
        assert_eq!(gateway.transport_name(), "noop");
        assert_eq!(
            format!("{gateway:?}"),
            r#"TelegramGateway { polling: true, transport: "noop" }"#
        );
    }

    #[tokio::test]
    async fn noop_transport_returns_typed_error() {
        let transport = NoopTelegramTransport;
        let error = transport
            .execute(TelegramRequest::SendMessage(
                super::TelegramSendMessageRequest {
                    chat_id: -100,
                    text: "hello".to_owned(),
                    reply_to_message_id: None,
                    silent: false,
                    parse_mode: ParseMode::PlainText,
                    markup: None,
                },
            ))
            .await
            .expect_err("noop transport should fail");

        assert_eq!(error.kind, TelegramErrorKind::TransportUnavailable);
        assert_eq!(error.operation, TelegramOperation::SendMessage);
    }

    #[test]
    fn delete_many_request_serializes_with_canonical_op_tag() {
        let request = TelegramRequest::DeleteMany(TelegramDeleteManyRequest {
            chat_id: -100,
            message_ids: vec![10, 11, 12],
            idempotency_key: Some("del:-100:10-12".to_owned()),
        });

        let json = to_value(&request).expect("request serializes");
        assert_eq!(json["op"], "tg.delete_many");
        assert_eq!(json["chat_id"], -100);
        assert_eq!(json["message_ids"], json!([10, 11, 12]));
        assert_eq!(request.idempotency_key(), Some("del:-100:10-12"));
        assert!(request.operation().requires_idempotency());
    }

    #[test]
    fn result_accessors_return_normalized_identifiers() {
        let message = TelegramResult::Message(TelegramMessageResult {
            chat_id: -100,
            message_id: 42,
            raw_passthrough: false,
        });
        let ui = TelegramResult::Ui(TelegramUiResult {
            chat_id: -100,
            message_id: 43,
            template: "moderation/warn.md".to_owned(),
            edited: true,
            raw_passthrough: false,
        });

        assert_eq!(message.chat_id(), Some(-100));
        assert_eq!(message.message_id(), Some(42));
        assert_eq!(message.operation_kind(), super::TelegramResultKind::Message);
        assert_eq!(ui.chat_id(), Some(-100));
        assert_eq!(ui.message_id(), Some(43));
        assert_eq!(ui.operation_kind(), super::TelegramResultKind::Ui);
    }

    #[tokio::test]
    async fn gateway_dispatches_to_custom_transport() {
        let gateway = TelegramGateway::new(false).with_transport(StaticTransport {
            result: TelegramResult::Ui(TelegramUiResult {
                chat_id: -100,
                message_id: 81,
                template: "ui/session.md".to_owned(),
                edited: false,
                raw_passthrough: false,
            }),
            calls: Arc::new(AtomicUsize::new(0)),
        });

        let result = gateway
            .execute(TelegramRequest::SendUi(super::TelegramSendUiRequest {
                chat_id: -100,
                template: "ui/session.md".to_owned(),
                data: json!({"target":"@spam_user"}),
                reply_to_message_id: Some(80),
                silent: true,
                parse_mode: ParseMode::MarkdownV2,
                markup: None,
            }))
            .await
            .expect("transport should succeed");

        assert!(!gateway.polling());
        assert_eq!(gateway.transport_name(), "static");
        assert_eq!(result.message_id(), Some(81));
    }

    #[tokio::test]
    async fn execute_checked_rejects_missing_idempotency_for_destructive_ops() {
        let gateway = TelegramGateway::default();

        let error = gateway
            .execute_checked(
                TelegramRequest::DeleteMany(TelegramDeleteManyRequest {
                    chat_id: -100,
                    message_ids: vec![10, 11],
                    idempotency_key: None,
                }),
                TelegramExecutionOptions::default(),
            )
            .await
            .expect_err("destructive op without idempotency must fail");

        assert_eq!(error.kind, TelegramErrorKind::Validation);
        assert_eq!(error.operation, TelegramOperation::DeleteMany);
    }

    #[tokio::test]
    async fn execute_checked_dry_run_predicts_without_transport_call() {
        let calls = Arc::new(AtomicUsize::new(0));
        let gateway = TelegramGateway::default().with_transport(StaticTransport {
            result: TelegramResult::Delete(super::TelegramDeleteResult {
                chat_id: -100,
                deleted: vec![77],
                failed: Vec::new(),
            }),
            calls: Arc::clone(&calls),
        });

        let execution = gateway
            .execute_checked(
                TelegramRequest::Delete(super::TelegramDeleteRequest {
                    chat_id: -100,
                    message_id: 77,
                    idempotency_key: Some("delete:-100:77".to_owned()),
                }),
                TelegramExecutionOptions { dry_run: true },
            )
            .await
            .expect("dry run must succeed");

        assert!(execution.metadata.dry_run);
        assert!(!execution.metadata.replayed);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(execution.result.chat_id(), Some(-100));
    }

    #[tokio::test]
    async fn execute_checked_replays_cached_idempotent_result() {
        let calls = Arc::new(AtomicUsize::new(0));
        let gateway = TelegramGateway::default().with_transport(StaticTransport {
            result: TelegramResult::Delete(super::TelegramDeleteResult {
                chat_id: -100,
                deleted: vec![77, 78],
                failed: Vec::new(),
            }),
            calls: Arc::clone(&calls),
        });

        let request = TelegramRequest::DeleteMany(TelegramDeleteManyRequest {
            chat_id: -100,
            message_ids: vec![77, 78],
            idempotency_key: Some("del-window:-100:77-78".to_owned()),
        });

        let first = gateway
            .execute_checked(request.clone(), TelegramExecutionOptions::default())
            .await
            .expect("first call succeeds");
        let second = gateway
            .execute_checked(request, TelegramExecutionOptions::default())
            .await
            .expect("second call succeeds");

        assert!(!first.metadata.replayed);
        assert!(second.metadata.replayed);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(first.result, second.result);
    }

    #[tokio::test]
    async fn execute_checked_validates_non_empty_message_text() {
        let gateway = TelegramGateway::default();
        let error = gateway
            .execute_checked(
                TelegramRequest::SendMessage(super::TelegramSendMessageRequest {
                    chat_id: -100,
                    text: "   ".to_owned(),
                    reply_to_message_id: None,
                    silent: false,
                    parse_mode: ParseMode::PlainText,
                    markup: None,
                }),
                TelegramExecutionOptions::default(),
            )
            .await
            .expect_err("empty text must fail");

        assert_eq!(error.kind, TelegramErrorKind::Validation);
        assert_eq!(error.operation, TelegramOperation::SendMessage);
    }
}
