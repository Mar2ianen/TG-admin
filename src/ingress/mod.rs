use std::{rc::Rc, time::Duration};

use anyhow::{Context, Result};
use chrono::Utc;
use serde_json::json;
use teloxide_core::payloads::GetUpdatesSetters;
use teloxide_core::prelude::{Request, Requester};
use teloxide_core::types::{
    AllowedUpdate, CallbackQuery, Chat, ChatKind, MediaKind, Message, MessageKind, PublicChatKind,
    Update, UpdateKind, User,
};
use tracing::warn;

use crate::event::{
    CallbackContext, ChatContext, EventContext, EventNormalizer, MemberContext, MessageContentKind,
    MessageContext, ReactionContext, ReplyContext, SenderContext, TelegramUpdateInput, UpdateType,
};
use crate::router::ExecutionRouter;
use crate::shutdown::{ShutdownController, ShutdownReason};
use crate::storage::{
    MessageJournalRecord, PROCESSED_UPDATE_STATUS_COMPLETED, PROCESSED_UPDATE_STATUS_PENDING,
    ProcessedUpdateRecord, StorageConnection,
};

const POLL_LIMIT: u8 = 32;
const POLL_TIMEOUT_SECS: u32 = 30;
const POLL_RETRY_INITIAL_DELAY: Duration = Duration::from_millis(250);
const POLL_RETRY_MAX_DELAY: Duration = Duration::from_secs(5);

#[derive(Debug)]
pub struct IngressPipeline {
    bot: teloxide_core::Bot,
    storage: StorageConnection,
    normalizer: EventNormalizer,
    router: Rc<ExecutionRouter>,
    admin_user_ids: Vec<i64>,
}

impl IngressPipeline {
    pub fn new(
        bot: teloxide_core::Bot,
        storage: StorageConnection,
        router: Rc<ExecutionRouter>,
    ) -> Self {
        Self {
            bot,
            storage,
            normalizer: EventNormalizer::new(),
            router,
            admin_user_ids: Vec::new(),
        }
    }

    pub fn with_admin_user_ids<I>(mut self, admin_user_ids: I) -> Self
    where
        I: IntoIterator<Item = i64>,
    {
        self.admin_user_ids = admin_user_ids.into_iter().collect();
        self
    }

    pub fn router(&self) -> &ExecutionRouter {
        self.router.as_ref()
    }

    pub async fn run_until_shutdown(&self, shutdown: ShutdownController) -> Result<ShutdownReason> {
        let mut offset = None;
        let mut retry_delay = POLL_RETRY_INITIAL_DELAY;

        loop {
            tokio::select! {
                reason = shutdown.wait() => return reason,
                result = self.poll_once(offset) => {
                    match result {
                        Ok(next_offset) => {
                            offset = next_offset;
                            retry_delay = POLL_RETRY_INITIAL_DELAY;
                        }
                        Err(err) => {
                            warn!(
                                error = %err,
                                ?offset,
                                retry_delay_ms = retry_delay.as_millis(),
                                "ingress polling failed; retrying"
                            );

                            tokio::select! {
                                reason = shutdown.wait() => return reason,
                                _ = tokio::time::sleep(retry_delay) => {}
                            }

                            retry_delay = next_retry_delay(retry_delay);
                        }
                    }
                }
            }
        }
    }

    async fn poll_once(&self, mut offset: Option<i32>) -> Result<Option<i32>> {
        let mut request = self
            .bot
            .get_updates()
            .limit(POLL_LIMIT)
            .timeout(POLL_TIMEOUT_SECS)
            .allowed_updates(vec![
                AllowedUpdate::Message,
                AllowedUpdate::EditedMessage,
                AllowedUpdate::ChannelPost,
                AllowedUpdate::EditedChannelPost,
                AllowedUpdate::CallbackQuery,
                AllowedUpdate::MyChatMember,
                AllowedUpdate::ChatMember,
                AllowedUpdate::ChatJoinRequest,
                AllowedUpdate::MessageReaction,
                AllowedUpdate::MessageReactionCount,
            ]);

        if let Some(offset) = offset {
            request = request.offset(offset);
        }

        let updates: Vec<Update> = request
            .send()
            .await
            .context("failed to fetch telegram updates")?;

        for update in updates {
            offset = Some(update.id.0 as i32 + 1);
            if let Err(err) = self.process_update(&update).await {
                warn!(
                    update_id = update.id.0,
                    error = %err,
                    "failed to process telegram update; continuing"
                );
            }
        }

        Ok(offset)
    }

