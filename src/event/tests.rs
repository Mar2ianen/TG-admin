use super::{
    CallbackContext, ChatContext, CommandSource, EventContext, EventNormalizationError,
    EventNormalizer, ExecutionMode, ManualInvocationInput, MessageContentKind, MessageContext,
    ReplyContext, ScheduledJobInput, SenderContext, SystemContext, SystemOrigin,
    TelegramUpdateInput, UnitContext, UpdateType,
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
        photo_file_id: None,
        thread_id: Some(11),
    }
}

fn sender() -> SenderContext {
    SenderContext {
        id: 42,
        username: Some("admin".to_owned()),
        display_name: Some("Admin".to_owned()),
        first_name: "Admin".to_owned(),
        last_name: None,
        photo_file_id: None,
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
        content_kind: Some(MessageContentKind::Text),
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
    "content_kind": null,
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
fn manual_normalization_rejects_missing_chat() {
    let normalizer = EventNormalizer::new();
    let input = ManualInvocationInput::new(UnitContext::new("moderation.warn"), "/warn @spam");

    let error = normalizer
        .normalize_manual(input)
        .expect_err("manual normalization must reject missing chat");
    assert!(matches!(error, EventNormalizationError::MissingManualChat));
}

#[test]
fn scheduled_normalization_rejects_reply_without_command_text() {
    let normalizer = EventNormalizer::new();
    let mut input = ScheduledJobInput::new(
        "job_123",
        UnitContext::new("moderation.mute").with_trigger("schedule"),
        json!({ "kind": "mute_recheck" }),
        ts(),
        ts(),
    );
    input.reply = Some(ReplyContext {
        message_id: 777,
        sender_user_id: Some(42),
        sender_username: Some("admin".to_owned()),
        text: Some("anchor".to_owned()),
        has_media: false,
    });

    let error = normalizer
        .normalize_scheduled(input)
        .expect_err("scheduled normalization must reject reply without command");
    assert!(matches!(
        error,
        EventNormalizationError::ScheduledReplyRequiresCommandText
    ));
}

#[test]
fn scheduled_normalization_rejects_command_text_without_chat() {
    let normalizer = EventNormalizer::new();
    let mut input = ScheduledJobInput::new(
        "job_123",
        UnitContext::new("moderation.mute").with_trigger("schedule"),
        json!({ "kind": "mute_recheck" }),
        ts(),
        ts(),
    );
    input.command_text = Some("/mute @spam 1h".to_owned());

    let error = normalizer
        .normalize_scheduled(input)
        .expect_err("scheduled normalization must reject missing chat");
    assert!(matches!(
        error,
        EventNormalizationError::ScheduledCommandRequiresChat
    ));
}

#[test]
fn scheduled_normalization_with_reply_and_command_text_copies_reply_to_message_id() {
    let normalizer = EventNormalizer::new();
    let mut input = ScheduledJobInput::new(
        "job_123",
        UnitContext::new("moderation.mute").with_trigger("schedule"),
        json!({ "kind": "mute_recheck" }),
        ts(),
        ts(),
    );
    input.chat = Some(chat());
    input.reply = Some(ReplyContext {
        message_id: 777,
        sender_user_id: Some(42),
        sender_username: Some("admin".to_owned()),
        text: Some("anchor".to_owned()),
        has_media: false,
    });
    input.command_text = Some("/mute @spam 1h".to_owned());

    let event = normalizer
        .normalize_scheduled(input)
        .expect("scheduled normalization must succeed");

    assert_eq!(
        event
            .message
            .as_ref()
            .and_then(|message| message.reply_to_message_id),
        Some(777)
    );
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
        chat_member: None,
        reaction: None,
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
        chat_member: None,
        reaction: None,
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

#[test]
fn chat_member_updates_normalize_without_message_or_callback_context() {
    let normalizer = EventNormalizer::new();
    let input = TelegramUpdateInput {
        event_id: Some("evt_chat_member".to_owned()),
        update_id: 9,
        update_type: UpdateType::ChatMember,
        received_at: ts(),
        execution_mode: ExecutionMode::Realtime,
        chat: chat(),
        sender: Some(sender()),
        message: None,
        reply: None,
        callback: None,
        chat_member: None,
        reaction: None,
        locale: Some("en".to_owned()),
        trace_id: None,
        build: None,
    };

    let event = normalizer
        .normalize_telegram(input)
        .expect("chat member normalization must succeed");

    assert_eq!(event.update_type, UpdateType::ChatMember);
    assert_eq!(event.system.origin, SystemOrigin::Telegram);
    assert_eq!(event.chat.as_ref().map(|chat| chat.id), Some(-100123));
    assert_eq!(event.sender.as_ref().map(|sender| sender.id), Some(42));
    assert!(event.message.is_none());
    assert!(event.callback.is_none());
}

#[test]
fn join_request_updates_normalize_without_message_or_callback_context() {
    let normalizer = EventNormalizer::new();
    let input = TelegramUpdateInput {
        event_id: Some("evt_join_request".to_owned()),
        update_id: 10,
        update_type: UpdateType::JoinRequest,
        received_at: ts(),
        execution_mode: ExecutionMode::Realtime,
        chat: chat(),
        sender: Some(sender()),
        message: None,
        reply: None,
        callback: None,
        chat_member: None,
        reaction: None,
        locale: Some("ru".to_owned()),
        trace_id: None,
        build: None,
    };

    let event = normalizer
        .normalize_telegram(input)
        .expect("join request normalization must succeed");

    assert_eq!(event.update_type, UpdateType::JoinRequest);
    assert_eq!(event.system.origin, SystemOrigin::Telegram);
    assert_eq!(event.system.locale.as_deref(), Some("ru"));
    assert!(event.message.is_none());
    assert!(event.callback.is_none());
}
