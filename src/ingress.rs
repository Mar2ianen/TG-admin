use std::rc::Rc;

use anyhow::{Context, Result};
use chrono::Utc;
use serde_json::json;
use teloxide_core::payloads::GetUpdatesSetters;
use teloxide_core::prelude::{Request, Requester};
use teloxide_core::types::{
    AllowedUpdate, CallbackQuery, Chat, ChatKind, MediaKind, Message, MessageKind, PublicChatKind,
    Update, UpdateKind, User,
};

use crate::event::{
    CallbackContext, ChatContext, EventContext, EventNormalizer, MessageContentKind,
    MessageContext, ReplyContext, SenderContext, TelegramUpdateInput, UpdateType,
};
use crate::router::ExecutionRouter;
use crate::shutdown::{ShutdownController, ShutdownReason};
use crate::storage::{
    MessageJournalRecord, ProcessedUpdateRecord, StorageConnection,
    PROCESSED_UPDATE_STATUS_COMPLETED, PROCESSED_UPDATE_STATUS_PENDING,
};

const POLL_LIMIT: u8 = 32;
const POLL_TIMEOUT_SECS: u32 = 30;

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

        loop {
            tokio::select! {
                reason = shutdown.wait() => return reason,
                result = self.poll_once(offset) => {
                    offset = result?;
                }
            }
        }
    }

    async fn poll_once(&self, offset: Option<i32>) -> Result<Option<i32>> {
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
            ]);

        if let Some(offset) = offset {
            request = request.offset(offset);
        }

        let updates: Vec<Update> = request
            .send()
            .await
            .context("failed to fetch telegram updates")?;

        let mut next_offset = offset;
        for update in updates {
            next_offset = Some(update.id.0 as i32 + 1);
            self.process_update(&update).await?;
        }

        Ok(next_offset)
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
        self.router
            .route(&event)
            .await
            .context("failed to route ingress event")?;
        self.complete_processed_update(&event)?;

        Ok(IngressProcessResult::Processed)
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
        UpdateKind::ChatMember(member) => Ok(Some(chat_member_update_input(
            update.id.0,
            UpdateType::ChatMember,
            &member.chat,
            &member.from,
            member.date,
            admin_user_ids,
        ))),
        UpdateKind::MyChatMember(member) => Ok(Some(chat_member_update_input(
            update.id.0,
            UpdateType::MyChatMember,
            &member.chat,
            &member.from,
            member.date,
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
    let Some(message) = callback.regular_message() else {
        return Ok(None);
    };

    Ok(Some(TelegramUpdateInput {
        event_id: None,
        update_id: u64::from(update_id),
        update_type: UpdateType::CallbackQuery,
        received_at: message.date,
        execution_mode: crate::event::ExecutionMode::Realtime,
        chat: chat_context(&message.chat, message),
        sender: Some(sender_context_from_user(&callback.from, admin_user_ids)),
        message: Some(message_context(message)),
        reply: reply_context_from_message(message),
        callback: Some(CallbackContext {
            query_id: callback.id.to_string(),
            data: callback.data.clone(),
            message_id: Some(message.id.0),
            origin_chat_id: Some(message.chat.id.0),
            from_user_id: callback.from.id.0 as i64,
        }),
        locale: callback.from.language_code.clone(),
        trace_id: None,
        build: None,
    }))
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
        locale: sender.language_code.clone(),
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
        ChatKind::Public(public) => match &public.kind {
            PublicChatKind::Channel(channel) => ("channel".to_owned(), channel.username.clone()),
            PublicChatKind::Group => ("group".to_owned(), None),
            PublicChatKind::Supergroup(group) => ("supergroup".to_owned(), group.username.clone()),
        },
    };

    ChatContext {
        id: chat.id.0,
        chat_type,
        title: chat.title().map(str::to_owned),
        username,
        thread_id: None,
    }
}

fn message_thread_id(message: &Message) -> Option<i64> {
    message.thread_id.map(|thread_id| i64::from(thread_id.0 .0))
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
        UpdateType::JoinRequest => "join_request",
        UpdateType::Job => "job",
        UpdateType::System => "system",
    }
}