    pub async fn process_update(&self, update: &Update) -> Result<IngressProcessResult> {
        let Some(input) = update_to_input_with_admin_user_ids(update, &self.admin_user_ids)? else {
            return Ok(IngressProcessResult::Ignored);
        };

        let event = self
            .normalizer
            .normalize_telegram(input)
            .context("failed to normalize telegram update")?;

        self.process_event(event).await
    }

    async fn process_event(&self, event: EventContext) -> Result<IngressProcessResult> {
        if let Some(result) = self.preflight_processed_update(&event)? {
            return Ok(result);
        }

        self.append_message_journal(&event)?;
        self.update_counters(&event)?;

        self.router
            .route(&event)
            .await
            .context("failed to route ingress event")?;
        self.complete_processed_update(&event)?;

        Ok(IngressProcessResult::Processed)
    }

    fn update_counters(&self, event: &EventContext) -> Result<()> {
        if !matches!(event.update_type, UpdateType::Message) {
            return Ok(());
        }

        let plan = self.router.plan(event);
        if plan.classified.command_name.is_none() {
            if let (Some(chat), Some(sender)) = (&event.chat, &event.sender) {
                self.storage
                    .increment_message_counters(chat.id, sender.id)?;
            }
        }

        Ok(())
    }

    fn preflight_processed_update(
        &self,
        event: &EventContext,
    ) -> Result<Option<IngressProcessResult>> {
        let Some(update_id) = event.update_id else {
            return Ok(None);
        };

        let existing = self.storage.mark_processed_update(&ProcessedUpdateRecord {
            update_id: update_id as i64,
            event_id: event.event_id.clone(),
            processed_at: event.received_at.to_rfc3339(),
            execution_mode: String::from("realtime"),
            status: PROCESSED_UPDATE_STATUS_PENDING.to_owned(),
        })?;

        match existing {
            Some(record) if record.status == PROCESSED_UPDATE_STATUS_COMPLETED => {
                Ok(Some(IngressProcessResult::Replayed(record.event_id)))
            }
            Some(record) => Ok(Some(IngressProcessResult::Interrupted(record.event_id))),
            None => Ok(None),
        }
    }

    fn complete_processed_update(&self, event: &EventContext) -> Result<()> {
        let Some(update_id) = event.update_id else {
            return Ok(());
        };

        let _ = self
            .storage
            .complete_processed_update(update_id as i64, &Utc::now().to_rfc3339())?;
        Ok(())
    }

