use super::{
    classify_event, AuthorKind, ChatScope, EventTrait, ExecutionLane, ExecutionOutcome,
    ExecutionRouter, IngressClass, RouteBucket, RouterIndex, RoutingError, UnitDispatchInvocation,
    UnitDispatchTrigger,
};
use crate::event::{
    CallbackContext, ChatContext, EventContext, EventNormalizer, ExecutionMode,
    ManualInvocationInput, MessageContentKind, MessageContext, ScheduledJobInput, SenderContext,
    SystemContext, TelegramUpdateInput, UnitContext, UpdateType,
};
use crate::moderation::{ModerationEngine, ModerationEventResult};
use crate::storage::Storage;
use crate::tg::TelegramGateway;
use crate::unit::{
    CapabilitiesSpec, ServiceSpec, TriggerSpec, UnitDefinition, UnitEventType, UnitManifest,
    UnitRegistry,
};
use chrono::{TimeZone, Utc};
use serde_json::json;
use tempfile::tempdir;

fn ts() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 4, 22, 12, 0, 0)
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

fn private_chat() -> ChatContext {
    ChatContext {
        id: 42,
        chat_type: "private".to_owned(),
        title: None,
        username: Some("dm_user".to_owned()),
        photo_file_id: None,
        thread_id: None,
    }
}

