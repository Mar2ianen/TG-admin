#![allow(dead_code)]

use anyhow::{Result, bail};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventContext {
    pub event_id: String,
    pub update_id: Option<u64>,
    pub update_type: UpdateType,
    pub received_at: DateTime<Utc>,
    pub execution_mode: ExecutionMode,
    pub recovery: bool,
    pub chat: Option<ChatContext>,
    pub sender: Option<SenderContext>,
    pub message: Option<MessageContext>,
    pub reply: Option<ReplyContext>,
    pub callback: Option<CallbackContext>,
    pub chat_member: Option<MemberContext>,
    pub reaction: Option<ReactionContext>,
    pub job: Option<JobContext>,
    pub system: SystemContext,
}

impl EventContext {
    pub fn system_event() -> Self {
        Self::new(
            "evt_bootstrap",
            UpdateType::System,
            ExecutionMode::Manual,
            SystemContext::runtime_bootstrap(),
        )
    }

    pub fn synthetic_for_unit(
        event_id: impl Into<String>,
        execution_mode: ExecutionMode,
        unit_id: impl Into<String>,
    ) -> Self {
        let unit = UnitContext::new(unit_id);
        let system = SystemContext::synthetic(SystemOrigin::Manual).with_unit(unit);

        Self::new(event_id, UpdateType::System, execution_mode, system)
    }

    pub fn new(
        event_id: impl Into<String>,
        update_type: UpdateType,
        execution_mode: ExecutionMode,
        system: SystemContext,
    ) -> Self {
        let recovery = matches!(execution_mode, ExecutionMode::Recovery);

        Self {
            event_id: event_id.into(),
            update_id: None,
            update_type,
            received_at: Utc::now(),
            execution_mode,
            recovery,
            chat: None,
            sender: None,
            message: None,
            reply: None,
            callback: None,
            chat_member: None,
            reaction: None,
            job: None,
            system,
        }
    }

    pub fn bind_unit(mut self, unit: UnitContext) -> Self {
        self.system = self.system.with_unit(unit);
        self
    }

    pub fn is_synthetic(&self) -> bool {
        self.system.synthetic
    }

    pub fn command_source(&self) -> Option<CommandSource<'_>> {
        if matches!(self.update_type, UpdateType::CallbackQuery) {
            return self
                .callback
                .as_ref()
                .and_then(|callback| callback.data.as_deref())
                .map(CommandSource::CallbackData);
        }

        if let Some(text) = self
            .message
            .as_ref()
            .and_then(|message| message.text.as_deref())
        {
            return Some(CommandSource::MessageText(text));
        }