#[cfg(test)]
mod tests {
    use super::{update_to_input_with_admin_user_ids, IngressPipeline, IngressProcessResult};
    use crate::event::{
        ChatContext, EventNormalizer, MessageContext, SenderContext, TelegramUpdateInput,
    };
    use crate::moderation::ModerationEngine;
    use crate::router::{ExecutionOutcome, ExecutionRouter};
    use crate::storage::{
        AuditLogFilter, MessageJournalRecord, Storage, StorageConnection,
        PROCESSED_UPDATE_STATUS_COMPLETED,
    };
    use crate::tg::{
        TelegramDeleteResult, TelegramGateway, TelegramMessageResult, TelegramRequest,
        TelegramResult, TelegramTransport, TelegramUiResult,
    };
    use crate::unit::{ServiceSpec, TriggerSpec, UnitDefinition, UnitManifest, UnitRegistry};
    use async_trait::async_trait;
    use chrono::{TimeZone, Utc};
    use std::rc::Rc;
    use std::sync::{Arc, Mutex};
    use teloxide_core::types::Update;

    fn pipeline() -> (tempfile::TempDir, IngressPipeline, StorageConnection) {
        let dir = tempfile::tempdir().expect("tempdir");
        let storage = Storage::new(dir.path().join("runtime.sqlite3"));
        let _ = storage.bootstrap().expect("bootstrap");
        let ingress_storage = storage.init().expect("ingress storage");
        let inspect_storage = storage.init().expect("inspect storage");
        let pipeline = IngressPipeline::new(
            teloxide_core::Bot::new("123456:TEST_TOKEN"),
            ingress_storage,
            Rc::new(ExecutionRouter::new()),
        )
        .with_admin_user_ids([42]);
        (dir, pipeline, inspect_storage)
    }

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