fn admin_sender() -> SenderContext {
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

fn manual_event(command_text: &str) -> EventContext {
    let normalizer = EventNormalizer::new();
    let mut input = ManualInvocationInput::new(
        UnitContext::new("moderation.test").with_trigger("manual"),
        command_text,
    );
    input.received_at = ts();
    input.chat = Some(chat());
    input.sender = Some(admin_sender());
    normalizer
        .normalize_manual(input)
        .expect("manual event normalizes")
}

fn scheduled_event(command_text: &str) -> EventContext {
    let normalizer = EventNormalizer::new();
    let mut input = ScheduledJobInput::new(
        "job_123",
        UnitContext::new("moderation.test").with_trigger("schedule"),
        json!({ "kind": "scheduled" }),
        ts(),
        ts(),
    );
    input.received_at = ts();
    input.chat = Some(chat());
    input.sender = Some(admin_sender());
    input.command_text = Some(command_text.to_owned());
    normalizer
        .normalize_scheduled(input)
        .expect("scheduled event normalizes")
}

fn callback_event(data: &str) -> EventContext {
    let mut event = EventContext::new(
        "evt_callback",
        UpdateType::CallbackQuery,
        ExecutionMode::Realtime,
        SystemContext::realtime(),
    );
    event.update_id = Some(1001);
    event.chat = Some(chat());
    event.sender = Some(admin_sender());
    event.callback = Some(CallbackContext {
        query_id: "cbq-1".to_owned(),
        data: Some(data.to_owned()),
        message_id: Some(700),
        origin_chat_id: Some(-100123),
        from_user_id: 42,
    });
    event
}

fn realtime_text_event(text: &str) -> EventContext {
    let mut input = TelegramUpdateInput::message(
        1002,
        chat(),
        admin_sender(),
        MessageContext {
            id: 701,
            date: ts(),
            text: Some(text.to_owned()),
            content_kind: Some(MessageContentKind::Text),
            entities: vec![],
            has_media: false,
            file_ids: Vec::new(),
            reply_to_message_id: None,
            media_group_id: None,
        },
    );
    input.received_at = ts();
    EventNormalizer::new()
        .normalize_telegram(input)
        .expect("telegram event normalizes")
}

fn private_text_event(text: &str) -> EventContext {
    let mut input = TelegramUpdateInput::message(
        1003,
        private_chat(),
        admin_sender(),
        MessageContext {
            id: 702,
            date: ts(),
            text: Some(text.to_owned()),
            content_kind: Some(MessageContentKind::Text),
            entities: vec![],
            has_media: false,
            file_ids: Vec::new(),
            reply_to_message_id: None,
            media_group_id: None,
        },
    );
    input.received_at = ts();
    EventNormalizer::new()
        .normalize_telegram(input)
        .expect("private telegram event normalizes")
}

fn linked_channel_style_group_event() -> EventContext {
    let mut event = EventContext::new(
        "evt_linked_channel_style",
        UpdateType::Message,
        ExecutionMode::Realtime,
        SystemContext::realtime(),
    );
    event.update_id = Some(1004);
    event.chat = Some(chat());
    event.message = Some(MessageContext {
        id: 703,
        date: ts(),
        text: Some("channel-style post".to_owned()),
        content_kind: Some(MessageContentKind::Text),
        entities: vec![],
        has_media: false,
        file_ids: Vec::new(),
        reply_to_message_id: None,
        media_group_id: None,
    });
    event
}

fn registry_with_caps(caps: &[&str]) -> UnitRegistry {
    let mut manifest = UnitManifest::new(
        UnitDefinition::new("moderation.test"),
        TriggerSpec::command(["warn", "mute", "ban", "del", "undo", "msg"]),
        ServiceSpec::new("scripts/moderation/test.rhai"),
    );
    manifest.capabilities = CapabilitiesSpec {
        allow: caps.iter().map(|value| (*value).to_owned()).collect(),
        deny: Vec::new(),
    };
    UnitRegistry::load_manifests(vec![manifest]).registry
}

fn registry_from_manifests(manifests: Vec<UnitManifest>) -> UnitRegistry {
    UnitRegistry::load_manifests(manifests).registry
}

fn router_with_moderation() -> ExecutionRouter {
    let dir = tempdir().expect("tempdir");
    let storage = Storage::new(dir.path().join("runtime.sqlite3"))
        .bootstrap()
        .expect("bootstrap")
        .into_connection();
    let gateway = TelegramGateway::new(false);
    let engine = ModerationEngine::new(storage, gateway)
        .with_unit_registry(registry_with_caps(&[]))
        .with_admin_user_ids([42]);
    ExecutionRouter::new(0, false).with_moderation(engine)
}

#[test]
fn classifier_marks_manual_command_reply_traits() {
    let mut event = manual_event("/warn @spam spam");
    event.reply = Some(crate::event::ReplyContext {
        message_id: 900,
        sender_user_id: Some(99),
        sender_username: Some("spam".to_owned()),
        text: Some("spam".to_owned()),
        has_media: false,
    });
    event.message = event.message.map(|message| message.with_reply(Some(900)));

    let classified = classify_event(&event);

    assert_eq!(classified.ingress_class, IngressClass::Manual);
    assert_eq!(classified.command_name.as_deref(), Some("warn"));
    assert!(classified.traits.contains(&EventTrait::System));
    assert!(classified.traits.contains(&EventTrait::Text));
    assert!(classified.traits.contains(&EventTrait::Command));
    assert!(classified.traits.contains(&EventTrait::Reply));
}

#[test]
fn classifier_marks_scheduled_job_and_command() {
    let classified = classify_event(&scheduled_event("/mute @spam 1h"));

    assert_eq!(classified.ingress_class, IngressClass::Scheduled);
    assert_eq!(classified.command_name.as_deref(), Some("mute"));
    assert!(classified.traits.contains(&EventTrait::Job));
    assert!(classified.traits.contains(&EventTrait::Text));
    assert!(classified.traits.contains(&EventTrait::Command));
}

#[test]
fn classifier_marks_callback_command_bucket() {
    let classified = classify_event(&callback_event("/undo"));

    assert_eq!(classified.ingress_class, IngressClass::Realtime);
    assert_eq!(classified.command_name.as_deref(), Some("undo"));
    assert!(classified.traits.contains(&EventTrait::CallbackQuery));
    assert!(classified.traits.contains(&EventTrait::CallbackData));
    assert!(classified.traits.contains(&EventTrait::Command));
}

#[test]
fn classifier_marks_private_chat_scope_and_human_admin_author() {
    let classified = classify_event(&private_text_event("hello"));

    assert_eq!(classified.chat_scope, ChatScope::Private);
    assert_eq!(classified.author_kind, AuthorKind::HumanAdmin);
    assert!(classified.traits.contains(&EventTrait::Message));
    assert!(classified.traits.contains(&EventTrait::Text));
}

#[test]
fn classifier_marks_linked_channel_style_approximation() {
    let classified = classify_event(&linked_channel_style_group_event());

    assert_eq!(classified.chat_scope, ChatScope::Supergroup);
    assert_eq!(classified.author_kind, AuthorKind::ChannelIdentity);
    assert!(classified.traits.contains(&EventTrait::LinkedChannelStyle));
    assert!(classified.traits.contains(&EventTrait::Message));
}

#[test]
fn classifier_marks_voice_bucket_when_content_kind_is_voice() {
    let mut event = realtime_text_event("");
    event.message = Some(MessageContext {
        id: 704,
        date: ts(),
        text: None,
        content_kind: Some(MessageContentKind::Voice),
        entities: vec![],
        has_media: true,
        file_ids: vec!["voice-file".to_owned()],
        reply_to_message_id: None,
        media_group_id: None,
    });

    let classified = classify_event(&event);

    assert!(classified.traits.contains(&EventTrait::Voice));
    assert!(classified.traits.contains(&EventTrait::Media));
    assert!(!classified.traits.contains(&EventTrait::Text));
}

#[test]
fn classifier_maps_every_message_content_kind_to_its_bucket() {
    let cases = [
        (MessageContentKind::Text, EventTrait::Text, false),
        (MessageContentKind::Photo, EventTrait::Photo, true),
        (MessageContentKind::Voice, EventTrait::Voice, true),
        (MessageContentKind::Video, EventTrait::Video, true),
        (MessageContentKind::Audio, EventTrait::Audio, true),
        (MessageContentKind::Document, EventTrait::Document, true),
        (MessageContentKind::Sticker, EventTrait::Sticker, true),
        (MessageContentKind::Animation, EventTrait::Animation, true),
        (MessageContentKind::VideoNote, EventTrait::VideoNote, true),
        (MessageContentKind::Contact, EventTrait::Contact, true),
        (MessageContentKind::Location, EventTrait::Location, true),
        (MessageContentKind::Poll, EventTrait::Poll, true),
        (MessageContentKind::Dice, EventTrait::Dice, true),
        (MessageContentKind::Venue, EventTrait::Venue, true),
        (MessageContentKind::Game, EventTrait::Game, true),
        (MessageContentKind::Invoice, EventTrait::Invoice, true),
        (MessageContentKind::Story, EventTrait::Story, true),
        (
            MessageContentKind::UnknownMedia,
            EventTrait::UnknownMedia,
            true,
        ),
    ];
    for (content_kind, expected_trait, expects_media_trait) in cases {
        let mut event = realtime_text_event("");
        event.message = Some(MessageContext {
            id: 900,
            date: ts(),
            text: matches!(content_kind, MessageContentKind::Text).then(|| "hello".to_owned()),
            content_kind: Some(content_kind),
            entities: vec![],
            has_media: expects_media_trait,
            file_ids: Vec::new(),
            reply_to_message_id: None,
            media_group_id: None,
        });

        let classified = classify_event(&event);

        assert!(
            classified.traits.contains(&expected_trait),
            "expected trait {expected_trait:?} for {content_kind:?}"
        );
        assert_eq!(
            classified.traits.contains(&EventTrait::Media),
            expects_media_trait,
            "unexpected media trait state for {content_kind:?}"
        );
    }
}

#[test]
fn router_plan_tracks_buckets_for_non_command_text() {
    let router = ExecutionRouter::new(0, false);
    let plan = router.plan(&realtime_text_event("hello"));

    assert!(
        plan.matched_buckets
            .contains(&RouteBucket::IngressClass(IngressClass::Realtime))
    );
    assert!(
        plan.matched_buckets
            .contains(&RouteBucket::ChatScope(ChatScope::Supergroup))
    );
    assert!(
        plan.matched_buckets
            .contains(&RouteBucket::AuthorKind(AuthorKind::HumanAdmin))
    );
    assert!(
        plan.matched_buckets
            .contains(&RouteBucket::EventTrait(EventTrait::Message))
    );
    assert!(
        plan.matched_buckets
            .contains(&RouteBucket::EventTrait(EventTrait::Text))
    );
    assert!(plan.lanes.is_empty());
}

#[test]
fn router_index_builds_unit_dispatch_routes_from_registry_triggers() {
    let command_manifest = UnitManifest::new(
        UnitDefinition::new("command.stats.unit"),
        TriggerSpec::command(["stats"]),
        ServiceSpec::new("scripts/command/stats.rhai"),
    );
    let callback_manifest = UnitManifest::new(
        UnitDefinition::new("callback.resolve.unit"),
        TriggerSpec::event_type([UnitEventType::CallbackQuery]),
        ServiceSpec::new("scripts/callback/resolve.rhai"),
    );
    let regex_manifest = UnitManifest::new(
        UnitDefinition::new("message.link_filter.unit"),
        TriggerSpec::regex("https?://"),
        ServiceSpec::new("scripts/message/link_filter.rhai"),
    );
    let index = RouterIndex::from_registry(&registry_from_manifests(vec![
        command_manifest,
        callback_manifest,
        regex_manifest,
    ]));
    let stats = index.stats();

    assert!(stats.command_routes >= 7);
    assert!(stats.trait_routes >= 2);

    let command_plan = index.plan(classify_event(&manual_event("/stats")));
    assert!(command_plan.lanes.contains(&ExecutionLane::UnitDispatch));
    assert!(
        command_plan
            .matched_buckets
            .contains(&RouteBucket::CommandIndex("stats".to_owned()))
    );

    let callback_plan = index.plan(classify_event(&callback_event("resolve")));
    assert!(
        callback_plan
            .matched_buckets
            .contains(&RouteBucket::EventTrait(EventTrait::CallbackQuery))
    );
    assert!(callback_plan.lanes.contains(&ExecutionLane::UnitDispatch));
}

#[test]
fn router_sync_registry_rebuilds_from_live_registry_state() {
    let stats_manifest = UnitManifest::new(
        UnitDefinition::new("command.stats.unit"),
        TriggerSpec::command(["stats"]),
        ServiceSpec::new("scripts/command/stats.rhai"),
    );
    let audit_manifest = UnitManifest::new(
        UnitDefinition::new("command.audit.unit"),
        TriggerSpec::command(["audit"]),
        ServiceSpec::new("scripts/command/audit.rhai"),
    );
    let router = ExecutionRouter::new(0, false)
        .with_registry(registry_from_manifests(vec![stats_manifest]));

    assert!(
        router
            .plan(&manual_event("/stats"))
            .lanes
            .contains(&ExecutionLane::UnitDispatch)
    );
    assert!(
        !router
            .plan(&manual_event("/audit"))
            .lanes
            .contains(&ExecutionLane::UnitDispatch)
    );

    router.sync_registry(registry_from_manifests(vec![audit_manifest]));

    assert!(
        !router
            .plan(&manual_event("/stats"))
            .lanes
            .contains(&ExecutionLane::UnitDispatch)
    );
    assert!(
        router
            .plan(&manual_event("/audit"))
            .lanes
            .contains(&ExecutionLane::UnitDispatch)
    );
}

#[tokio::test]
async fn router_executes_built_in_moderation_for_indexed_command() {
    let router = router_with_moderation();
    let mut event = manual_event("/warn @spam spam");
    event.reply = Some(crate::event::ReplyContext {
        message_id: 900,
        sender_user_id: Some(99),
        sender_username: Some("spam".to_owned()),
        text: Some("spam".to_owned()),
        has_media: false,
    });
    event.message = event.message.map(|message| message.with_reply(Some(900)));

    let outcome = router.route(&event).await.expect("routing succeeds");

    let ExecutionOutcome::BuiltInModeration {
        plan,
        result,
        deferred_invocations,
    } = outcome
    else {
        panic!("expected built-in moderation outcome");
    };
    assert!(
        plan.matched_buckets
            .contains(&RouteBucket::CommandIndex("warn".to_owned()))
    );
    assert!(deferred_invocations.is_empty());
    assert!(matches!(result, ModerationEventResult::Executed(_)));
}

#[tokio::test]
async fn router_surfaces_deferred_unit_dispatch_when_built_in_and_unit_match_same_command() {
    let unit_manifest = UnitManifest::new(
        UnitDefinition::new("moderation.warn.shadow"),
        TriggerSpec::command(["warn"]),
        ServiceSpec::new("scripts/moderation/warn_shadow.rhai"),
    );
    let registry = registry_from_manifests(vec![unit_manifest]);
    let router = router_with_moderation().with_registry(registry.clone());
    let mut event = manual_event("/warn @spam spam");
    event.reply = Some(crate::event::ReplyContext {
        message_id: 900,
        sender_user_id: Some(99),
        sender_username: Some("spam".to_owned()),
        text: Some("spam".to_owned()),
        has_media: false,
    });
    event.message = event.message.map(|message| message.with_reply(Some(900)));

    let outcome = router.route(&event).await.expect("routing succeeds");

    let ExecutionOutcome::BuiltInModeration {
        plan,
        result,
        deferred_invocations,
    } = outcome
    else {
        panic!("expected built-in moderation outcome");
    };
    assert_eq!(
        plan.lanes,
        vec![
            ExecutionLane::BuiltInModeration,
            ExecutionLane::UnitDispatch,
        ]
    );
    assert_eq!(
        deferred_invocations,
        vec![UnitDispatchInvocation {
            unit_id: "moderation.warn.shadow".to_owned(),
            exec_start: "scripts/moderation/warn_shadow.rhai".to_owned(),
            entry_point: None,
            trigger: UnitDispatchTrigger::Command {
                command: "warn".to_owned(),
            },
        }]
    );
    assert!(matches!(result, ModerationEventResult::Executed(_)));
}

#[tokio::test]
async fn router_passes_explicit_unit_policy_to_built_in_moderation() {
    let mut unit_manifest = UnitManifest::new(
        UnitDefinition::new("moderation.mute.shadow"),
        TriggerSpec::command(["mute"]),
        ServiceSpec::new("scripts/moderation/mute_shadow.rhai"),
    );
    unit_manifest.capabilities = CapabilitiesSpec {
        allow: vec!["tg.moderate.restrict".to_owned()],
        deny: Vec::new(),
    };
    let registry = registry_from_manifests(vec![unit_manifest]);
    let dir = tempdir().expect("tempdir");
    let storage = Storage::new(dir.path().join("runtime.sqlite3"))
        .bootstrap()
        .expect("bootstrap")
        .into_connection();
    let gateway = TelegramGateway::new(false);
    let engine = ModerationEngine::new(storage, gateway)
        .with_unit_registry(registry.clone())
        .with_dry_run(true)
        .with_admin_user_ids([42]);
    let router = ExecutionRouter::new(0, false)
        .with_registry(registry.clone())
        .with_moderation(engine);
    let mut event = manual_event("/mute 30m spam");
    event.reply = Some(crate::event::ReplyContext {
        message_id: 900,
        sender_user_id: Some(99),
        sender_username: Some("spam".to_owned()),
        text: Some("spam".to_owned()),
        has_media: false,
    });
    event.message = event.message.map(|message| message.with_reply(Some(900)));

    let expected_unit_id = event.system.unit.as_ref().map(|unit| unit.id.clone());
    let expected_unit_trigger = event
        .system
        .unit
        .as_ref()
        .and_then(|unit| unit.trigger.clone());
    let outcome = router.route(&event).await.expect("routing succeeds");

    let ExecutionOutcome::BuiltInModeration {
        plan,
        result,
        deferred_invocations,
    } = outcome
    else {
        panic!("expected built-in moderation outcome");
    };
    assert_eq!(
        plan.lanes,
        vec![
            ExecutionLane::BuiltInModeration,
            ExecutionLane::UnitDispatch,
        ]
    );
    assert_eq!(
        deferred_invocations,
        vec![UnitDispatchInvocation {
            unit_id: "moderation.mute.shadow".to_owned(),
            exec_start: "scripts/moderation/mute_shadow.rhai".to_owned(),
            entry_point: None,
            trigger: UnitDispatchTrigger::Command {
                command: "mute".to_owned(),
            },
        }]
    );
    assert_eq!(
        event.system.unit.as_ref().map(|unit| unit.id.as_str()),
        expected_unit_id.as_deref()
    );
    assert_eq!(
        event
            .system
            .unit
            .as_ref()
            .and_then(|unit| unit.trigger.as_deref()),
        expected_unit_trigger.as_deref()
    );
    let ModerationEventResult::Executed(execution) = result else {
        panic!("expected built-in moderation execution");
    };
    assert_eq!(
        execution.audit_entries[0].unit_name,
        "moderation.mute.shadow"
    );
}

#[tokio::test]
async fn router_dispatches_matching_command_units_with_service_envelope() {
    let mut manifest = UnitManifest::new(
        UnitDefinition::new("moderation.stats.audit"),
        TriggerSpec::command(["stats", "audit_stats"]),
        ServiceSpec::new("scripts/moderation/stats_audit.rhai"),
    );
    manifest.service.entry_point = Some("handle_stats".to_owned());
    let router = ExecutionRouter::new(0, false).with_registry(registry_from_manifests(vec![
        manifest,
    ]));

    let outcome = router
        .route(&manual_event("/stats"))
        .await
        .expect("routing succeeds");

    let ExecutionOutcome::UnitDispatch { plan, invocations } = outcome else {
        panic!("expected unit dispatch outcome");
    };
    assert_eq!(plan.lanes, vec![ExecutionLane::UnitDispatch]);
    assert_eq!(
        invocations,
        vec![UnitDispatchInvocation {
            unit_id: "moderation.stats.audit".to_owned(),
            exec_start: "scripts/moderation/stats_audit.rhai".to_owned(),
            entry_point: Some("handle_stats".to_owned()),
            trigger: UnitDispatchTrigger::Command {
                command: "stats".to_owned(),
            },
        }]
    );
}

#[tokio::test]
async fn router_dispatches_matching_regex_units_only_when_pattern_matches() {
    let matching = UnitManifest::new(
        UnitDefinition::new("message.link.filter"),
        TriggerSpec::regex("https?://"),
        ServiceSpec::new("scripts/filter/link.rhai"),
    );
    let non_matching = UnitManifest::new(
        UnitDefinition::new("message.phone.filter"),
        TriggerSpec::regex("\\+\\d{11}"),
        ServiceSpec::new("scripts/filter/phone.rhai"),
    );
    let router = ExecutionRouter::new(0, false)
        .with_registry(registry_from_manifests(vec![matching, non_matching]));

    let outcome = router
        .route(&realtime_text_event("visit https://example.com now"))
        .await
        .expect("routing succeeds");

    let ExecutionOutcome::UnitDispatch { invocations, .. } = outcome else {
        panic!("expected unit dispatch outcome");
    };
    assert_eq!(
        invocations,
        vec![UnitDispatchInvocation {
            unit_id: "message.link.filter".to_owned(),
            exec_start: "scripts/filter/link.rhai".to_owned(),
            entry_point: None,
            trigger: UnitDispatchTrigger::Regex {
                pattern: "https?://".to_owned(),
            },
        }]
    );
}

#[tokio::test]
async fn router_dispatches_matching_event_type_units() {
    let manifest = UnitManifest::new(
        UnitDefinition::new("callback.resolve"),
        TriggerSpec::event_type([UnitEventType::CallbackQuery]),
        ServiceSpec::new("scripts/callback/resolve.rhai"),
    );
    let router = ExecutionRouter::new(0, false).with_registry(registry_from_manifests(vec![
        manifest,
    ]));

    let outcome = router
        .route(&callback_event("resolve:123"))
        .await
        .expect("routing succeeds");

    let ExecutionOutcome::UnitDispatch { invocations, .. } = outcome else {
        panic!("expected unit dispatch outcome");
    };
    assert_eq!(
        invocations,
        vec![UnitDispatchInvocation {
            unit_id: "callback.resolve".to_owned(),
            exec_start: "scripts/callback/resolve.rhai".to_owned(),
            entry_point: None,
            trigger: UnitDispatchTrigger::EventType {
                event: UnitEventType::CallbackQuery,
            },
        }]
    );
}

#[tokio::test]
async fn router_reports_missing_executor_for_indexed_lane() {
    let router = ExecutionRouter::new(0, false);
    let mut event = manual_event("/warn @spam spam");
    event.reply = Some(crate::event::ReplyContext {
        message_id: 900,
        sender_user_id: Some(99),
        sender_username: Some("spam".to_owned()),
        text: Some("spam".to_owned()),
        has_media: false,
    });
    event.message = event.message.map(|message| message.with_reply(Some(900)));

    let error = router
        .route(&event)
        .await
        .expect_err("missing executor must fail");

    assert!(matches!(error, RoutingError::MissingLaneExecutor { .. }));
}
