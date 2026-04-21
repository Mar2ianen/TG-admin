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

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct EventNormalizer;

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
    #[error("scheduled invocation requires non-empty job_id")]
    MissingJobId,
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
    pub thread_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SenderContext {
    pub id: i64,
    pub username: Option<String>,
    pub display_name: Option<String>,
    pub is_bot: bool,
    pub is_admin: bool,
    pub role: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageContext {
    pub id: i32,
    pub date: DateTime<Utc>,
    pub text: Option<String>,
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
        other => {
            return Err(EventNormalizationError::UnsupportedTelegramUpdateType(
                other,
            ));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        CallbackContext, ChatContext, CommandSource, EventContext, EventNormalizationError,
        EventNormalizer, ExecutionMode, ManualInvocationInput, MessageContext, ScheduledJobInput,
        SenderContext, SystemContext, SystemOrigin, TelegramUpdateInput, UnitContext, UpdateType,
    };
    use chrono::{TimeZone, Utc};
    use serde_json::json;

    fn ts() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 4, 21, 9, 30, 0)
            .single()
            .expect("valid timestamp")
    }

    fn chat() -> ChatContext {
        ChatContext {
            id: -100123,
            chat_type: "supergroup".to_owned(),
            title: Some("Moderation HQ".to_owned()),
            username: Some("mod_hq".to_owned()),
            thread_id: Some(11),
        }
    }

    fn sender() -> SenderContext {
        SenderContext {
            id: 42,
            username: Some("admin".to_owned()),
            display_name: Some("Admin".to_owned()),
            is_bot: false,
            is_admin: true,
            role: Some("owner".to_owned()),
        }
    }

    fn message(text: &str) -> MessageContext {
        MessageContext {
            id: 777,
            date: ts(),
            text: Some(text.to_owned()),
            entities: vec!["bot_command".to_owned()],
            has_media: false,
            file_ids: Vec::new(),
            reply_to_message_id: None,
            media_group_id: Some("grp-1".to_owned()),
        }
    }

    #[test]
    fn system_event_uses_synthetic_runtime_context() {
        let event = EventContext::system_event();

        assert_eq!(event.update_type, UpdateType::System);
        assert_eq!(event.execution_mode, ExecutionMode::Manual);
        assert!(event.is_synthetic());
        assert_eq!(event.system.origin, SystemOrigin::Runtime);
        assert!(event.system.unit.is_none());
        event.validate_invariants().expect("valid system event");
    }

    #[test]
    fn synthetic_unit_context_satisfies_manual_invariants() {
        let event = EventContext::synthetic_for_unit(
            "evt_manual_warn",
            ExecutionMode::Manual,
            "moderation.warn",
        );

        assert_eq!(event.system.origin, SystemOrigin::Manual);
        assert_eq!(
            event.system.unit.as_ref().map(|unit| unit.id.as_str()),
            Some("moderation.warn")
        );
        event.validate_invariants().expect("valid unit event");
    }

    #[test]
    fn manual_origin_requires_unit_context() {
        let event = EventContext::new(
            "evt_invalid",
            UpdateType::System,
            ExecutionMode::Manual,
            SystemContext::synthetic(SystemOrigin::Manual),
        );

        let err = event
            .validate_invariants()
            .expect_err("missing unit must fail");
        assert!(err.to_string().contains("system.unit"));
    }

    #[test]
    fn recovery_flag_must_match_execution_mode() {
        let mut event = EventContext::new(
            "evt_replay",
            UpdateType::Message,
            ExecutionMode::Recovery,
            SystemContext::realtime()
                .with_origin(SystemOrigin::RecoveryReplay)
                .with_unit(UnitContext::new("moderation.mute")),
        );
        event.update_id = Some(9);
        event.recovery = false;

        let err = event
            .validate_invariants()
            .expect_err("recovery mismatch must fail");
        assert!(err.to_string().contains("recovery flag"));
    }

    #[test]
    fn normalizes_manual_invocation_into_synthetic_command_event() {
        let normalizer = EventNormalizer::new();
        let mut input = ManualInvocationInput::new(
            UnitContext::new("moderation.warn").with_trigger("manual"),
            "/warn @spam rule:spam",
        );
        input.received_at = ts();
        input.chat = Some(chat());
        input.sender = Some(sender());

        let event = normalizer
            .normalize_manual(input)
            .expect("manual normalization must succeed");

        assert_eq!(event.update_type, UpdateType::System);
        assert_eq!(event.execution_mode, ExecutionMode::Manual);
        assert!(event.is_synthetic());
        assert_eq!(event.system.origin, SystemOrigin::Manual);
        assert_eq!(
            event.command_source(),
            Some(CommandSource::MessageText("/warn @spam rule:spam"))
        );
        assert_eq!(event.message.as_ref().map(|message| message.id), Some(0));
        event
            .validate_invariants()
            .expect("manual event must stay valid");
    }

    #[test]
    fn manual_normalization_snapshot_shape_stays_stable() {
        let normalizer = EventNormalizer::new();
        let mut input = ManualInvocationInput::new(
            UnitContext::new("moderation.warn").with_trigger("manual"),
            "/warn @spam spam",
        );
        input.event_id = Some("evt_manual_snapshot".to_owned());
        input.received_at = ts();
        input.chat = Some(chat());
        input.sender = Some(sender());
        input.locale = Some("ru".to_owned());
        input.trace_id = Some("trace-manual".to_owned());
        input.build = Some("dev".to_owned());

        let event = normalizer
            .normalize_manual(input)
            .expect("manual normalization must succeed");

        let snapshot = serde_json::to_string_pretty(&event).expect("event serializes");
        assert_eq!(
            snapshot,
            r#"{
  "event_id": "evt_manual_snapshot",
  "update_id": null,
  "update_type": "system",
  "received_at": "2026-04-21T09:30:00Z",
  "execution_mode": "manual",
  "recovery": false,
  "chat": {
    "id": -100123,
    "type": "supergroup",
    "title": "Moderation HQ",
    "username": "mod_hq",
    "thread_id": 11
  },
  "sender": {
    "id": 42,
    "username": "admin",
    "display_name": "Admin",
    "is_bot": false,
    "is_admin": true,
    "role": "owner"
  },
  "message": {
    "id": 0,
    "date": "2026-04-21T09:30:00Z",
    "text": "/warn @spam spam",
    "entities": [],
    "has_media": false,
    "file_ids": [],
    "reply_to_message_id": null,
    "media_group_id": null
  },
  "reply": null,
  "callback": null,
  "job": null,
  "system": {
    "locale": "ru",
    "unit": {
      "id": "moderation.warn",
      "version": null,
      "trigger": "manual"
    },
    "trace_id": "trace-manual",
    "build": "dev",
    "origin": "manual",
    "synthetic": true
  }
}"#
        );
    }

    #[test]
    fn normalizes_scheduled_job_into_job_event_with_optional_command_payload() {
        let normalizer = EventNormalizer::new();
        let mut input = ScheduledJobInput::new(
            "job_123",
            UnitContext::new("moderation.mute").with_trigger("schedule"),
            json!({ "kind": "mute_recheck", "target": 42 }),
            ts(),
            ts(),
        );
        input.received_at = ts();
        input.chat = Some(chat());
        input.command_text = Some("/mute @spam 1h rule:repeat".to_owned());

        let event = normalizer
            .normalize_scheduled(input)
            .expect("scheduled normalization must succeed");

        assert_eq!(event.update_type, UpdateType::Job);
        assert_eq!(event.execution_mode, ExecutionMode::Scheduled);
        assert!(event.is_synthetic());
        assert_eq!(event.system.origin, SystemOrigin::Scheduler);
        assert_eq!(
            event.job.as_ref().map(|job| job.job_id.as_str()),
            Some("job_123")
        );
        assert_eq!(
            event.command_source(),
            Some(CommandSource::MessageText("/mute @spam 1h rule:repeat"))
        );
        event
            .validate_invariants()
            .expect("scheduled event must stay valid");
    }

    #[test]
    fn normalizes_basic_telegram_message_into_realtime_event() {
        let normalizer = EventNormalizer::new();
        let mut input = TelegramUpdateInput::message(1001, chat(), sender(), message("/del -up 2"));
        input.received_at = ts();

        let event = normalizer
            .normalize_telegram(input)
            .expect("telegram normalization must succeed");

        assert_eq!(event.update_id, Some(1001));
        assert_eq!(event.update_type, UpdateType::Message);
        assert_eq!(event.execution_mode, ExecutionMode::Realtime);
        assert!(!event.is_synthetic());
        assert_eq!(event.system.origin, SystemOrigin::Telegram);
        assert_eq!(
            event.command_source(),
            Some(CommandSource::MessageText("/del -up 2"))
        );
        assert_eq!(
            event
                .message
                .as_ref()
                .and_then(|message| message.media_group_id.as_deref()),
            Some("grp-1")
        );
        assert_eq!(
            event.chat.as_ref().and_then(|chat| chat.thread_id),
            Some(11)
        );
        event
            .validate_invariants()
            .expect("telegram event must stay valid");
    }

    #[test]
    fn rejects_telegram_message_shape_without_message_context() {
        let normalizer = EventNormalizer::new();
        let input = TelegramUpdateInput {
            event_id: None,
            update_id: 7,
            update_type: UpdateType::Message,
            received_at: ts(),
            execution_mode: ExecutionMode::Realtime,
            chat: chat(),
            sender: Some(sender()),
            message: None,
            reply: None,
            callback: None,
            locale: None,
            trace_id: None,
            build: None,
        };

        let err = normalizer
            .normalize_telegram(input)
            .expect_err("invalid telegram input must fail");
        assert!(matches!(
            err,
            EventNormalizationError::MissingTelegramMessage
        ));
    }

    #[test]
    fn callback_updates_prefer_callback_data_as_command_source() {
        let normalizer = EventNormalizer::new();
        let input = TelegramUpdateInput {
            event_id: Some("evt_callback_source".to_owned()),
            update_id: 8,
            update_type: UpdateType::CallbackQuery,
            received_at: ts(),
            execution_mode: ExecutionMode::Realtime,
            chat: chat(),
            sender: Some(sender()),
            message: Some(message("button label")),
            reply: None,
            callback: Some(CallbackContext {
                query_id: "cbq-1".to_owned(),
                data: Some("/undo -dry".to_owned()),
                message_id: Some(777),
                origin_chat_id: Some(-100123),
                from_user_id: 42,
            }),
            locale: None,
            trace_id: None,
            build: None,
        };

        let event = normalizer
            .normalize_telegram(input)
            .expect("callback normalization must succeed");

        assert_eq!(
            event.command_source(),
            Some(CommandSource::CallbackData("/undo -dry"))
        );
    }
}
