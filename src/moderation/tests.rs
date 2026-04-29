use super::{ModerationEngine, ModerationError, ModerationEventResult, ModerationUnitPolicy};
use crate::event::{
    ChatContext, EventNormalizer, ExecutionMode, ManualInvocationInput, MessageContext,
    ReplyContext, SenderContext, SystemContext, TelegramUpdateInput, UnitContext,
};
use crate::tg::{
    TelegramDeleteResult, TelegramGateway, TelegramMessageResult, TelegramRequest, TelegramResult,
    TelegramTransport, TelegramUiResult,
};
use crate::unit::{
    CapabilitiesSpec, ServiceSpec, TriggerSpec, UnitDefinition, UnitManifest, UnitRegistry,
};
use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use std::sync::{Arc, Mutex};
use tempfile::tempdir;

use crate::storage::{
    AuditLogFilter, MessageJournalRecord, PROCESSED_UPDATE_STATUS_PENDING, ProcessedUpdateRecord,
    Storage,
};

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
            TelegramRequest::DeleteMany(request) => TelegramResult::Delete(TelegramDeleteResult {
                chat_id: request.chat_id,
                deleted: request.message_ids,
                failed: Vec::new(),
            }),
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
            TelegramRequest::Ban(request) => TelegramResult::Ban(crate::tg::TelegramBanResult {
                chat_id: request.chat_id,
                user_id: request.user_id,
                until: request.until,
                delete_history: request.delete_history,
                changed: true,
            }),
            TelegramRequest::Unban(request) => TelegramResult::Ban(crate::tg::TelegramBanResult {
                chat_id: request.chat_id,
                user_id: request.user_id,
                until: None,
                delete_history: false,
                changed: true,
            }),
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
            TelegramRequest::GetChatAdministrators(request) => {
                TelegramResult::ChatAdministrators(crate::tg::TelegramChatAdministratorsResult {
                    chat_id: request.chat_id,
                    administrators: Vec::new(),
                })
            }
            TelegramRequest::GetChatMember(request) => {
                TelegramResult::ChatMember(crate::tg::TelegramChatMemberResult {
                    chat_id: request.chat_id,
                    user_id: request.user_id,
                    member: crate::tg::TelegramChatMember {
                        user_id: request.user_id,
                        is_admin: false,
                        can_restrict_members: None,
                    },
                })
            }
        })
    }
}

fn ts() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 4, 22, 11, 0, 0)
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
        thread_id: None,
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

fn non_admin_sender() -> SenderContext {
    SenderContext {
        id: 777,
        username: Some("member".to_owned()),
        display_name: Some("Member".to_owned()),
        first_name: "Member".to_owned(),
        last_name: None,
        photo_file_id: None,
        is_bot: false,
        is_admin: false,
        role: Some("member".to_owned()),
    }
}

fn registry_with_caps(caps: &[&str]) -> UnitRegistry {
    let mut manifest = UnitManifest::new(
        UnitDefinition::new("moderation.test"),
        TriggerSpec::command(["warn", "mute", "del", "undo"]),
        ServiceSpec::new("scripts/moderation/test.rhai"),
    );
    manifest.capabilities = CapabilitiesSpec {
        allow: caps.iter().map(|value| (*value).to_owned()).collect(),
        deny: Vec::new(),
    };
    UnitRegistry::load_manifests(vec![manifest]).registry
}

fn engine_with_caps(
    caps: &[&str],
) -> (
    tempfile::TempDir,
    Arc<Mutex<Vec<TelegramRequest>>>,
    ModerationEngine,
) {
    engine_with_caps_and_admins(caps, [])
}

