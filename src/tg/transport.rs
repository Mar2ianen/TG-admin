use async_trait::async_trait;
use std::fmt;
use teloxide_core::errors::{ApiError as TeloxideApiError, RequestError as TeloxideRequestError};
use teloxide_core::payloads::{
    AnswerCallbackQuerySetters, BanChatMemberSetters, RestrictChatMemberSetters,
    SendMessageSetters, UnbanChatMemberSetters,
};
use teloxide_core::prelude::{Request, Requester};
use teloxide_core::types::{
    CallbackQueryId as TeloxideCallbackQueryId, ChatId as TeloxideChatId,
    ChatPermissions as TeloxideChatPermissions,
    InlineKeyboardButton as TeloxideInlineKeyboardButton,
    InlineKeyboardMarkup as TeloxideInlineKeyboardMarkup, MessageId as TeloxideMessageId,
    ParseMode as TeloxideParseMode, ReplyMarkup as TeloxideReplyMarkup,
    ReplyParameters as TeloxideReplyParameters, UserId as TeloxideUserId,
};

use super::types::*;
use super::validation::validation_error;

#[async_trait]
pub trait TelegramTransport: Send + Sync + fmt::Debug {
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
                    api = api.reply_parameters(TeloxideReplyParameters::new(TeloxideMessageId(
                        reply_to_message_id,
                    )));
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
                        validation_error(operation, "url", "callback url must be valid")
                            .with_details(serde_json::json!({
                                "source": error.to_string(),
                            }))
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

fn to_teloxide_parse_mode(parse_mode: ParseMode) -> Option<TeloxideParseMode> {
    match parse_mode {
        ParseMode::PlainText => None,
        ParseMode::MarkdownV2 => Some(TeloxideParseMode::MarkdownV2),
        ParseMode::Html => Some(TeloxideParseMode::Html),
    }
}

fn to_teloxide_permissions(permissions: &TelegramPermissions) -> TeloxideChatPermissions {
    let mut mapped = TeloxideChatPermissions::empty();

    macro_rules! map_perms {
        ($($field:ident => $flag:ident),+ $(,)?) => {
            $(
                if permissions.$field.unwrap_or(false) {
                    mapped |= TeloxideChatPermissions::$flag;
                }
            )+
        };
    }

    map_perms! {
        can_send_messages => SEND_MESSAGES,
        can_send_audios => SEND_AUDIOS,
        can_send_documents => SEND_DOCUMENTS,
        can_send_photos => SEND_PHOTOS,
        can_send_videos => SEND_VIDEOS,
        can_send_video_notes => SEND_VIDEO_NOTES,
        can_send_voice_notes => SEND_VOICE_NOTES,
        can_send_polls => SEND_POLLS,
        can_send_other_messages => SEND_OTHER_MESSAGES,
        can_add_web_page_previews => ADD_WEB_PAGE_PREVIEWS,
        can_change_info => CHANGE_INFO,
        can_invite_users => INVITE_USERS,
        can_pin_messages => PIN_MESSAGES,
        can_manage_topics => MANAGE_TOPICS,
    }

    mapped
}

fn to_teloxide_reply_markup(
    markup: &TelegramUiMarkup,
) -> Result<TeloxideReplyMarkup, TelegramError> {
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

fn map_teloxide_error(operation: TelegramOperation, error: TeloxideRequestError) -> TelegramError {
    match error {
        TeloxideRequestError::RetryAfter(seconds) => TelegramError::new(
            operation,
            TelegramErrorKind::RateLimited,
            format!(
                "telegram rate limited request, retry after {}s",
                seconds.seconds()
            ),
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
        TeloxideRequestError::Network(source) => TelegramError::new(
            operation,
            TelegramErrorKind::TransportUnavailable,
            source.to_string(),
        )
        .with_retryable(true),
        TeloxideRequestError::Io(source) => TelegramError::new(
            operation,
            TelegramErrorKind::TransportUnavailable,
            source.to_string(),
        )
        .with_retryable(true),
        TeloxideRequestError::InvalidJson { source, raw } => {
            TelegramError::new(operation, TelegramErrorKind::Internal, source.to_string())
                .with_retryable(true)
                .with_details(serde_json::json!({
                    "raw": raw,
                }))
        }
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
        | TeloxideApiError::InvalidQueryId => TelegramError::new(
            operation,
            TelegramErrorKind::NotFound,
            api_error.to_string(),
        ),
        TeloxideApiError::MessageTextIsEmpty
        | TeloxideApiError::CantRestrictSelf
        | TeloxideApiError::MethodNotAvailableInPrivateChats => TelegramError::new(
            operation,
            TelegramErrorKind::Validation,
            api_error.to_string(),
        ),
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
        _ => TelegramError::new(
            operation,
            TelegramErrorKind::Internal,
            api_error.to_string(),
        ),
    }
}