    fn append_message_journal(&self, event: &EventContext) -> Result<()> {
        let (Some(chat), Some(message)) = (&event.chat, &event.message) else {
            return Ok(());
        };

        self.storage.append_message_journal(&MessageJournalRecord {
            chat_id: chat.id,
            message_id: i64::from(message.id),
            user_id: event.sender.as_ref().map(|sender| sender.id),
            date_utc: message.date.to_rfc3339(),
            update_type: update_type_name(event.update_type).to_owned(),
            text: message.text.clone(),
            normalized_text: message.text.as_ref().map(|text| text.trim().to_owned()),
            has_media: message.has_media,
            reply_to_message_id: message.reply_to_message_id.map(i64::from),
            file_ids_json: if message.file_ids.is_empty() {
                None
            } else {
                Some(serde_json::to_string(&message.file_ids)?)
            },
            meta_json: Some(serde_json::to_string(&json!({
                "content_kind": message.content_kind,
                "media_group_id": message.media_group_id,
                "author_kind": format!("{:?}", event.author_source_class()),
                "linked_channel_style": event.is_linked_channel_style_approx(),
            }))?),
        })?;

        Ok(())
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum IngressProcessResult {
    Processed,
    Replayed(String),
    Interrupted(String),
    Ignored,
}

pub fn update_to_input(update: &Update) -> Result<Option<TelegramUpdateInput>> {
    update_to_input_with_admin_user_ids(update, &[])
}

fn update_to_input_with_admin_user_ids(
    update: &Update,
    admin_user_ids: &[i64],
) -> Result<Option<TelegramUpdateInput>> {
    match &update.kind {
        UpdateKind::Message(message) => Ok(Some(message_update_input(
            update.id.0,
            UpdateType::Message,
            message,
            admin_user_ids,
        ))),
        UpdateKind::EditedMessage(message) => Ok(Some(message_update_input(
            update.id.0,
            UpdateType::EditedMessage,
            message,
            admin_user_ids,
        ))),
        UpdateKind::ChannelPost(message) => Ok(Some(message_update_input(
            update.id.0,
            UpdateType::ChannelPost,
            message,
            admin_user_ids,
        ))),
        UpdateKind::EditedChannelPost(message) => Ok(Some(message_update_input(
            update.id.0,
            UpdateType::EditedChannelPost,
            message,
            admin_user_ids,
        ))),
        UpdateKind::CallbackQuery(callback) => {
            callback_update_input(update.id.0, callback, admin_user_ids)
        }
        UpdateKind::ChatMember(member) => Ok(Some(chat_member_updated_to_input(
            update.id.0,
            UpdateType::ChatMember,
            member,
            admin_user_ids,
        ))),
        UpdateKind::MyChatMember(member) => Ok(Some(chat_member_updated_to_input(
            update.id.0,
            UpdateType::MyChatMember,
            member,
            admin_user_ids,
        ))),
        UpdateKind::ChatJoinRequest(request) => Ok(Some(chat_member_update_input(
            update.id.0,
            UpdateType::JoinRequest,
            &request.chat,
            &request.from,
            request.date,
            admin_user_ids,
        ))),
        UpdateKind::MessageReaction(reaction) => Ok(Some(reaction_update_input(
            update.id.0,
            UpdateType::MessageReaction,
            reaction,
            admin_user_ids,
        ))),
        UpdateKind::MessageReactionCount(reaction) => Ok(Some(reaction_count_update_input(
            update.id.0,
            UpdateType::MessageReactionCount,
            reaction,
            admin_user_ids,
        ))),
        _ => Ok(None),
    }
}

fn message_update_input(
    update_id: u32,
    update_type: UpdateType,
    message: &Message,
    admin_user_ids: &[i64],
) -> TelegramUpdateInput {
    TelegramUpdateInput {
        event_id: None,
        update_id: u64::from(update_id),
        update_type,
        received_at: message.date,
        execution_mode: crate::event::ExecutionMode::Realtime,
        chat: chat_context(&message.chat, message),
        sender: sender_context_from_message(message, admin_user_ids),
        message: Some(message_context(message)),
        reply: reply_context_from_message(message),
        callback: None,
        chat_member: None,
        reaction: None,
        locale: None,
        trace_id: None,
        build: None,
    }
}

fn callback_update_input(
    update_id: u32,
    callback: &CallbackQuery,
    admin_user_ids: &[i64],
) -> Result<Option<TelegramUpdateInput>> {
    let Some(maybe_message) = callback.message.as_ref() else {
        return Ok(None);
    };

    let regular_message = callback.regular_message();
    let received_at = regular_message
        .map(|message| message.date)
        .unwrap_or_else(Utc::now);
    let (chat, message, reply) = match regular_message {
        Some(message) => (
            chat_context(&message.chat, message),
            Some(message_context(message)),
            reply_context_from_message(message),
        ),
        None => (
            chat_context_without_message(maybe_message.chat()),
            None,
            None,
        ),
    };

    Ok(Some(TelegramUpdateInput {
        event_id: None,
        update_id: u64::from(update_id),
        update_type: UpdateType::CallbackQuery,
        received_at,
        execution_mode: crate::event::ExecutionMode::Realtime,
        chat,
        sender: Some(sender_context_from_user(&callback.from, admin_user_ids)),
        message,
        reply,
        callback: Some(CallbackContext {
            query_id: callback.id.to_string(),
            data: callback.data.clone(),
            message_id: Some(maybe_message.id().0),
            origin_chat_id: Some(maybe_message.chat().id.0),
            from_user_id: callback.from.id.0 as i64,
        }),
        chat_member: None,
        reaction: None,
        locale: callback.from.language_code.clone(),
        trace_id: None,
        build: None,
    }))
}

fn chat_member_updated_to_input(
    update_id: u32,
    update_type: UpdateType,
    member: &teloxide_core::types::ChatMemberUpdated,
    admin_user_ids: &[i64],
) -> TelegramUpdateInput {
    TelegramUpdateInput {
        event_id: None,
        update_id: u64::from(update_id),
        update_type,
        received_at: member.date,
        execution_mode: crate::event::ExecutionMode::Realtime,
        chat: chat_context_without_message(&member.chat),
        sender: Some(sender_context_from_user(&member.from, admin_user_ids)),
        message: None,
        reply: None,
        callback: None,
        chat_member: Some(MemberContext {
            old_status: format!("{:?}", member.old_chat_member.kind),
            new_status: format!("{:?}", member.new_chat_member.kind),
            user: sender_context_from_user(&member.new_chat_member.user, admin_user_ids),
        }),
        reaction: None,
        locale: member.from.language_code.clone(),
        trace_id: None,
        build: None,
    }
}

fn chat_member_update_input(
    update_id: u32,
    update_type: UpdateType,
    chat: &Chat,
    sender: &User,
    received_at: chrono::DateTime<chrono::Utc>,
    admin_user_ids: &[i64],
) -> TelegramUpdateInput {
    TelegramUpdateInput {
        event_id: None,
        update_id: u64::from(update_id),
        update_type,
        received_at,
        execution_mode: crate::event::ExecutionMode::Realtime,
        chat: chat_context_without_message(chat),
        sender: Some(sender_context_from_user(sender, admin_user_ids)),
        message: None,
        reply: None,
        callback: None,
        chat_member: None,
        reaction: None,
        locale: sender.language_code.clone(),
        trace_id: None,
        build: None,
    }
}

fn reaction_update_input(
    update_id: u32,
    update_type: UpdateType,
    reaction: &teloxide_core::types::MessageReactionUpdated,
    admin_user_ids: &[i64],
) -> TelegramUpdateInput {
    let sender = reaction
        .user()
        .map(|user| sender_context_from_user(user, admin_user_ids));
    TelegramUpdateInput {
        event_id: None,
        update_id: u64::from(update_id),
        update_type,
        received_at: Utc::now(),
        execution_mode: crate::event::ExecutionMode::Realtime,
        chat: chat_context_without_message(&reaction.chat),
        sender,
        message: None,
        reply: None,
        callback: None,
        chat_member: None,
        reaction: Some(ReactionContext {
            message_id: reaction.message_id.0,
            old_reaction: reaction
                .old_reaction
                .iter()
                .map(|r| format!("{:?}", r))
                .collect(),
            new_reaction: reaction
                .new_reaction
                .iter()
                .map(|r| format!("{:?}", r))
                .collect(),
        }),
        locale: None,
        trace_id: None,
        build: None,
    }
}

fn reaction_count_update_input(
    update_id: u32,
    update_type: UpdateType,
    reaction: &teloxide_core::types::MessageReactionCountUpdated,
    _admin_user_ids: &[i64],
) -> TelegramUpdateInput {
    TelegramUpdateInput {
        event_id: None,
        update_id: u64::from(update_id),
        update_type,
        received_at: Utc::now(),
        execution_mode: crate::event::ExecutionMode::Realtime,
        chat: chat_context_without_message(&reaction.chat),
        sender: None,
        message: None,
        reply: None,
        callback: None,
        chat_member: None,
        reaction: Some(ReactionContext {
            message_id: reaction.message_id.0,
            old_reaction: Vec::new(),
            new_reaction: reaction
                .reactions
                .iter()
                .map(|r| format!("{:?}", r.r#type))
                .collect(),
        }),
        locale: None,
        trace_id: None,
        build: None,
    }
}

fn chat_context(chat: &Chat, message: &Message) -> ChatContext {
    let mut context = chat_context_without_message(chat);
    context.thread_id = message_thread_id(message);
    context
}

fn chat_context_without_message(chat: &Chat) -> ChatContext {
    let (chat_type, username) = match &chat.kind {
        ChatKind::Private(private) => ("private".to_owned(), private.username.clone()),
        ChatKind::Public(public) => {
            let username = match &public.kind {
                PublicChatKind::Channel(channel) => channel.username.clone(),
                PublicChatKind::Group => None,
                PublicChatKind::Supergroup(group) => group.username.clone(),
            };
            let chat_type = match &public.kind {
                PublicChatKind::Channel(_) => "channel".to_owned(),
                PublicChatKind::Group => "group".to_owned(),
                PublicChatKind::Supergroup(_) => "supergroup".to_owned(),
            };
            (chat_type, username)
        }
    };

    ChatContext {
        id: chat.id.0,
        chat_type,
        title: chat.title().map(str::to_owned),
        username,
        photo_file_id: None,
        thread_id: None,
    }
}

fn message_thread_id(message: &Message) -> Option<i64> {
    message.thread_id.map(|thread_id| i64::from(thread_id.0.0))
}

fn sender_context_from_message(message: &Message, admin_user_ids: &[i64]) -> Option<SenderContext> {
    message
        .from
        .as_ref()
        .map(|user| sender_context_from_user(user, admin_user_ids))
}

fn sender_context_from_user(user: &User, admin_user_ids: &[i64]) -> SenderContext {
    let is_admin = admin_user_ids.contains(&(user.id.0 as i64));
    sender_context(user, is_admin)
}

fn sender_context(user: &User, is_admin: bool) -> SenderContext {
    SenderContext {
        id: user.id.0 as i64,
        username: user.username.clone(),
        display_name: Some(
            [Some(user.first_name.as_str()), user.last_name.as_deref()]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" "),
        )
        .filter(|name| !name.is_empty()),
        first_name: user.first_name.clone(),
        last_name: user.last_name.clone(),
        photo_file_id: None, // User photo must be fetched via HostApi
        is_bot: user.is_bot,
        is_admin,
        role: None,
    }
}

fn message_context(message: &Message) -> MessageContext {
    let content_kind = content_kind_for_message(message);
    MessageContext {
        id: message.id.0,
        date: message.date,
        text: message
            .text()
            .or_else(|| message.caption())
            .map(str::to_owned),
        content_kind: Some(content_kind),
        entities: Vec::new(),
        has_media: !matches!(content_kind, MessageContentKind::Text),
        file_ids: extract_file_ids(message),
        reply_to_message_id: reply_message_id(message),
        media_group_id: message.media_group_id().map(ToString::to_string),
    }
}

fn content_kind_for_message(message: &Message) -> MessageContentKind {
    match &message.kind {
        MessageKind::Invoice(_) => MessageContentKind::Invoice,
        MessageKind::Dice(_) => MessageContentKind::Dice,
        MessageKind::Common(common) => match &common.media_kind {
            MediaKind::Text(_) => MessageContentKind::Text,
            MediaKind::Photo(_) => MessageContentKind::Photo,
            MediaKind::Voice(_) => MessageContentKind::Voice,
            MediaKind::Video(_) => MessageContentKind::Video,
            MediaKind::Audio(_) => MessageContentKind::Audio,
            MediaKind::Document(_) => MessageContentKind::Document,
            MediaKind::Sticker(_) => MessageContentKind::Sticker,
            MediaKind::Animation(_) => MessageContentKind::Animation,
            MediaKind::VideoNote(_) => MessageContentKind::VideoNote,
            MediaKind::Contact(_) => MessageContentKind::Contact,
            MediaKind::Location(_) => MessageContentKind::Location,
            MediaKind::Poll(_) => MessageContentKind::Poll,
            MediaKind::Venue(_) => MessageContentKind::Venue,
            MediaKind::Game(_) => MessageContentKind::Game,
            MediaKind::Story(_) => MessageContentKind::Story,
            _ => MessageContentKind::UnknownMedia,
        },
        _ => MessageContentKind::UnknownMedia,
    }
}

fn extract_file_ids(message: &Message) -> Vec<String> {
    match &message.kind {
        MessageKind::Common(common) => match &common.media_kind {
            MediaKind::Animation(media) => vec![media.animation.file.id.to_string()],
            MediaKind::Audio(media) => vec![media.audio.file.id.to_string()],
            MediaKind::Document(media) => vec![media.document.file.id.to_string()],
            MediaKind::Photo(media) => media
                .photo
                .iter()
                .map(|photo| photo.file.id.to_string())
                .collect(),
            MediaKind::Sticker(media) => vec![media.sticker.file.id.to_string()],
            MediaKind::Video(media) => vec![media.video.file.id.to_string()],
            MediaKind::VideoNote(media) => vec![media.video_note.file.id.to_string()],
            MediaKind::Voice(media) => vec![media.voice.file.id.to_string()],
            _ => Vec::new(),
        },
        _ => Vec::new(),
    }
}

fn reply_context_from_message(message: &Message) -> Option<ReplyContext> {
    let MessageKind::Common(common) = &message.kind else {
        return None;
    };
    let reply = common.reply_to_message.as_deref()?;

    Some(ReplyContext {
        message_id: reply.id.0,
        sender_user_id: reply.from.as_ref().map(|user| user.id.0 as i64),
        sender_username: reply.from.as_ref().and_then(|user| user.username.clone()),
        text: reply.text().or_else(|| reply.caption()).map(str::to_owned),
        has_media: !matches!(content_kind_for_message(reply), MessageContentKind::Text),
    })
}

fn reply_message_id(message: &Message) -> Option<i32> {
    let MessageKind::Common(common) = &message.kind else {
        return None;
    };

    common.reply_to_message.as_ref().map(|reply| reply.id.0)
}

fn update_type_name(update_type: UpdateType) -> &'static str {
    match update_type {
        UpdateType::Message => "message",
        UpdateType::EditedMessage => "edited_message",
        UpdateType::ChannelPost => "channel_post",
        UpdateType::EditedChannelPost => "edited_channel_post",
        UpdateType::CallbackQuery => "callback_query",
        UpdateType::ChatMember => "chat_member",
        UpdateType::MyChatMember => "my_chat_member",
        UpdateType::ChatMemberUpdated => "chat_member_updated",
        UpdateType::MessageReaction => "message_reaction",
        UpdateType::MessageReactionCount => "message_reaction_count",
        UpdateType::JoinRequest => "join_request",
        UpdateType::Job => "job",
        UpdateType::System => "system",
    }
}

fn next_retry_delay(current: Duration) -> Duration {
    std::cmp::min(
        current.checked_add(current).unwrap_or(POLL_RETRY_MAX_DELAY),
        POLL_RETRY_MAX_DELAY,
    )
}

#[cfg(test)]
pub(crate) fn process_polled_updates_for_test(
    updates: Vec<Update>,
    mut process_update: impl FnMut(&Update) -> Result<IngressProcessResult>,
) -> Option<i32> {
    let mut next_offset = None;

    for update in updates {
        next_offset = Some(update.id.0 as i32 + 1);
        if let Err(err) = process_update(&update) {
            warn!(
                update_id = update.id.0,
                error = %err,
                "failed to process telegram update; continuing"
            );
        }
    }

    next_offset
}

pub mod json_export;

#[cfg(test)]
pub mod tests;