fn engine_with_caps_and_admins<I>(
    caps: &[&str],
    admin_user_ids: I,
) -> (
    tempfile::TempDir,
    Arc<Mutex<Vec<TelegramRequest>>>,
    ModerationEngine,
)
where
    I: IntoIterator<Item = i64>,
{
    let dir = tempdir().expect("tempdir");
    let storage = Storage::new(dir.path().join("runtime.sqlite3"))
        .bootstrap()
        .expect("bootstrap")
        .into_connection();
    storage
        .set_bot_is_admin(-100123, true)
        .expect("seed bot admin");
    let requests = Arc::new(Mutex::new(Vec::new()));
    let transport = RecordingTransport {
        requests: Arc::clone(&requests),
    };
    let gateway = TelegramGateway::new(false).with_transport(transport);
    let engine = ModerationEngine::new(storage, gateway)
        .with_unit_registry(registry_with_caps(caps))
        .with_admin_user_ids(admin_user_ids);
    (dir, requests, engine)
}

fn engine_without_registry() -> (
    tempfile::TempDir,
    Arc<Mutex<Vec<TelegramRequest>>>,
    ModerationEngine,
) {
    let dir = tempdir().expect("tempdir");
    let storage = Storage::new(dir.path().join("runtime.sqlite3"))
        .bootstrap()
        .expect("bootstrap")
        .into_connection();
    storage
        .set_bot_is_admin(-100123, true)
        .expect("seed bot admin");
    let requests = Arc::new(Mutex::new(Vec::new()));
    let transport = RecordingTransport {
        requests: Arc::clone(&requests),
    };
    let gateway = TelegramGateway::new(false).with_transport(transport);
    let engine = ModerationEngine::new(storage, gateway);
    (dir, requests, engine)
}

fn manual_event(command_text: &str) -> crate::event::EventContext {
    let normalizer = EventNormalizer::new();
    let mut input = ManualInvocationInput::new(
        UnitContext::new("moderation.test").with_trigger("manual"),
        command_text,
    );
    input.received_at = ts();
    input.chat = Some(chat());
    input.sender = Some(sender());
    normalizer
        .normalize_manual(input)
        .expect("manual event normalizes")
}

fn reply_event(
    command_text: &str,
    reply_user_id: i64,
    reply_message_id: i32,
) -> crate::event::EventContext {
    let mut event = manual_event(command_text);
    event.reply = Some(ReplyContext {
        message_id: reply_message_id,
        sender_user_id: Some(reply_user_id),
        sender_username: Some("spam_user".to_owned()),
        text: Some("spam".to_owned()),
        has_media: false,
    });
    event.message = Some(MessageContext {
        id: 900,
        date: ts(),
        text: Some(command_text.to_owned()),
        content_kind: Some(crate::event::MessageContentKind::Text),
        entities: vec!["bot_command".to_owned()],
        has_media: false,
        file_ids: Vec::new(),
        reply_to_message_id: Some(reply_message_id),
        media_group_id: None,
    });
    event
}

fn reply_event_with_sender(
    command_text: &str,
    reply_user_id: i64,
    reply_message_id: i32,
    sender: SenderContext,
) -> crate::event::EventContext {
    let mut event = reply_event(command_text, reply_user_id, reply_message_id);
    event.sender = Some(sender);
    event
}

fn live_reply_event_with_sender(
    command_text: &str,
    reply_user_id: i64,
    reply_message_id: i32,
    sender: SenderContext,
) -> crate::event::EventContext {
    let mut event = reply_event_with_sender(command_text, reply_user_id, reply_message_id, sender);
    event.execution_mode = ExecutionMode::Realtime;
    event.recovery = false;
    event.update_id = Some(4242);
    event.system = SystemContext::realtime();
    event
}