    fn moderation_ts() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 4, 23, 12, 0, 0)
            .single()
            .expect("valid timestamp")
    }

    fn pipeline_with_router(
        router: ExecutionRouter,
    ) -> (tempfile::TempDir, IngressPipeline, StorageConnection) {
        let dir = tempfile::tempdir().expect("tempdir");
        let storage = Storage::new(dir.path().join("runtime.sqlite3"));
        let _ = storage.bootstrap().expect("bootstrap");
        let ingress_storage = storage.init().expect("ingress storage");
        let inspect_storage = storage.init().expect("inspect storage");
        let pipeline = IngressPipeline::new(
            teloxide_core::Bot::new("123456:TEST_TOKEN"),
            ingress_storage,
            Rc::new(router),
        )
        .with_admin_user_ids([42]);
        (dir, pipeline, inspect_storage)
    }

    fn moderation_pipeline_with_caps(
        caps: &[&str],
    ) -> (
        tempfile::TempDir,
        IngressPipeline,
        StorageConnection,
        Arc<Mutex<Vec<TelegramRequest>>>,
    ) {
        let dir = tempfile::tempdir().expect("tempdir");
        let storage = Storage::new(dir.path().join("runtime.sqlite3"));
        let bootstrap = storage.bootstrap().expect("bootstrap");
        let moderation_storage = bootstrap.into_connection();
        let ingress_storage = storage.init().expect("ingress storage");
        let inspect_storage = storage.init().expect("inspect storage");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let transport = RecordingTransport {
            requests: Arc::clone(&requests),
        };
        let gateway = TelegramGateway::new(false).with_transport(transport);
        let registry = crate::unit::UnitRegistry::load_manifests(vec![{
            let mut manifest = UnitManifest::new(
                UnitDefinition::new("moderation.test"),
                TriggerSpec::command(["warn", "mute", "del", "undo"]),
                ServiceSpec::new("scripts/moderation/test.rhai"),
            );
            manifest.capabilities.allow = caps.iter().map(|value| (*value).to_owned()).collect();
            manifest
        }])
        .registry;
        let moderation = ModerationEngine::new(moderation_storage, gateway)
            .with_unit_registry(registry.clone())
            .with_admin_user_ids([42])
            .without_processed_update_guard();
        let router = ExecutionRouter::new()
            .with_registry(registry)
            .with_moderation(moderation);
        let pipeline = IngressPipeline::new(
            teloxide_core::Bot::new("123456:TEST_TOKEN"),
            ingress_storage,
            Rc::new(router),
        )
        .with_admin_user_ids([42]);
        (dir, pipeline, inspect_storage, requests)
    }

    fn seed_journal(storage: &StorageConnection) {
        for (message_id, user_id) in [(810, Some(99)), (811, Some(77)), (812, Some(99))] {
            storage
                .append_message_journal(&MessageJournalRecord {
                    chat_id: -100123,
                    message_id,
                    user_id,
                    date_utc: moderation_ts().to_rfc3339(),
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

    #[test]
    fn message_updates_capture_thread_id_and_known_admin_sender() {
        let update = serde_json::from_str::<Update>(
            r#"{
                "update_id": 439432600,
                "message": {
                    "chat": {
                        "id": -1001293752024,
                        "title": "CryptoInside Chat",
                        "type": "supergroup",
                        "username": "cryptoinside_talk"
                    },
                    "date": 1721592580,
                    "from": {
                        "first_name": "the Cable Guy",
                        "id": 42,
                        "is_bot": false,
                        "language_code": "en",
                        "username": "spacewhaleblues"
                    },
                    "message_id": 134546,
                    "message_thread_id": 134545,
                    "text": "/report"
                }
            }"#,
        )
        .expect("update parses");

        let input = update_to_input_with_admin_user_ids(&update, &[42])
            .expect("update converts")
            .expect("update supported");
        let event = EventNormalizer::new()
            .normalize_telegram(input)
            .expect("event normalizes");

        assert_eq!(
            event.chat.as_ref().and_then(|chat| chat.thread_id),
            Some(134545)
        );
        assert_eq!(
            event.sender.as_ref().map(|sender| sender.is_admin),
            Some(true)
        );
    }

    #[test]
    fn callback_updates_mark_known_admin_sender() {
        let update = serde_json::from_str::<Update>(
            r#"{
                "update_id": 439432601,
                "callback_query": {
                    "id": "cbq-1",
                    "from": {
                        "first_name": "Alice",
                        "id": 42,
                        "is_bot": false,
                        "language_code": "en",
                        "username": "alice"
                    },
                    "chat_instance": "chat-instance-1",
                    "data": "/undo",
                    "message": {
                        "chat": {
                            "id": -1001293752024,
                            "title": "CryptoInside Chat",
                            "type": "supergroup",
                            "username": "cryptoinside_talk"
                        },
                        "date": 1721592581,
                        "from": {
                            "first_name": "Bot",
                            "id": 999,
                            "is_bot": true,
                            "username": "sample_bot"
                        },
                        "message_id": 134547,
                        "message_thread_id": 134545,
                        "text": "undo?"
                    }
                }
            }"#,
        )
        .expect("update parses");

        let input = update_to_input_with_admin_user_ids(&update, &[42])
            .expect("update converts")
            .expect("update supported");
        let event = EventNormalizer::new()
            .normalize_telegram(input)
            .expect("event normalizes");

        assert_eq!(
            event.chat.as_ref().and_then(|chat| chat.thread_id),
            Some(134545)
        );
        assert_eq!(
            event.sender.as_ref().map(|sender| sender.is_admin),
            Some(true)
        );
        assert_eq!(
            event
                .callback
                .as_ref()
                .map(|callback| callback.from_user_id),
            Some(42)
        );
    }

    #[test]
    fn chat_member_update_converts_and_normalizes() {
        let update = serde_json::from_str::<Update>(
            r#"{
                "update_id": 439432602,
                "chat_member": {
                    "chat": {
                        "id": -1001293752024,
                        "title": "CryptoInside Chat",
                        "type": "supergroup",
                        "username": "cryptoinside_talk"
                    },
                    "from": {
                        "first_name": "Alice",
                        "id": 42,
                        "is_bot": false,
                        "language_code": "en",
                        "username": "alice"
                    },
                    "date": 1721592582,
                    "old_chat_member": {
                        "user": {
                            "first_name": "Bob",
                            "id": 99,
                            "is_bot": false,
                            "username": "bob"
                        },
                        "status": "member"
                    },
                    "new_chat_member": {
                        "user": {
                            "first_name": "Bob",
                            "id": 99,
                            "is_bot": false,
                            "username": "bob"
                        },
                        "status": "kicked",
                        "until_date": 0
                    }
                }
            }"#,
        )
        .expect("update parses");

        let input = update_to_input_with_admin_user_ids(&update, &[42])
            .expect("update converts")
            .expect("update supported");
        let event = EventNormalizer::new()
            .normalize_telegram(input)
            .expect("event normalizes");

        assert_eq!(event.update_type, crate::event::UpdateType::ChatMember);
        assert_eq!(
            event.chat.as_ref().map(|chat| chat.id),
            Some(-1001293752024)
        );
        assert_eq!(event.chat.as_ref().and_then(|chat| chat.thread_id), None);
        assert_eq!(event.sender.as_ref().map(|sender| sender.id), Some(42));
        assert_eq!(
            event.sender.as_ref().map(|sender| sender.is_admin),
            Some(true)
        );
        assert!(event.message.is_none());
        assert!(event.callback.is_none());
    }

    #[test]
    fn my_chat_member_update_converts_and_normalizes() {
        let update = serde_json::from_str::<Update>(
            r#"{
                "update_id": 439432603,
                "my_chat_member": {
                    "chat": {
                        "id": 408258968,
                        "first_name": "Hirrolot",
                        "type": "private",
                        "username": "hirrolot"
                    },
                    "from": {
                        "first_name": "Hirrolot",
                        "id": 42,
                        "is_bot": false,
                        "language_code": "ru",
                        "username": "hirrolot"
                    },
                    "date": 1721592583,
                    "old_chat_member": {
                        "user": {
                            "first_name": "Bot",
                            "id": 999,
                            "is_bot": true,
                            "username": "sample_bot"
                        },
                        "status": "member"
                    },
                    "new_chat_member": {
                        "user": {
                            "first_name": "Bot",
                            "id": 999,
                            "is_bot": true,
                            "username": "sample_bot"
                        },
                        "status": "kicked",
                        "until_date": 0
                    }
                }
            }"#,
        )
        .expect("update parses");

        let input = update_to_input_with_admin_user_ids(&update, &[42])
            .expect("update converts")
            .expect("update supported");
        let event = EventNormalizer::new()
            .normalize_telegram(input)
            .expect("event normalizes");

        assert_eq!(event.update_type, crate::event::UpdateType::MyChatMember);
        assert_eq!(event.chat.as_ref().map(|chat| chat.id), Some(408258968));
        assert_eq!(event.sender.as_ref().map(|sender| sender.id), Some(42));
        assert_eq!(event.system.locale.as_deref(), Some("ru"));
        assert!(event.message.is_none());
        assert!(event.callback.is_none());
    }

    #[test]
    fn join_request_update_converts_and_normalizes() {
        let update = serde_json::from_str::<Update>(
            r#"{
                "update_id": 439432604,
                "chat_join_request": {
                    "chat": {
                        "id": -1001293752024,
                        "title": "CryptoInside Chat",
                        "type": "supergroup",
                        "username": "cryptoinside_talk"
                    },
                    "from": {
                        "first_name": "Carol",
                        "id": 77,
                        "is_bot": false,
                        "language_code": "en",
                        "username": "carol"
                    },
                    "user_chat_id": 5001,
                    "date": 1721592584,
                    "bio": "let me in"
                }
            }"#,
        )
        .expect("update parses");

        let input = update_to_input_with_admin_user_ids(&update, &[42])
            .expect("update converts")
            .expect("update supported");
        let event = EventNormalizer::new()
            .normalize_telegram(input)
            .expect("event normalizes");

        assert_eq!(event.update_type, crate::event::UpdateType::JoinRequest);
        assert_eq!(
            event.chat.as_ref().map(|chat| chat.id),
            Some(-1001293752024)
        );
        assert_eq!(event.sender.as_ref().map(|sender| sender.id), Some(77));
        assert_eq!(event.system.locale.as_deref(), Some("en"));
        assert!(event.message.is_none());
        assert!(event.callback.is_none());
    }

    #[tokio::test]
    async fn process_event_appends_journal_and_marks_update_complete() {
        let (_dir, pipeline, inspect_storage) = pipeline();
        let event = EventNormalizer::new()
            .normalize_telegram(TelegramUpdateInput::message(
                306197398,
                ChatContext {
                    id: 408258968,
                    chat_type: "private".to_owned(),
                    title: None,
                    username: Some("hirrolot".to_owned()),
                    thread_id: None,
                },
                SenderContext {
                    id: 408258968,
                    username: Some("hirrolot".to_owned()),
                    display_name: Some("Hirrolot".to_owned()),
                    is_bot: false,
                    is_admin: false,
                    role: None,
                },
                MessageContext {
                    id: 154,
                    date: chrono::DateTime::from_timestamp(1_581_448_857, 0).expect("timestamp"),
                    text: Some("4".to_owned()),
                    content_kind: Some(crate::event::MessageContentKind::Text),
                    entities: Vec::new(),
                    has_media: false,
                    file_ids: Vec::new(),
                    reply_to_message_id: None,
                    media_group_id: None,
                },
            ))
            .expect("event normalizes");

        let result = pipeline
            .process_event(event)
            .await
            .expect("ingress succeeds");

        assert_eq!(result, IngressProcessResult::Processed);
        let journal = inspect_storage
            .message_window(408258968, 154, 0, 0, true)
            .expect("journal query");
        assert_eq!(journal.len(), 1);
        assert_eq!(journal[0].text.as_deref(), Some("4"));

        let processed = inspect_storage
            .get_processed_update(306197398)
            .expect("processed query")
            .expect("processed record exists");
        assert_eq!(processed.status, PROCESSED_UPDATE_STATUS_COMPLETED);
    }

    #[tokio::test]
    async fn process_event_skips_replayed_updates_before_routing() {
        let (_dir, pipeline, inspect_storage) = pipeline();
        let event = EventNormalizer::new()
            .normalize_telegram(TelegramUpdateInput::message(
                401,
                ChatContext {
                    id: -100123,
                    chat_type: "supergroup".to_owned(),
                    title: Some("Replay".to_owned()),
                    username: None,
                    thread_id: None,
                },
                SenderContext {
                    id: 77,
                    username: Some("alice".to_owned()),
                    display_name: Some("Alice".to_owned()),
                    is_bot: false,
                    is_admin: false,
                    role: None,
                },
                MessageContext {
                    id: 810,
                    date: chrono::Utc::now(),
                    text: Some("hello".to_owned()),
                    content_kind: Some(crate::event::MessageContentKind::Text),
                    entities: Vec::new(),
                    has_media: false,
                    file_ids: Vec::new(),
                    reply_to_message_id: None,
                    media_group_id: None,
                },
            ))
            .expect("event normalizes");

        let first = pipeline
            .process_event(event.clone())
            .await
            .expect("first ingress succeeds");
        let second = pipeline
            .process_event(event)
            .await
            .expect("second ingress succeeds");

        assert_eq!(first, IngressProcessResult::Processed);
        assert!(matches!(second, IngressProcessResult::Replayed(_)));
        let processed = inspect_storage
            .get_processed_update(401)
            .expect("processed query")
            .expect("processed record exists");
        assert_eq!(processed.status, PROCESSED_UPDATE_STATUS_COMPLETED);
    }

    #[tokio::test]
    async fn process_update_dispatches_loaded_unit_and_marks_update_complete() {
        let registry = UnitRegistry::load_manifests(vec![UnitManifest::new(
            UnitDefinition::new("command.stats.unit"),
            TriggerSpec::command(["stats"]),
            ServiceSpec::new("scripts/command/stats.rhai"),
        )])
        .registry;
        let (_dir, pipeline, inspect_storage) =
            pipeline_with_router(ExecutionRouter::new().with_registry(registry));
        let update = serde_json::from_str::<Update>(
            r#"{
                "update_id": 439432700,
                "message": {
                    "chat": {
                        "id": -1001293752024,
                        "title": "CryptoInside Chat",
                        "type": "supergroup",
                        "username": "cryptoinside_talk"
                    },
                    "date": 1721592680,
                    "from": {
                        "first_name": "Alice",
                        "id": 42,
                        "is_bot": false,
                        "language_code": "en",
                        "username": "alice"
                    },
                    "message_id": 134600,
                    "text": "/stats"
                }
            }"#,
        )
        .expect("update parses");

        let expected_event = EventNormalizer::new()
            .normalize_telegram(
                update_to_input_with_admin_user_ids(&update, &[42])
                    .expect("update converts")
                    .expect("update supported"),
            )
            .expect("event normalizes");
        let outcome = pipeline
            .router()
            .route(&expected_event)
            .await
            .expect("routing succeeds");
        match outcome {
            ExecutionOutcome::UnitDispatch { invocations, .. } => {
                assert_eq!(invocations.len(), 1);
                assert_eq!(invocations[0].unit_id, "command.stats.unit");
                assert_eq!(invocations[0].exec_start, "scripts/command/stats.rhai");
            }
            other => panic!("expected unit dispatch, got {other:?}"),
        }

        let result = pipeline
            .process_update(&update)
            .await
            .expect("ingress succeeds");

        assert_eq!(result, IngressProcessResult::Processed);
        let processed = inspect_storage
            .get_processed_update(439432700)
            .expect("processed query")
            .expect("processed record exists");
        assert_eq!(processed.status, PROCESSED_UPDATE_STATUS_COMPLETED);
        assert_eq!(processed.execution_mode, "realtime");
        assert!(processed.event_id.starts_with("evt_tg_"));
        assert_eq!(processed.event_id.len(), "evt_tg_".len() + 32);
    }

    #[tokio::test]
    async fn process_update_skips_replayed_live_unit_dispatch_before_routing() {
        let registry = UnitRegistry::load_manifests(vec![UnitManifest::new(
            UnitDefinition::new("command.stats.unit"),
            TriggerSpec::command(["stats"]),
            ServiceSpec::new("scripts/command/stats.rhai"),
        )])
        .registry;
        let (_dir, pipeline, inspect_storage) =
            pipeline_with_router(ExecutionRouter::new().with_registry(registry));
        let update = serde_json::from_str::<Update>(
            r#"{
                "update_id": 439432701,
                "message": {
                    "chat": {
                        "id": -1001293752024,
                        "title": "CryptoInside Chat",
                        "type": "supergroup",
                        "username": "cryptoinside_talk"
                    },
                    "date": 1721592681,
                    "from": {
                        "first_name": "Alice",
                        "id": 42,
                        "is_bot": false,
                        "language_code": "en",
                        "username": "alice"
                    },
                    "message_id": 134601,
                    "text": "/stats"
                }
            }"#,
        )
        .expect("update parses");

        let first = pipeline
            .process_update(&update)
            .await
            .expect("first ingress succeeds");
        let second = pipeline
            .process_update(&update)
            .await
            .expect("second ingress succeeds");

        assert_eq!(first, IngressProcessResult::Processed);
        assert!(matches!(second, IngressProcessResult::Replayed(_)));
        let processed = inspect_storage
            .get_processed_update(439432701)
            .expect("processed query")
            .expect("processed record exists");
        assert_eq!(processed.status, PROCESSED_UPDATE_STATUS_COMPLETED);
    }

    #[tokio::test]
    async fn process_update_handles_chat_member_live_update_end_to_end() {
        let (_dir, pipeline, inspect_storage) = pipeline();
        let update = serde_json::from_str::<Update>(
            r#"{
                "update_id": 439432702,
                "chat_member": {
                    "chat": {
                        "id": -1001293752024,
                        "title": "CryptoInside Chat",
                        "type": "supergroup",
                        "username": "cryptoinside_talk"
                    },
                    "from": {
                        "first_name": "Alice",
                        "id": 42,
                        "is_bot": false,
                        "language_code": "en",
                        "username": "alice"
                    },
                    "date": 1721592682,
                    "old_chat_member": {
                        "user": {
                            "first_name": "Bob",
                            "id": 99,
                            "is_bot": false,
                            "username": "bob"
                        },
                        "status": "member"
                    },
                    "new_chat_member": {
                        "user": {
                            "first_name": "Bob",
                            "id": 99,
                            "is_bot": false,
                            "username": "bob"
                        },
                        "status": "kicked",
                        "until_date": 0
                    }
                }
            }"#,
        )
        .expect("update parses");

        let result = pipeline
            .process_update(&update)
            .await
            .expect("ingress succeeds");

        assert_eq!(result, IngressProcessResult::Processed);
        let processed = inspect_storage
            .get_processed_update(439432702)
            .expect("processed query")
            .expect("processed record exists");
        assert_eq!(processed.status, PROCESSED_UPDATE_STATUS_COMPLETED);
        assert_eq!(processed.execution_mode, "realtime");
        assert!(processed.event_id.starts_with("evt_tg_"));
        assert_eq!(processed.event_id.len(), "evt_tg_".len() + 32);
    }

    #[tokio::test]
    async fn process_update_executes_live_warn_via_built_in_moderation() {
        let (_dir, pipeline, inspect_storage, requests) = moderation_pipeline_with_caps(&[]);
        let update = serde_json::from_str::<Update>(
            r#"{
                "update_id": 439432707,
                "message": {
                    "chat": {
                        "id": -100123,
                        "title": "Moderation HQ",
                        "type": "supergroup",
                        "username": "mod_hq"
                    },
                    "date": 1721592687,
                    "from": {
                        "first_name": "Admin",
                        "id": 42,
                        "is_bot": false,
                        "language_code": "en",
                        "username": "admin"
                    },
                    "message_id": 904,
                    "text": "/warn 2.8",
                    "reply_to_message": {
                        "message_id": 810,
                        "chat": {
                            "id": -100123,
                            "title": "Moderation HQ",
                            "type": "supergroup",
                            "username": "mod_hq"
                        },
                        "date": 1721592580,
                        "from": {
                            "first_name": "Spammer",
                            "id": 99,
                            "is_bot": false,
                            "username": "spam_user"
                        },
                        "text": "spam"
                    }
                }
            }"#,
        )
        .expect("update parses");

        let result = pipeline
            .process_update(&update)
            .await
            .expect("ingress succeeds");

        assert_eq!(result, IngressProcessResult::Processed);
        let requests = requests.lock().expect("requests");
        assert!(
            requests.is_empty(),
            "warn should not emit telegram side effects"
        );
        drop(requests);

        let user = inspect_storage
            .get_user(99)
            .expect("user lookup")
            .expect("warn target exists");
        assert_eq!(user.warn_count, 1);

        let warn_entries = inspect_storage
            .find_audit_entries(
                &AuditLogFilter {
                    op: Some("warn".to_owned()),
                    target_id: Some("99".to_owned()),
                    ..AuditLogFilter::default()
                },
                10,
            )
            .expect("audit lookup");
        assert_eq!(warn_entries.len(), 1);

        let processed = inspect_storage
            .get_processed_update(439432707)
            .expect("processed query")
            .expect("processed record exists");
        assert_eq!(processed.status, PROCESSED_UPDATE_STATUS_COMPLETED);
        assert_eq!(processed.execution_mode, "realtime");
        assert!(processed.event_id.starts_with("evt_tg_"));
    }

    #[tokio::test]
    async fn process_update_executes_live_mute_via_built_in_moderation() {
        let (_dir, pipeline, inspect_storage, requests) =
            moderation_pipeline_with_caps(&["tg.moderate.restrict"]);
        let update = serde_json::from_str::<Update>(
            r#"{
                "update_id": 439432703,
                "message": {
                    "chat": {
                        "id": -100123,
                        "title": "Moderation HQ",
                        "type": "supergroup",
                        "username": "mod_hq"
                    },
                    "date": 1721592683,
                    "from": {
                        "first_name": "Admin",
                        "id": 42,
                        "is_bot": false,
                        "language_code": "en",
                        "username": "admin"
                    },
                    "message_id": 900,
                    "text": "/mute 30m spam",
                    "reply_to_message": {
                        "message_id": 902,
                        "chat": {
                            "id": -100123,
                            "title": "Moderation HQ",
                            "type": "supergroup",
                            "username": "mod_hq"
                        },
                        "date": 1721592685,
                        "from": {
                            "first_name": "Admin",
                            "id": 42,
                            "is_bot": false,
                            "username": "admin"
                        },
                        "text": "/mute 30m spam"
                    }
                }
            }"#,
        )
        .expect("update parses");

        let result = pipeline
            .process_update(&update)
            .await
            .expect("ingress succeeds");

        assert_eq!(result, IngressProcessResult::Processed);
        let requests = requests.lock().expect("requests");
        assert_eq!(requests.len(), 1);
        assert!(matches!(requests[0], TelegramRequest::Restrict(_)));
        let processed = inspect_storage
            .get_processed_update(439432703)
            .expect("processed query")
            .expect("processed record exists");
        assert_eq!(processed.status, PROCESSED_UPDATE_STATUS_COMPLETED);
    }

    #[tokio::test]
    async fn process_update_executes_live_mute_dry_run_without_side_effects() {
        let (_dir, pipeline, inspect_storage, requests) =
            moderation_pipeline_with_caps(&["tg.moderate.restrict"]);
        let update = serde_json::from_str::<Update>(
            r#"{
                "update_id": 439432708,
                "message": {
                    "chat": {
                        "id": -100123,
                        "title": "Moderation HQ",
                        "type": "supergroup",
                        "username": "mod_hq"
                    },
                    "date": 1721592688,
                    "from": {
                        "first_name": "Admin",
                        "id": 42,
                        "is_bot": false,
                        "language_code": "en",
                        "username": "admin"
                    },
                    "message_id": 905,
                    "text": "/mute 30m spam -dry",
                    "reply_to_message": {
                        "message_id": 810,
                        "chat": {
                            "id": -100123,
                            "title": "Moderation HQ",
                            "type": "supergroup",
                            "username": "mod_hq"
                        },
                        "date": 1721592580,
                        "from": {
                            "first_name": "Spammer",
                            "id": 99,
                            "is_bot": false,
                            "username": "spam_user"
                        },
                        "text": "spam"
                    }
                }
            }"#,
        )
        .expect("update parses");

        let result = pipeline
            .process_update(&update)
            .await
            .expect("ingress succeeds");

        assert_eq!(result, IngressProcessResult::Processed);
        assert!(requests.lock().expect("requests").is_empty());
        let mute_entries = inspect_storage
            .find_audit_entries(
                &AuditLogFilter {
                    op: Some("mute".to_owned()),
                    target_id: Some("99".to_owned()),
                    ..AuditLogFilter::default()
                },
                10,
            )
            .expect("audit lookup");
        assert!(mute_entries.is_empty());
        let processed = inspect_storage
            .get_processed_update(439432708)
            .expect("processed query")
            .expect("processed record exists");
        assert_eq!(processed.status, PROCESSED_UPDATE_STATUS_COMPLETED);
    }

    #[tokio::test]
    async fn process_update_executes_live_delete_window_via_built_in_moderation() {
        let (_dir, pipeline, inspect_storage, requests) =
            moderation_pipeline_with_caps(&["tg.moderate.delete"]);
        seed_journal(&inspect_storage);
        let update = serde_json::from_str::<Update>(
            r#"{
                "update_id": 439432704,
                "message": {
                    "chat": {
                        "id": -100123,
                        "title": "Moderation HQ",
                        "type": "supergroup",
                        "username": "mod_hq"
                    },
                    "date": 1721592684,
                    "from": {
                        "first_name": "Admin",
                        "id": 42,
                        "is_bot": false,
                        "language_code": "en",
                        "username": "admin"
                    },
                    "message_id": 901,
                    "text": "/del msg:811 -up 1 -dn 1 -user 99"
                }
            }"#,
        )
        .expect("update parses");

        let result = pipeline
            .process_update(&update)
            .await
            .expect("ingress succeeds");

        assert_eq!(result, IngressProcessResult::Processed);
        let requests = requests.lock().expect("requests");
        assert_eq!(requests.len(), 1);
        let TelegramRequest::DeleteMany(request) = &requests[0] else {
            panic!("expected delete_many request");
        };
        assert_eq!(request.message_ids, vec![810, 812]);
        let processed = inspect_storage
            .get_processed_update(439432704)
            .expect("processed query")
            .expect("processed record exists");
        assert_eq!(processed.status, PROCESSED_UPDATE_STATUS_COMPLETED);
    }

    #[tokio::test]
    async fn process_update_executes_live_undo_after_mute_via_built_in_moderation() {
        let (_dir, pipeline, inspect_storage, requests) =
            moderation_pipeline_with_caps(&["tg.moderate.restrict", "audit.compensate"]);
        let mute_update = serde_json::from_str::<Update>(
            r#"{
                "update_id": 439432705,
                "message": {
                    "chat": {
                        "id": -100123,
                        "title": "Moderation HQ",
                        "type": "supergroup",
                        "username": "mod_hq"
                    },
                    "date": 1721592685,
                    "from": {
                        "first_name": "Admin",
                        "id": 42,
                        "is_bot": false,
                        "language_code": "en",
                        "username": "admin"
                    },
                    "message_id": 902,
                    "text": "/mute 30m spam",
                    "reply_to_message": {
                        "message_id": 810,
                        "chat": {
                            "id": -100123,
                            "title": "Moderation HQ",
                            "type": "supergroup",
                            "username": "mod_hq"
                        },
                        "date": 1721592580,
                        "from": {
                            "first_name": "Spammer",
                            "id": 99,
                            "is_bot": false,
                            "username": "spam_user"
                        },
                        "text": "spam"
                    }
                }
            }"#,
        )
        .expect("mute update parses");
        let undo_update = serde_json::from_str::<Update>(
            r#"{
                "update_id": 439432706,
                "message": {
                    "chat": {
                        "id": -100123,
                        "title": "Moderation HQ",
                        "type": "supergroup",
                        "username": "mod_hq"
                    },
                    "date": 1721592686,
                    "from": {
                        "first_name": "Admin",
                        "id": 42,
                        "is_bot": false,
                        "language_code": "en",
                        "username": "admin"
                    },
                    "message_id": 903,
                    "text": "/undo",
                    "reply_to_message": {
                        "message_id": 902,
                        "chat": {
                            "id": -100123,
                            "title": "Moderation HQ",
                            "type": "supergroup",
                            "username": "mod_hq"
                        },
                        "date": 1721592685,
                        "from": {
                            "first_name": "Admin",
                            "id": 42,
                            "is_bot": false,
                            "language_code": "en",
                            "username": "admin"
                        },
                        "text": "/mute 30m spam"
                    }
                }
            }"#,
        )
        .expect("undo update parses");

        let mute_result = pipeline
            .process_update(&mute_update)
            .await
            .expect("mute ingress succeeds");
        let undo_result = pipeline
            .process_update(&undo_update)
            .await
            .expect("undo ingress succeeds");

        assert_eq!(mute_result, IngressProcessResult::Processed);
        assert_eq!(undo_result, IngressProcessResult::Processed);
        let requests = requests.lock().expect("requests");
        assert_eq!(requests.len(), 2);
        assert!(matches!(requests[0], TelegramRequest::Restrict(_)));
        assert!(matches!(requests[1], TelegramRequest::Unrestrict(_)));
        let processed_mute = inspect_storage
            .get_processed_update(439432705)
            .expect("mute processed query")
            .expect("mute processed record exists");
        assert_eq!(processed_mute.status, PROCESSED_UPDATE_STATUS_COMPLETED);
        let processed_undo = inspect_storage
            .get_processed_update(439432706)
            .expect("undo processed query")
            .expect("undo processed record exists");
        assert_eq!(processed_undo.status, PROCESSED_UPDATE_STATUS_COMPLETED);
        let undo_entries = inspect_storage
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
    }
}
