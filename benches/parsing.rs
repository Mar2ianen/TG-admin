use chrono::Utc;
use criterion::{Criterion, black_box, criterion_group, criterion_main};
use telegram_moderation_os::event::{
    ChatContext, EventNormalizer, ExecutionMode, MessageContentKind, MessageContext, SenderContext,
    SystemContext, UpdateType,
};
use telegram_moderation_os::parser::command::CommandParser;
use telegram_moderation_os::parser::dispatch::EventCommandDispatcher;
use telegram_moderation_os::parser::duration::DurationParser;
use telegram_moderation_os::parser::target::TargetSelectorParser;
use telegram_moderation_os::unit::UnitManifest;

fn bench_duration_parse(c: &mut Criterion) {
    c.bench_function("duration_parse/7d", |b| {
        b.iter(|| DurationParser::new().parse(black_box("7d")));
    });
}

fn bench_target_parse(c: &mut Criterion) {
    c.bench_function("target_parse/username", |b| {
        b.iter(|| TargetSelectorParser::new().parse(black_box("@username")));
    });

    c.bench_function("target_parse/user_id", |b| {
        b.iter(|| TargetSelectorParser::new().parse(black_box("12345")));
    });
}

fn bench_command_parse(c: &mut Criterion) {
    let event = dummy_event();
    let parser = CommandParser::new();

    c.bench_function("command_parse/warn", |b| {
        b.iter(|| parser.parse(black_box("/warn @user 2.8"), &event));
    });

    c.bench_function("command_parse/mute", |b| {
        b.iter(|| parser.parse(black_box("/mute @user 7d spam"), &event));
    });
}

fn bench_event_normalize(c: &mut Criterion) {
    let normalizer = EventNormalizer::new();
    let chat = ChatContext {
        id: -100123,
        chat_type: "supergroup".to_owned(),
        title: Some("Test".to_owned()),
        username: Some("test".to_owned()),
        thread_id: None,
    };
    let sender = SenderContext {
        id: 42,
        username: Some("user".to_owned()),
        display_name: Some("User".to_owned()),
        is_bot: false,
        is_admin: false,
        role: None,
    };
    let msg = MessageContext {
        id: 777,
        date: Utc::now(),
        text: Some("/warn @bad 2.8".to_owned()),
        content_kind: Some(MessageContentKind::Text),
        entities: vec!["bot_command".to_owned()],
        has_media: false,
        file_ids: vec![],
        reply_to_message_id: None,
        media_group_id: None,
    };

    c.bench_function("event_normalize/telegram", |b| {
        b.iter(|| {
            let input = telegram_moderation_os::event::TelegramUpdateInput::message(
                999,
                chat.clone(),
                sender.clone(),
                msg.clone(),
            );
            let _ = normalizer.normalize_telegram(input);
        });
    });
}

fn bench_unit_parse(c: &mut Criterion) {
    let manifest = r#"
[Unit]
Name = "test.unit"
Description = "Test unit"

[Trigger]
Type = "command"
Commands = ["test"]

[Service]
ExecStart = "scripts/test.rhai"
TimeoutSec = 30
"#;

    c.bench_function("unit_parse/toml", |b| {
        b.iter(|| UnitManifest::from_toml_str(black_box(manifest)));
    });

    c.bench_function("unit_validate/validate", |b| {
        let m = UnitManifest::from_toml_str(black_box(manifest)).unwrap();
        b.iter(|| m.validate());
    });
}

fn bench_dispatch(c: &mut Criterion) {
    let dispatcher = EventCommandDispatcher::new();
    let event = dummy_event();

    c.bench_function("dispatch/warn", |b| {
        b.iter(|| dispatcher.dispatch(black_box(&event)));
    });
}

fn bench_json(c: &mut Criterion) {
    let event = dummy_event();

    c.bench_function("json/serialize", |b| {
        b.iter(|| serde_json::to_string(black_box(&event)));
    });
}

fn dummy_event() -> telegram_moderation_os::event::EventContext {
    let mut event = telegram_moderation_os::event::EventContext::new(
        "evt_bench",
        UpdateType::Message,
        ExecutionMode::Realtime,
        SystemContext::realtime(),
    );
    event.message = Some(MessageContext {
        id: 101,
        date: Utc::now(),
        text: Some("/warn @user 2.8".to_owned()),
        content_kind: Some(MessageContentKind::Text),
        entities: vec!["bot_command".to_owned()],
        has_media: false,
        file_ids: vec![],
        reply_to_message_id: None,
        media_group_id: None,
    });
    event.chat = Some(ChatContext {
        id: -100123,
        chat_type: "supergroup".to_owned(),
        title: Some("Bench".to_owned()),
        username: Some("bench".to_owned()),
        thread_id: None,
    });
    event.sender = Some(SenderContext {
        id: 42,
        username: Some("user".to_owned()),
        display_name: Some("User".to_owned()),
        is_bot: false,
        is_admin: false,
        role: None,
    });
    event
}

criterion_group!(
    benches,
    bench_duration_parse,
    bench_target_parse,
    bench_command_parse,
    bench_event_normalize,
    bench_unit_parse,
    bench_dispatch,
    bench_json
);
criterion_main!(benches);
