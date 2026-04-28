use super::{HostApi, HostApiOperation};
use crate::event::{
    ChatContext, EventContext, EventNormalizer, ManualInvocationInput, ReplyContext, UnitContext,
};
use crate::storage::{AuditLogEntry, MessageJournalRecord, Storage};
use crate::unit::{
    CapabilitiesSpec, ServiceSpec, TriggerSpec, UnitDefinition, UnitManifest, UnitRegistry,
};
use chrono::{TimeZone, Utc};
use tempfile::TempDir;

pub(crate) fn ts() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 4, 21, 12, 0, 0)
        .single()
        .expect("valid timestamp")
}

pub(crate) fn manual_event() -> EventContext {
    let normalizer = EventNormalizer::new();
    let mut input = ManualInvocationInput::new(
        UnitContext::new("moderation.test").with_trigger("manual"),
        "/warn @spam spam",
    );
    input.event_id = Some("evt_host_api_manual".to_owned());
    input.received_at = ts();
    input.chat = Some(ChatContext {
        id: -100123,
        chat_type: "supergroup".to_owned(),
        title: Some("Moderation HQ".to_owned()),
        username: Some("mod_hq".to_owned()),
        photo_file_id: None,
        thread_id: Some(7),
    });
    input.reply = Some(ReplyContext {
        message_id: 99,
        sender_user_id: Some(77),
        sender_username: Some("reply_user".to_owned()),
        text: Some("reply".to_owned()),
        has_media: false,
    });

    normalizer
        .normalize_manual(input)
        .expect("manual event normalizes")
}

pub(crate) fn storage_api() -> (TempDir, HostApi) {
    let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
    let path = dir.path().join("host-api.sqlite3");
    let storage = Storage::new(path)
        .init()
        .unwrap_or_else(|error| panic!("storage init failed: {error}"));
    (dir, HostApi::new(false).with_storage(storage))
}

pub(crate) fn dry_run_storage_api() -> (TempDir, HostApi) {
    let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
    let path = dir.path().join("host-api.sqlite3");
    let storage = Storage::new(path)
        .init()
        .unwrap_or_else(|error| panic!("storage init failed: {error}"));
    (dir, HostApi::new(true).with_storage(storage))
}

pub(crate) fn storage_api_with_registry(
    allow: &[&str],
    deny: &[&str],
    dry_run: bool,
) -> (TempDir, HostApi) {
    let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
    let path = dir.path().join("host-api.sqlite3");
    let storage = Storage::new(path)
        .init()
        .unwrap_or_else(|error| panic!("storage init failed: {error}"));

    let mut manifest = UnitManifest::new(
        UnitDefinition::new("moderation.test"),
        TriggerSpec::command(["warn"]),
        ServiceSpec::new("cargo run"),
    );
    manifest.capabilities = CapabilitiesSpec {
        allow: allow.iter().map(|value| (*value).to_owned()).collect(),
        deny: deny.iter().map(|value| (*value).to_owned()).collect(),
    };
    let registry = UnitRegistry::load_manifests(vec![manifest]).registry;

    let api = HostApi::new(dry_run)
        .with_storage(storage)
        .with_unit_registry(registry);
    (dir, api)
}

pub(crate) fn unit_registry_api() -> HostApi {
    let active = UnitManifest::new(
        UnitDefinition::new("moderation.warn"),
        TriggerSpec::command(["warn"]),
        ServiceSpec::new("cargo run"),
    );
    let mut disabled = UnitManifest::new(
        UnitDefinition::new("moderation.mute"),
        TriggerSpec::command(["mute"]),
        ServiceSpec::new("cargo run"),
    );
    disabled.unit.enabled = false;

    let report = UnitRegistry::load_manifests(vec![active, disabled]);
    assert!(report.is_fully_valid());

    HostApi::new(false).with_unit_registry(report.registry)
}

pub(crate) fn seed_message_journal(api: &HostApi) {
    let storage = api
        .storage(HostApiOperation::MsgWindow)
        .expect("storage available");
    for (message_id, user_id, text, date_utc) in [
        (
            81229_i64,
            Some(99887766_i64),
            Some("spam 1"),
            "2026-04-21T11:59:00Z",
        ),
        (
            81230,
            Some(99887766),
            Some("spam 2"),
            "2026-04-21T11:59:10Z",
        ),
        (
            81231,
            Some(99887766),
            Some("spam 3"),
            "2026-04-21T11:59:20Z",
        ),
        (
            81232,
            Some(99887766),
            Some("spam 4"),
            "2026-04-21T11:59:30Z",
        ),
        (
            81233,
            Some(99887766),
            Some("spam 5"),
            "2026-04-21T11:59:40Z",
        ),
        (81234, Some(42), Some("admin note"), "2026-04-21T12:05:00Z"),
    ] {
        storage
            .append_message_journal(&MessageJournalRecord {
                chat_id: -100123,
                message_id,
                user_id,
                date_utc: date_utc.to_owned(),
                update_type: "message".to_owned(),
                text: text.map(str::to_owned),
                normalized_text: text.map(str::to_owned),
                has_media: false,
                reply_to_message_id: None,
                file_ids_json: None,
                meta_json: None,
            })
            .expect("seed message journal");
    }
}

pub(crate) fn seed_audit_entries(api: &HostApi) {
    let storage = api
        .storage(HostApiOperation::AuditFind)
        .expect("storage available");
    for entry in [
        AuditLogEntry {
            action_id: "act_1".to_owned(),
            trace_id: Some("trace-1".to_owned()),
            request_id: Some("req-1".to_owned()),
            unit_name: "moderation.test".to_owned(),
            execution_mode: "manual".to_owned(),
            op: "mute".to_owned(),
            actor_user_id: Some(42),
            chat_id: Some(-100123),
            target_kind: Some("user".to_owned()),
            target_id: Some("99887766".to_owned()),
            trigger_message_id: Some(81231),
            idempotency_key: Some("idem-1".to_owned()),
            reversible: true,
            compensation_json: Some("{\"kind\":\"host_op\",\"op\":\"tg.unrestrict\"}".to_owned()),
            args_json: "{\"duration\":\"7d\"}".to_owned(),
            result_json: Some("{\"ok\":true}".to_owned()),
            created_at: "2026-04-21T12:00:00Z".to_owned(),
        },
        AuditLogEntry {
            action_id: "act_2".to_owned(),
            trace_id: Some("trace-2".to_owned()),
            request_id: Some("req-2".to_owned()),
            unit_name: "moderation.test".to_owned(),
            execution_mode: "manual".to_owned(),
            op: "del".to_owned(),
            actor_user_id: Some(42),
            chat_id: Some(-100123),
            target_kind: Some("message".to_owned()),
            target_id: Some("81231".to_owned()),
            trigger_message_id: Some(81231),
            idempotency_key: Some("idem-2".to_owned()),
            reversible: false,
            compensation_json: None,
            args_json: "{\"count\":1}".to_owned(),
            result_json: Some("{\"deleted\":1}".to_owned()),
            created_at: "2026-04-21T12:01:00Z".to_owned(),
        },
    ] {
        storage
            .append_audit_entry(&entry)
            .expect("seed audit entry");
    }
}
