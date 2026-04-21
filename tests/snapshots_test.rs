use chrono::Utc;
use std::fs;
use telegram_moderation_os::event::{
    ChatContext, EventNormalizer, ExecutionMode, MessageContext, SenderContext, SystemContext,
    UpdateType,
};
use telegram_moderation_os::parser::command::CommandParser;
use telegram_moderation_os::parser::dispatch::EventCommandDispatcher;
use telegram_moderation_os::parser::reason::{ReasonAliasDefinition, ReasonAliasRegistry};
use telegram_moderation_os::unit::UnitManifest;

fn realtime_event() -> telegram_moderation_os::event::EventContext {
    let mut event = telegram_moderation_os::event::EventContext::new(
        "evt_snapshots",
        UpdateType::Message,
        ExecutionMode::Realtime,
        SystemContext::realtime(),
    );
    event.message = Some(MessageContext {
        id: 101,
        date: Utc::now(),
        text: Some("/cmd".to_owned()),
        entities: vec!["bot_command".to_owned()],
        has_media: false,
        file_ids: vec![],
        reply_to_message_id: Some(77),
        media_group_id: None,
    });
    event
}

fn snapshots_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(std::env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("snapshots")
}

#[test]
fn command_warn_snapshot_matches_golden() {
    let event = realtime_event();
    let parser = CommandParser::new();

    let parsed = parser
        .parse("/warn @spam_user 2.8", &event)
        .expect("should parse");

    let actual_value: serde_json::Value =
        serde_json::from_str(&serde_json::to_string_pretty(&parsed).expect("should serialize"))
            .expect("should parse");

    let golden_path = snapshots_dir().join("command__warn.json");
    let golden: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&golden_path).expect("should read golden file"))
            .expect("should parse golden");

    assert_eq!(actual_value, golden, "snapshot mismatch");
}

#[test]
fn command_mute_with_flags_snapshot() {
    let event = realtime_event();
    let parser = CommandParser::new();

    let parsed = parser
        .parse("/mute @bad_user 7d spam -s -dry", &event)
        .expect("should parse");

    let actual = serde_json::to_string_pretty(&parsed).expect("should serialize");

    println!("Actual output:\n{}", actual);
}

#[test]
fn unit_manifest_snapshot() {
    let manifest = UnitManifest::from_toml_str(
        r#"
[Unit]
Name = "snapshot.unit"
Description = "Test unit for snapshot"
After = ["dep.unit"]
Requires = ["req.unit"]
Enabled = true
Tags = ["test", "snapshot"]
Owner = "admin"
Version = "1.0.0"

[Trigger]
Type = "command"
Commands = ["test", "run"]

[Service]
ExecStart = "scripts/test.rhai"
EntryPoint = "main"
TimeoutSec = 30
Restart = "on-failure"
RestartSec = 5
MaxRetries = 3
OnFailure = "alert.unit"

[Capabilities]
Allow = ["tg.read_basic", "tg.write_message"]

[Runtime]
MaxMemoryKb = 65536
DryRunSupported = true
AllowManualInvoke = true
"#,
    )
    .expect("should parse");

    let actual = serde_json::to_string_pretty(&manifest).expect("should serialize");

    println!("Unit manifest snapshot:\n{}", actual);
}

#[test]
fn event_context_snapshot() {
    let normalizer = EventNormalizer::new();

    let input = telegram_moderation_os::event::TelegramUpdateInput::message(
        999,
        ChatContext {
            id: -100123,
            chat_type: "supergroup".to_owned(),
            title: Some("Moderation HQ".to_owned()),
            username: Some("mod_hq".to_owned()),
            thread_id: Some(11),
        },
        SenderContext {
            id: 42,
            username: Some("admin".to_owned()),
            display_name: Some("Admin".to_owned()),
            is_bot: false,
            is_admin: true,
            role: Some("owner".to_owned()),
        },
        MessageContext {
            id: 777,
            date: Utc::now(),
            text: Some("/warn @spam 2.8".to_owned()),
            entities: vec!["bot_command".to_owned()],
            has_media: false,
            file_ids: vec![],
            reply_to_message_id: None,
            media_group_id: None,
        },
    );

    let event = normalizer
        .normalize_telegram(input)
        .expect("should normalize");

    let actual = serde_json::to_string_pretty(&event).expect("should serialize");

    println!("Event context snapshot:\n{}", actual);
}

#[test]
fn dispatch_result_with_aliases_snapshot() {
    let normalizer = EventNormalizer::new();
    let mut aliases = ReasonAliasRegistry::new();

    aliases.insert(
        "spam",
        ReasonAliasDefinition::new("spam or scam promotion")
            .with_rule_code("2.8")
            .with_title("Spam"),
    );

    let dispatcher = EventCommandDispatcher::with_aliases(aliases);

    let mut input = telegram_moderation_os::event::ManualInvocationInput::new(
        telegram_moderation_os::event::UnitContext::new("moderation.warn").with_trigger("manual"),
        "/warn @spam spam -s",
    );
    input.received_at = Utc::now();
    input.chat = Some(ChatContext {
        id: -100123,
        chat_type: "supergroup".to_owned(),
        title: Some("Moderation HQ".to_owned()),
        username: Some("mod_hq".to_owned()),
        thread_id: None,
    });
    input.sender = Some(SenderContext {
        id: 42,
        username: Some("admin".to_owned()),
        display_name: Some("Admin".to_owned()),
        is_bot: false,
        is_admin: true,
        role: Some("owner".to_owned()),
    });

    let event = normalizer
        .normalize_manual(input)
        .expect("should normalize");

    let result = dispatcher.dispatch(&event);

    let actual = serde_json::to_string_pretty(&result).expect("should serialize");

    println!("Dispatch result snapshot:\n{}", actual);
}