fn seed_journal(engine: &ModerationEngine) {
    for (message_id, user_id) in [(810, Some(99)), (811, Some(77)), (812, Some(99))] {
        engine
            .storage
            .append_message_journal(&MessageJournalRecord {
                chat_id: -100123,
                message_id,
                user_id,
                date_utc: ts().to_rfc3339(),
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

#[tokio::test]
async fn warn_updates_user_and_audit_log() {
    let (_dir, _requests, engine) = engine_with_caps(&[]);
    let event = reply_event("/warn 2.8", 99, 810);

    let result = engine.handle_event(&event).await.expect("warn succeeds");

    let ModerationEventResult::Executed(execution) = result else {
        panic!("expected executed result");
    };
    assert_eq!(execution.audit_entries.len(), 1);
    let user = engine
        .storage
        .get_user(99)
        .expect("user lookup")
        .expect("user exists");
    assert_eq!(user.warn_count, 1);
    assert_eq!(execution.audit_entries[0].op, "warn");
    assert!(execution.audit_entries[0].reversible);
}

#[tokio::test]
async fn mute_executes_restrict_and_schedules_pipe_message() {
    let (_dir, requests, engine) =
        engine_with_caps(&["tg.moderate.restrict", "job.schedule", "tg.write_message"]);
    let event = reply_event(r#"/mute 30m spam | /msg "mute expired""#, 99, 810);

    let result = engine.handle_event(&event).await.expect("mute succeeds");

    let ModerationEventResult::Executed(execution) = result else {
        panic!("expected executed result");
    };
    assert_eq!(execution.telegram.len(), 1);
    assert_eq!(execution.audit_entries[0].op, "mute");
    assert_eq!(execution.jobs.len(), 1);
    let stored_job = engine
        .storage
        .get_job(&execution.jobs[0].job_id)
        .expect("job lookup")
        .expect("job exists");
    assert_eq!(stored_job.executor_unit, "moderation.pipe.message");
    let requests = requests.lock().expect("requests");
    assert!(matches!(requests[0], TelegramRequest::Restrict(_)));
}

#[tokio::test]
async fn mute_pipe_requires_job_schedule_before_side_effects() {
    let (_dir, requests, engine) = engine_with_caps(&["tg.moderate.restrict"]);
    let event = reply_event(r#"/mute 30m spam | /msg "mute expired""#, 99, 810);

    let error = engine
        .handle_event(&event)
        .await
        .expect_err("mute pipe must be denied");

    assert!(matches!(
        error,
        ModerationError::CapabilityDenied {
            capability,
            unit_id,
        } if capability == "job.schedule" && unit_id == "moderation.test"
    ));
    assert!(requests.lock().expect("requests").is_empty());
    assert!(
        engine
            .storage
            .find_audit_entries(&AuditLogFilter::default(), 10)
            .expect("audit lookup")
            .is_empty()
    );
}

#[tokio::test]
async fn delete_window_uses_anchor_and_user_filter() {
    let (_dir, requests, engine) = engine_with_caps(&["tg.moderate.delete"]);
    seed_journal(&engine);
    let event = manual_event("/del msg:811 -up 1 -dn 1 -user 99");

    let result = engine.handle_event(&event).await.expect("delete succeeds");

    let ModerationEventResult::Executed(execution) = result else {
        panic!("expected executed result");
    };
    assert_eq!(execution.audit_entries[0].op, "del");
    let requests = requests.lock().expect("requests");
    let TelegramRequest::DeleteMany(request) = &requests[0] else {
        panic!("expected delete_many request");
    };
    assert_eq!(request.message_ids, vec![810, 812]);
}

#[tokio::test]
async fn undo_compensates_previous_mute() {
    let (_dir, requests, engine) = engine_with_caps(&["tg.moderate.restrict", "audit.compensate"]);
    let mute_event = reply_event("/mute 30m spam", 99, 810);
    let mute_result = engine
        .handle_event(&mute_event)
        .await
        .expect("mute succeeds");
    let ModerationEventResult::Executed(mute_execution) = mute_result else {
        panic!("expected executed mute");
    };
    let original_action_id = mute_execution.audit_entries[0].action_id.clone();

    let undo_event = reply_event("/undo", 99, 900);
    let undo_result = engine
        .handle_event(&undo_event)
        .await
        .expect("undo succeeds");

    let ModerationEventResult::Executed(execution) = undo_result else {
        panic!("expected executed undo");
    };
    assert_eq!(execution.audit_entries[0].op, "undo");
    assert_eq!(execution.audit_entries[0].target_id.as_deref(), Some("99"));
    let requests = requests.lock().expect("requests");
    assert!(matches!(requests[0], TelegramRequest::Restrict(_)));
    assert!(matches!(requests[1], TelegramRequest::Unrestrict(_)));
    let undo_entries = engine
        .storage
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
    assert_ne!(undo_entries[0].action_id, original_action_id);
}

#[tokio::test]
async fn undo_cannot_compensate_same_action_twice() {
    let (_dir, _requests, engine) = engine_with_caps(&["tg.moderate.restrict", "audit.compensate"]);
    let mute_event = reply_event("/mute 30m spam", 99, 810);
    let mute_result = engine
        .handle_event(&mute_event)
        .await
        .expect("mute succeeds");
    let ModerationEventResult::Executed(mute_execution) = mute_result else {
        panic!("expected executed mute");
    };

    let undo_event = reply_event("/undo", 99, 900);
    let first_undo = engine
        .handle_event(&undo_event)
        .await
        .expect("first undo succeeds");
    assert!(matches!(first_undo, ModerationEventResult::Executed(_)));

    let error = engine
        .handle_event(&undo_event)
        .await
        .expect_err("second undo must fail");

    assert!(matches!(
        error,
        ModerationError::Validation(message)
        if message == format!("action {} is already compensated", mute_execution.audit_entries[0].action_id)
    ));
}

#[tokio::test]
async fn replayed_update_is_skipped_without_duplicate_transport_calls() {
    let (_dir, requests, engine) = engine_with_caps(&["tg.moderate.delete"]);
    seed_journal(&engine);
    let normalizer = EventNormalizer::new();
    let mut input = TelegramUpdateInput::message(
        1001,
        chat(),
        sender(),
        MessageContext {
            id: 811,
            date: ts(),
            text: Some("/del msg:811".to_owned()),
            content_kind: Some(crate::event::MessageContentKind::Text),
            entities: vec!["bot_command".to_owned()],
            has_media: false,
            file_ids: Vec::new(),
            reply_to_message_id: None,
            media_group_id: None,
        },
    );
    input.event_id = Some("evt_tg_delete".to_owned());
    input.received_at = ts();
    let mut event = normalizer
        .normalize_telegram(input)
        .expect("telegram event normalizes");
    event.system.unit = Some(UnitContext::new("moderation.test").with_trigger("telegram"));

    let first = engine
        .handle_event(&event)
        .await
        .expect("first pass succeeds");
    assert!(matches!(first, ModerationEventResult::Executed(_)));
    let second = engine.handle_event(&event).await.expect("replay succeeds");
    assert!(matches!(second, ModerationEventResult::Replayed(_)));
    assert_eq!(requests.lock().expect("requests").len(), 1);
}

#[tokio::test]
async fn pending_realtime_update_fails_closed_without_reexecution() {
    let (_dir, requests, engine) = engine_with_caps(&["tg.moderate.delete"]);
    seed_journal(&engine);
    let normalizer = EventNormalizer::new();
    let mut input = TelegramUpdateInput::message(
        1002,
        chat(),
        sender(),
        MessageContext {
            id: 811,
            date: ts(),
            text: Some("/del msg:811".to_owned()),
            content_kind: Some(crate::event::MessageContentKind::Text),
            entities: vec!["bot_command".to_owned()],
            has_media: false,
            file_ids: Vec::new(),
            reply_to_message_id: None,
            media_group_id: None,
        },
    );
    input.event_id = Some("evt_tg_delete_pending".to_owned());
    input.received_at = ts();
    let mut event = normalizer
        .normalize_telegram(input)
        .expect("telegram event normalizes");
    event.system.unit = Some(UnitContext::new("moderation.test").with_trigger("telegram"));
    engine
        .storage
        .mark_processed_update(&ProcessedUpdateRecord {
            update_id: 1002,
            event_id: "evt_tg_delete_pending".to_owned(),
            processed_at: ts().to_rfc3339(),
            execution_mode: "realtime".to_owned(),
            status: PROCESSED_UPDATE_STATUS_PENDING.to_owned(),
        })
        .expect("pending mark succeeds");

    let error = engine
        .handle_event(&event)
        .await
        .expect_err("pending update must fail closed");

    assert!(matches!(
        error,
        ModerationError::ProcessingInterrupted(event_id)
        if event_id == "evt_tg_delete_pending"
    ));
    assert!(requests.lock().expect("requests").is_empty());
}

#[tokio::test]
async fn capability_denial_is_structured() {
    let (_dir, _requests, engine) = engine_with_caps(&["audit.compensate"]);
    let event = reply_event("/mute 30m spam", 99, 810);

    let error = engine
        .handle_event(&event)
        .await
        .expect_err("mute must be denied");

    assert!(matches!(
        error,
        ModerationError::CapabilityDenied {
            capability,
            unit_id,
        } if capability == "tg.moderate.restrict" && unit_id == "moderation.test"
    ));
}

#[tokio::test]
async fn capability_gated_operation_fails_closed_without_registry() {
    let (_dir, requests, engine) = engine_without_registry();
    let event = reply_event("/mute 30m spam", 99, 810);

    let error = engine
        .handle_event(&event)
        .await
        .expect_err("mute must be denied without registry");

    assert!(matches!(
        error,
        ModerationError::CapabilityDenied {
            capability,
            unit_id,
        } if capability == "tg.moderate.restrict" && unit_id == "moderation.test"
    ));
    assert!(requests.lock().expect("requests").is_empty());
}

#[tokio::test]
async fn explicit_unit_policy_keeps_capability_checks_and_audit_scoped() {
    let (_dir, _requests, engine) = engine_with_caps(&["tg.moderate.restrict"]);
    let event = reply_event("/mute 30m spam", 99, 810);
    let policy =
        ModerationUnitPolicy::new(UnitContext::new("moderation.test").with_trigger("telegram"));

    let result = engine
        .handle_event_with_unit_policy(&event, Some(&policy))
        .await
        .expect("mute succeeds with explicit policy");

    let ModerationEventResult::Executed(execution) = result else {
        panic!("expected executed result");
    };
    assert_eq!(execution.audit_entries[0].unit_name, "moderation.test");
}

#[tokio::test]
async fn non_admin_sender_cannot_execute_moderation_command() {
    let (_dir, requests, engine) = engine_with_caps(&["tg.moderate.restrict"]);
    let event = reply_event_with_sender("/mute 30m spam", 99, 810, non_admin_sender());

    let error = engine
        .handle_event(&event)
        .await
        .expect_err("non-admin sender must be denied");

    assert!(matches!(
        error,
        ModerationError::AuthorizationDenied { user_id: Some(777) }
    ));
    assert!(requests.lock().expect("requests").is_empty());
}

#[tokio::test]
async fn configured_admin_id_can_execute_even_without_sender_admin_flag() {
    let (_dir, requests, engine) = engine_with_caps_and_admins(&["tg.moderate.restrict"], [777]);
    let event = reply_event_with_sender("/mute 30m spam", 99, 810, non_admin_sender());

    let result = engine
        .handle_event(&event)
        .await
        .expect("configured admin succeeds");

    assert!(matches!(result, ModerationEventResult::Executed(_)));
    assert_eq!(requests.lock().expect("requests").len(), 1);
}

#[tokio::test]
async fn chat_admin_from_storage_can_execute_without_config_allowlist() {
    let (_dir, requests, engine) = engine_without_registry();
    engine
        .storage
        .set_chat_user_is_admin(-100123, 777, true)
        .expect("seed chat admin");
    engine
        .storage
        .upsert_user(&crate::storage::UserPatch {
            user_id: 99,
            username: Some("spam_user".to_owned()),
            display_name: Some("Spam User".to_owned()),
            seen_at: ts().to_rfc3339(),
            warn_count: None,
            shadowbanned: None,
            reputation: None,
            state_json: None,
            updated_at: ts().to_rfc3339(),
        })
        .expect("seed seen user");
    let event = live_reply_event_with_sender("/ban @spam_user spam", 99, 810, non_admin_sender());

    let result = engine
        .handle_event(&event)
        .await
        .expect("chat admin from storage succeeds");

    assert!(matches!(result, ModerationEventResult::Executed(_)));
    let requests = requests.lock().expect("requests");
    let TelegramRequest::Ban(request) = &requests[0] else {
        panic!("expected ban request");
    };
    assert_eq!(request.user_id, 99);
}

#[tokio::test]
async fn ping_is_available_to_non_admin_sender() {
    let (_dir, requests, engine) = engine_without_registry();
    let event = reply_event_with_sender("/ping", 99, 810, non_admin_sender());

    let result = engine.handle_event(&event).await.expect("ping succeeds");

    assert!(matches!(result, ModerationEventResult::Executed(_)));
    let requests = requests.lock().expect("requests");
    let TelegramRequest::SendMessage(request) = &requests[0] else {
        panic!("expected send_message request");
    };
    assert_eq!(request.text, "pong");
    assert_eq!(request.reply_to_message_id, None);
}

#[tokio::test]
async fn ping_bypasses_bot_admin_gate() {
    let (_dir, requests, engine) = engine_without_registry();
    engine
        .storage
        .set_bot_is_admin(-100123, false)
        .expect("clear bot admin");
    let event = reply_event_with_sender("/ping", 99, 810, non_admin_sender());

    let result = engine.handle_event(&event).await.expect("ping succeeds");

    assert!(matches!(result, ModerationEventResult::Executed(_)));
    assert_eq!(requests.lock().expect("requests").len(), 1);
}

#[tokio::test]
async fn help_is_available_to_non_admin_sender() {
    let (_dir, requests, engine) = engine_without_registry();
    let event = reply_event_with_sender("/help", 99, 810, non_admin_sender());

    let result = engine.handle_event(&event).await.expect("help succeeds");

    assert!(matches!(result, ModerationEventResult::Executed(_)));
    let requests = requests.lock().expect("requests");
    let TelegramRequest::SendMessage(request) = &requests[0] else {
        panic!("expected send_message request");
    };
    assert_eq!(request.reply_to_message_id, None);
    assert_eq!(request.parse_mode, crate::tg::ParseMode::Html);
    assert!(request.text.contains("<b>Публичные команды</b>"));
    assert!(request.text.contains("<code>/ping</code>"));
    assert!(request.text.contains("<b>Команды модерации</b>"));
}

#[tokio::test]
async fn ban_resolves_username_target_from_seen_users_cache() {
    let (_dir, requests, engine) = engine_with_caps_and_admins(&["tg.moderate.ban"], [777]);
    engine
        .storage
        .upsert_user(&crate::storage::UserPatch {
            user_id: 99,
            username: Some("spam_user".to_owned()),
            display_name: Some("Spam User".to_owned()),
            seen_at: ts().to_rfc3339(),
            warn_count: None,
            shadowbanned: None,
            reputation: None,
            state_json: None,
            updated_at: ts().to_rfc3339(),
        })
        .expect("seed seen user");
    let event = reply_event_with_sender("/ban @spam_user spam", 99, 810, non_admin_sender());

    let result = engine
        .handle_event(&event)
        .await
        .expect("ban by username succeeds");

    assert!(matches!(result, ModerationEventResult::Executed(_)));
    let requests = requests.lock().expect("requests");
    let TelegramRequest::Ban(request) = &requests[0] else {
        panic!("expected ban request");
    };
    assert_eq!(request.user_id, 99);
}