        self.callback
            .as_ref()
            .and_then(|callback| callback.data.as_deref())
            .map(CommandSource::CallbackData)
    }

    pub fn author_source_class(&self) -> AuthorSourceClass {
        match self.sender.as_ref() {
            Some(sender) if sender.is_bot => AuthorSourceClass::Bot,
            Some(sender) if sender.is_admin => AuthorSourceClass::HumanAdmin,
            Some(_) => AuthorSourceClass::HumanMember,
            None if self.is_linked_channel_style_approx() => {
                AuthorSourceClass::ChannelStyleNoSender
            }
            None => AuthorSourceClass::Unknown,
        }
    }

    pub fn is_linked_channel_style_approx(&self) -> bool {
        matches!(
            self.update_type,
            UpdateType::ChannelPost | UpdateType::EditedChannelPost
        ) || (matches!(
            self.update_type,
            UpdateType::Message | UpdateType::EditedMessage
        ) && self.sender.is_none()
            && self.message.is_some()
            && self
                .chat
                .as_ref()
                .is_some_and(|chat| chat.route_class() == ChatRouteClass::GroupLike))
    }

    pub fn validate_invariants(&self) -> Result<()> {
        if self.event_id.trim().is_empty() {
            bail!("event_id must not be empty");
        }

        if self.recovery != matches!(self.execution_mode, ExecutionMode::Recovery) {
            bail!("recovery flag must match execution_mode");
        }

        if self.reply.is_some() && self.message.is_none() {
            bail!("reply context requires message context");
        }

        if self.message.is_some() && self.chat.is_none() {
            bail!("message context requires chat context");
        }

        if matches!(self.update_type, UpdateType::CallbackQuery) != self.callback.is_some() {
            bail!("callback context must exist only for callback_query events");
        }

        if matches!(
            self.update_type,
            UpdateType::ChatMember | UpdateType::MyChatMember | UpdateType::ChatMemberUpdated
        ) != self.chat_member.is_some()
        {
            bail!("member context must exist for chat member updates");
        }

        if matches!(
            self.update_type,
            UpdateType::MessageReaction | UpdateType::MessageReactionCount
        ) != self.reaction.is_some()
        {
            bail!("reaction context must exist for reaction updates");
        }

        if matches!(self.update_type, UpdateType::Job) != self.job.is_some() {
            bail!("job context must exist only for job events");
        }

        if self.system.requires_unit_context() && self.system.unit.is_none() {
            bail!("system.unit is required for unit-scoped execution");
        }

        match self.system.origin {
            SystemOrigin::Telegram | SystemOrigin::RecoveryReplay => {
                if self.update_id.is_none() {
                    bail!("telegram-origin events require update_id");
                }
                if self.system.synthetic {
                    bail!("telegram-origin events must not be synthetic");
                }
            }
            SystemOrigin::Scheduler => {
                if self.execution_mode != ExecutionMode::Scheduled {
                    bail!("scheduler-origin events must use scheduled execution_mode");
                }
                if self.update_type != UpdateType::Job {
                    bail!("scheduler-origin events must use job update_type");
                }
                if !self.system.synthetic {
                    bail!("scheduler-origin events must be synthetic");
                }
            }
            SystemOrigin::Manual => {
                if self.execution_mode != ExecutionMode::Manual {
                    bail!("manual-origin events must use manual execution_mode");
                }
                if !self.system.synthetic {
                    bail!("manual-origin events must be synthetic");
                }
            }
            SystemOrigin::Runtime => {
                if !self.system.synthetic {
                    bail!("runtime-origin events must be synthetic");
                }
            }
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum CommandSource<'a> {
    MessageText(&'a str),
    CallbackData(&'a str),
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatRouteClass {
    Private,
    GroupLike,
    Channel,
    Unknown,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthorSourceClass {
    HumanAdmin,
    HumanMember,
    Bot,
    ChannelStyleNoSender,
    Unknown,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct EventNormalizer;

impl Default for EventNormalizer {
    fn default() -> Self {
        Self::new()
    }
}

impl EventNormalizer {
    pub fn new() -> Self {
        Self
    }

    pub fn normalize_manual(
        &self,
        input: ManualInvocationInput,
    ) -> Result<EventContext, EventNormalizationError> {
        if input.command_text.trim().is_empty() {
            return Err(EventNormalizationError::MissingCommandText);
        }
        if input.chat.is_none() {
            return Err(EventNormalizationError::MissingManualChat);
        }

        let mut event = EventContext::new(
            take_event_id(input.event_id, "manual")?,
            UpdateType::System,
            ExecutionMode::Manual,
            SystemContext::synthetic(SystemOrigin::Manual)
                .with_unit(input.unit)
                .with_metadata(input.locale, input.trace_id, input.build),
        );

        event.received_at = input.received_at;
        event.chat = input.chat;
        event.sender = input.sender;
        event.reply = input.reply;
        event.message = Some(
            MessageContext::synthetic_command(input.command_text, input.received_at)
                .with_reply(event.reply.as_ref().map(|reply| reply.message_id)),
        );

        validate_event(event)
    }

    pub fn normalize_scheduled(
        &self,
        input: ScheduledJobInput,
    ) -> Result<EventContext, EventNormalizationError> {
        if input.job_id.trim().is_empty() {
            return Err(EventNormalizationError::MissingJobId);
        }
        if input.reply.is_some() && input.command_text.is_none() {
            return Err(EventNormalizationError::ScheduledReplyRequiresCommandText);
        }
        if input.command_text.is_some() && input.chat.is_none() {
            return Err(EventNormalizationError::ScheduledCommandRequiresChat);
        }

        let mut event = EventContext::new(
            take_event_id(input.event_id, "job")?,
            UpdateType::Job,
            ExecutionMode::Scheduled,
            SystemContext::synthetic(SystemOrigin::Scheduler)
                .with_unit(input.unit)
                .with_metadata(input.locale, input.trace_id, input.build),
        );

        event.received_at = input.received_at;
        event.chat = input.chat;
        event.sender = input.sender;
        event.reply = input.reply;
        event.job = Some(JobContext {
            job_id: input.job_id,
            payload: input.payload,
            scheduled_at: input.scheduled_at,
            run_at: input.run_at,
        });
        event.message = input.command_text.map(|text| {
            MessageContext::synthetic_command(text, event.received_at)
                .with_reply(event.reply.as_ref().map(|reply| reply.message_id))
        });

        validate_event(event)
    }

    pub fn normalize_telegram(
        &self,
        input: TelegramUpdateInput,
    ) -> Result<EventContext, EventNormalizationError> {
        validate_telegram_shape(&input)?;

        let origin = match input.execution_mode {
            ExecutionMode::Realtime => SystemOrigin::Telegram,
            ExecutionMode::Recovery => SystemOrigin::RecoveryReplay,
            other => {
                return Err(EventNormalizationError::UnsupportedTelegramExecutionMode(
                    other,
                ));
            }
        };

        let mut event = EventContext::new(
            take_event_id(input.event_id, "tg")?,
            input.update_type,
            input.execution_mode,
            SystemContext::realtime().with_origin(origin).with_metadata(
                input.locale,
                input.trace_id,
                input.build,
            ),
        );

        event.update_id = Some(input.update_id);
        event.received_at = input.received_at;
        event.chat = Some(input.chat);
        event.sender = input.sender;
        event.message = input.message;
        event.reply = input.reply;
        event.callback = input.callback;
        event.chat_member = input.chat_member;
        event.reaction = input.reaction;

        validate_event(event)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManualInvocationInput {
    pub event_id: Option<String>,
    pub received_at: DateTime<Utc>,
    pub command_text: String,
    pub unit: UnitContext,
    pub chat: Option<ChatContext>,
    pub sender: Option<SenderContext>,
    pub reply: Option<ReplyContext>,
    pub chat_member: Option<MemberContext>,
    pub reaction: Option<ReactionContext>,
    pub locale: Option<String>,
    pub trace_id: Option<String>,
    pub build: Option<String>,
}

impl ManualInvocationInput {
    pub fn new(unit: UnitContext, command_text: impl Into<String>) -> Self {
        Self {
            event_id: None,
            received_at: Utc::now(),
            command_text: command_text.into(),
            unit,
            chat: None,
            sender: None,
            reply: None,
            chat_member: None,
            reaction: None,
            locale: None,
            trace_id: None,
            build: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledJobInput {
    pub event_id: Option<String>,
    pub received_at: DateTime<Utc>,
    pub job_id: String,
    pub payload: Value,
    pub scheduled_at: DateTime<Utc>,
    pub run_at: DateTime<Utc>,
    pub command_text: Option<String>,
    pub unit: UnitContext,
    pub chat: Option<ChatContext>,
    pub sender: Option<SenderContext>,
    pub reply: Option<ReplyContext>,
    pub chat_member: Option<MemberContext>,
    pub reaction: Option<ReactionContext>,
    pub locale: Option<String>,
    pub trace_id: Option<String>,
    pub build: Option<String>,
}

impl ScheduledJobInput {
    pub fn new(
        job_id: impl Into<String>,
        unit: UnitContext,
        payload: Value,
        scheduled_at: DateTime<Utc>,
        run_at: DateTime<Utc>,
    ) -> Self {
        Self {
            event_id: None,
            received_at: Utc::now(),
            job_id: job_id.into(),
            payload,
            scheduled_at,
            run_at,
            command_text: None,
            unit,
            chat: None,
            sender: None,
            reply: None,
            chat_member: None,
            reaction: None,
            locale: None,
            trace_id: None,
            build: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelegramUpdateInput {
    pub event_id: Option<String>,
    pub update_id: u64,
    pub update_type: UpdateType,
    pub received_at: DateTime<Utc>,
    pub execution_mode: ExecutionMode,
    pub chat: ChatContext,
    pub sender: Option<SenderContext>,
    pub message: Option<MessageContext>,
    pub reply: Option<ReplyContext>,
    pub callback: Option<CallbackContext>,
    pub chat_member: Option<MemberContext>,
    pub reaction: Option<ReactionContext>,
    pub locale: Option<String>,
    pub trace_id: Option<String>,
    pub build: Option<String>,
}

impl TelegramUpdateInput {
    pub fn message(
        update_id: u64,
        chat: ChatContext,
        sender: SenderContext,
        message: MessageContext,
    ) -> Self {
        Self {
            event_id: None,
            update_id,
            update_type: UpdateType::Message,
            received_at: Utc::now(),
            execution_mode: ExecutionMode::Realtime,
            chat,
            sender: Some(sender),
            message: Some(message),
            reply: None,
            callback: None,
            chat_member: None,
            reaction: None,
            locale: None,
            trace_id: None,
            build: None,
        }
    }
}

#[derive(Debug, Error)]
pub enum EventNormalizationError {
    #[error("event_id must not be empty")]
    EmptyEventId,
    #[error("manual invocation requires non-empty command text")]
    MissingCommandText,
    #[error("manual invocation requires chat context for synthetic command events")]
    MissingManualChat,
    #[error("scheduled invocation requires non-empty job_id")]
    MissingJobId,
    #[error("scheduled invocation reply context requires command_text")]
    ScheduledReplyRequiresCommandText,
    #[error("scheduled invocation command_text requires chat context")]
    ScheduledCommandRequiresChat,
    #[error("telegram update missing required message context")]
    MissingTelegramMessage,
    #[error("telegram callback update missing required callback context")]
    MissingTelegramCallback,
    #[error("telegram update_type `{0:?}` is not supported by the normalization layer yet")]
    UnsupportedTelegramUpdateType(UpdateType),
    #[error("telegram execution_mode `{0:?}` is not supported by the normalization layer yet")]
    UnsupportedTelegramExecutionMode(ExecutionMode),
    #[error(transparent)]
    InvalidEvent(#[from] anyhow::Error),
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateType {
    Message,
    EditedMessage,
    ChannelPost,
    EditedChannelPost,
    CallbackQuery,
    ChatMember,
    MyChatMember,
    ChatMemberUpdated,
    MessageReaction,
    MessageReactionCount,
    JoinRequest,
    Job,
    System,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionMode {
    Realtime,
    Recovery,
    Scheduled,
    Manual,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatContext {
    pub id: i64,
    #[serde(rename = "type")]
    pub chat_type: String,
    pub title: Option<String>,
    pub username: Option<String>,
    pub photo_file_id: Option<String>,
    pub thread_id: Option<i64>,
}

impl ChatContext {
    pub fn route_class(&self) -> ChatRouteClass {
        match self.chat_type.as_str() {
            "private" => ChatRouteClass::Private,
            "group" | "supergroup" => ChatRouteClass::GroupLike,
            "channel" => ChatRouteClass::Channel,
            _ => ChatRouteClass::Unknown,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SenderContext {
    pub id: i64,
    pub username: Option<String>,
    pub display_name: Option<String>,
    pub first_name: String,
    pub last_name: Option<String>,
    pub photo_file_id: Option<String>,
    pub is_bot: bool,
    pub is_admin: bool,
    pub role: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemberContext {
    pub old_status: String,
    pub new_status: String,
    pub user: SenderContext,
}

impl MemberContext {
    pub fn is_joined(&self) -> bool {
        matches!(self.old_status.as_str(), "Left" | "Kicked")
            && matches!(
                self.new_status.as_str(),
                "Member" | "Administrator" | "Owner" | "Restricted"
            )
    }

    pub fn is_left(&self) -> bool {
        matches!(
            self.old_status.as_str(),
            "Member" | "Administrator" | "Owner" | "Restricted"
        ) && matches!(self.new_status.as_str(), "Left" | "Kicked")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReactionContext {
    pub message_id: i32,
    pub old_reaction: Vec<String>,
    pub new_reaction: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageContext {
    pub id: i32,
    pub date: DateTime<Utc>,
    pub text: Option<String>,
    pub content_kind: Option<MessageContentKind>,
    pub entities: Vec<String>,
    pub has_media: bool,
    pub file_ids: Vec<String>,
    pub reply_to_message_id: Option<i32>,
    pub media_group_id: Option<String>,
}

impl MessageContext {
    pub fn synthetic_command(text: impl Into<String>, at: DateTime<Utc>) -> Self {
        Self {
            id: 0,
            date: at,
            text: Some(text.into()),
            content_kind: None,
            entities: Vec::new(),
            has_media: false,
            file_ids: Vec::new(),
            reply_to_message_id: None,
            media_group_id: None,
        }
    }

    pub fn with_reply(mut self, reply_to_message_id: Option<i32>) -> Self {
        self.reply_to_message_id = reply_to_message_id;
        self
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageContentKind {
    Text,
    Photo,
    Voice,
    Video,
    Audio,
    Document,
    Sticker,
    Animation,
    VideoNote,
    Contact,
    Location,
    Poll,
    Dice,
    Venue,
    Game,
    Invoice,
    Story,
    UnknownMedia,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplyContext {
    pub message_id: i32,
    pub sender_user_id: Option<i64>,
    pub sender_username: Option<String>,
    pub text: Option<String>,
    pub has_media: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallbackContext {
    pub query_id: String,
    pub data: Option<String>,
    pub message_id: Option<i32>,
    pub origin_chat_id: Option<i64>,
    pub from_user_id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobContext {
    pub job_id: String,
    pub payload: Value,
    pub scheduled_at: DateTime<Utc>,
    pub run_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemContext {
    pub locale: Option<String>,
    pub unit: Option<UnitContext>,
    pub trace_id: Option<String>,
    pub build: Option<String>,
    pub origin: SystemOrigin,
    pub synthetic: bool,
}

impl SystemContext {
    pub fn runtime_bootstrap() -> Self {
        Self {
            locale: None,
            unit: None,
            trace_id: None,
            build: None,
            origin: SystemOrigin::Runtime,
            synthetic: true,
        }
    }

    pub fn synthetic(origin: SystemOrigin) -> Self {
        Self {
            locale: None,
            unit: None,
            trace_id: None,
            build: None,
            origin,
            synthetic: true,
        }
    }

    pub fn realtime() -> Self {
        Self {
            locale: None,
            unit: None,
            trace_id: None,
            build: None,
            origin: SystemOrigin::Telegram,
            synthetic: false,
        }
    }

    pub fn with_unit(mut self, unit: UnitContext) -> Self {
        self.unit = Some(unit);
        self
    }

    pub fn with_metadata(
        mut self,
        locale: Option<String>,
        trace_id: Option<String>,
        build: Option<String>,
    ) -> Self {
        self.locale = locale;
        self.trace_id = trace_id;
        self.build = build;
        self
    }

    pub fn with_origin(mut self, origin: SystemOrigin) -> Self {
        self.origin = origin;
        self
    }

    fn requires_unit_context(&self) -> bool {
        matches!(
            self.origin,
            SystemOrigin::Manual | SystemOrigin::Scheduler | SystemOrigin::RecoveryReplay
        )
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SystemOrigin {
    Telegram,
    Scheduler,
    Manual,
    RecoveryReplay,
    Runtime,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnitContext {
    pub id: String,
    pub version: Option<String>,
    pub trigger: Option<String>,
}

impl UnitContext {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            version: None,
            trigger: None,
        }
    }

    pub fn with_trigger(mut self, trigger: impl Into<String>) -> Self {
        self.trigger = Some(trigger.into());
        self
    }
}

fn take_event_id(
    event_id: Option<String>,
    prefix: &'static str,
) -> Result<String, EventNormalizationError> {
    match event_id {
        Some(value) if value.trim().is_empty() => Err(EventNormalizationError::EmptyEventId),
        Some(value) => Ok(value),
        None => Ok(format!("evt_{prefix}_{}", Uuid::new_v4().simple())),
    }
}

fn validate_event(event: EventContext) -> Result<EventContext, EventNormalizationError> {
    event.validate_invariants()?;
    Ok(event)
}

fn validate_telegram_shape(input: &TelegramUpdateInput) -> Result<(), EventNormalizationError> {
    match input.update_type {
        UpdateType::Message
        | UpdateType::EditedMessage
        | UpdateType::ChannelPost
        | UpdateType::EditedChannelPost => {
            if input.message.is_none() {
                return Err(EventNormalizationError::MissingTelegramMessage);
            }
            if input.callback.is_some() {
                return Err(EventNormalizationError::InvalidEvent(anyhow::anyhow!(
                    "message-style telegram updates must not include callback context"
                )));
            }
        }
        UpdateType::CallbackQuery => {
            if input.callback.is_none() {
                return Err(EventNormalizationError::MissingTelegramCallback);
            }
        }
        UpdateType::ChatMember
        | UpdateType::MyChatMember
        | UpdateType::ChatMemberUpdated
        | UpdateType::JoinRequest => {
            if input.message.is_some() {
                return Err(EventNormalizationError::InvalidEvent(anyhow::anyhow!(
                    "chat-member-style telegram updates must not include message context"
                )));
            }
            if input.callback.is_some() {
                return Err(EventNormalizationError::InvalidEvent(anyhow::anyhow!(
                    "chat-member-style telegram updates must not include callback context"
                )));
            }
        }
        UpdateType::MessageReaction | UpdateType::MessageReactionCount => {
            if input.message.is_some() {
                return Err(EventNormalizationError::InvalidEvent(anyhow::anyhow!(
                    "reaction-style telegram updates must not include message context"
                )));
            }
        }
        other => {
            return Err(EventNormalizationError::UnsupportedTelegramUpdateType(
                other,
            ));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests;
