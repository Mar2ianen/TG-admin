use super::{IngressPipeline, IngressProcessResult, update_to_input_with_admin_user_ids};
use crate::event::{
    ChatContext, EventContext, EventNormalizer, ExecutionMode, MemberContext, MessageContext,
    SenderContext, SystemContext, TelegramUpdateInput, UpdateType,
};
use crate::moderation::ModerationEngine;
use crate::router::{ExecutionOutcome, ExecutionRouter};
use crate::storage::{
    AuditLogFilter, MessageJournalRecord, PROCESSED_UPDATE_STATUS_COMPLETED, Storage,
    StorageConnection,
};
use crate::tg::{
    TelegramDeleteResult, TelegramGateway, TelegramMessageResult, TelegramRequest, TelegramResult,
    TelegramTransport, TelegramUiResult,
};
use crate::unit::{ServiceSpec, TriggerSpec, UnitDefinition, UnitManifest, UnitRegistry};
use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use teloxide_core::types::Update;

fn ts() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 4, 22, 12, 0, 0)
        .single()
        .expect("valid timestamp")
}

fn pipeline() -> (tempfile::TempDir, IngressPipeline, StorageConnection) {
    let dir = tempfile::tempdir().expect("tempdir");
    let storage = Storage::new(dir.path().join("runtime.sqlite3"));
    let _ = storage.bootstrap().expect("bootstrap");
    let ingress_storage = storage.init().expect("ingress storage");
    let inspect_storage = storage.init().expect("inspect storage");
    let pipeline = IngressPipeline::new(
        teloxide_core::Bot::new("123456:TEST_TOKEN"),
        ingress_storage,
        Rc::new(ExecutionRouter::new(0, false)),
    )
    .with_admin_user_ids([42]);
    (dir, pipeline, inspect_storage)
}

#[cfg(test)]
const MOCK_FALLBACK_SEND_ID: i32 = 900;
#[cfg(test)]
const MOCK_FALLBACK_UI_ID: i32 = 700;

#[cfg(test)]
#[derive(Debug, Default)]
struct RecordingTransport {
    requests: Arc<Mutex<Vec<TelegramRequest>>>,
}

#[cfg(test)]
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
                    message_id: request
                        .reply_to_message_id
                        .unwrap_or(MOCK_FALLBACK_SEND_ID)
                        .saturating_add(1),
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
                message_id: request
                    .reply_to_message_id
                    .unwrap_or(MOCK_FALLBACK_UI_ID)
                    .saturating_add(1),
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
    moderation_storage
        .set_bot_is_admin(-100123, true)
        .expect("seed bot admin");
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
    let router = ExecutionRouter::new(0, false)
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
fn retry_delay_doubles_until_capped() {
    assert_eq!(
        super::next_retry_delay(Duration::from_millis(250)),
        Duration::from_millis(500)
    );
    assert_eq!(
        super::next_retry_delay(Duration::from_secs(3)),
        Duration::from_secs(5)
    );
    assert_eq!(
        super::next_retry_delay(Duration::from_secs(5)),
        Duration::from_secs(5)
    );
}

#[tokio::test]
async fn batch_processing_continues_after_a_single_update_failure() {
    let (_dir, _pipeline, _storage) = pipeline();
    let updates = vec![
        serde_json::from_str::<Update>(
            r#"{
                "update_id": 9001,
                "message": {
                    "chat": {
                        "id": -1001293752024,
                        "title": "CryptoInside Chat",
                        "type": "supergroup",
                        "username": "cryptoinside_talk"
                    },
                    "date": 1721592601,
                    "from": {
                        "first_name": "Alice",
                        "id": 42,
                        "is_bot": false,
                        "language_code": "en",
                        "username": "alice"
                    },
                    "message_id": 140001,
                    "text": "first"
                }
            }"#,
        )
        .expect("first update parses"),
        serde_json::from_str::<Update>(
            r#"{
                "update_id": 9002,
                "message": {
                    "chat": {
                        "id": -1001293752024,
                        "title": "CryptoInside Chat",
                        "type": "supergroup",
                        "username": "cryptoinside_talk"
                    },
                    "date": 1721592602,
                    "from": {
                        "first_name": "Alice",
                        "id": 42,
                        "is_bot": false,
                        "language_code": "en",
                        "username": "alice"
                    },
                    "message_id": 140002,
                    "text": "second"
                }
            }"#,
        )
        .expect("second update parses"),
    ];
    let seen = Arc::new(Mutex::new(Vec::new()));
    let seen_for_closure = Arc::clone(&seen);

    let next_offset = super::process_polled_updates_for_test(updates, move |update| {
        let seen_for_closure = Arc::clone(&seen_for_closure);
        seen_for_closure
            .lock()
            .expect("seen lock")
            .push(update.id.0);

        if update.id.0 == 9001 {
            return Err(anyhow::anyhow!("synthetic failure"));
        }

        Ok(IngressProcessResult::Processed)
    });

    assert_eq!(next_offset, Some(9003));
    assert_eq!(*seen.lock().expect("seen lock"), vec![9001, 9002]);
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
fn callback_updates_keep_inaccessible_message_context() {
    let update = serde_json::from_str::<Update>(
        r#"{
            "update_id": 439432605,
            "callback_query": {
                "id": "cbq-2",
                "from": {
                    "first_name": "Alice",
                    "id": 42,
                    "is_bot": false,
                    "language_code": "en",
                    "username": "alice"
                },
                "chat_instance": "chat-instance-2",
                "data": "/undo",
                "message": {
                    "chat": {
                        "id": -1001293752024,
                        "title": "CryptoInside Chat",
                        "type": "supergroup",
                        "username": "cryptoinside_talk"
                    },
                    "date": 0,
                    "message_id": 134547
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

    assert_eq!(event.update_type, crate::event::UpdateType::CallbackQuery);
    assert_eq!(
        event.chat.as_ref().map(|chat| chat.id),
        Some(-1001293752024)
    );
    assert!(event.message.is_none());
    assert_eq!(
        event
            .callback
            .as_ref()
            .and_then(|callback| callback.message_id),
        Some(134547)
    );
    assert_eq!(
        event
            .callback
            .as_ref()
            .and_then(|callback| callback.origin_chat_id),
        Some(-1001293752024)
    );
}

#[test]
fn inline_callback_without_message_context_stays_unsupported() {
    let update = serde_json::from_str::<Update>(
        r#"{
            "update_id": 439432606,
            "callback_query": {
                "id": "cbq-inline",
                "from": {
                    "first_name": "Alice",
                    "id": 42,
                    "is_bot": false,
                    "language_code": "en",
                    "username": "alice"
                },
                "chat_instance": "chat-instance-3",
                "inline_message_id": "AAEAAAE",
                "data": "/undo"
            }
        }"#,
    )
    .expect("update parses");

    assert!(
        update_to_input_with_admin_user_ids(&update, &[42])
            .expect("update converts")
            .is_none()
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
                photo_file_id: None,
                thread_id: None,
            },
            SenderContext {
                id: 408258968,
                username: Some("hirrolot".to_owned()),
                display_name: Some("Hirrolot".to_owned()),
                first_name: "Hirrolot".to_owned(),
                last_name: None,
                photo_file_id: None,
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
                photo_file_id: None,
                thread_id: None,
            },
            SenderContext {
                id: 77,
                username: Some("alice".to_owned()),
                display_name: Some("Alice".to_owned()),
                first_name: "Alice".to_owned(),
                last_name: None,
                photo_file_id: None,
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
        pipeline_with_router(ExecutionRouter::new(0, false).with_registry(registry));
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
        pipeline_with_router(ExecutionRouter::new(0, false).with_registry(registry));
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
async fn process_update_upserts_sender_into_users_cache() {
    let dir = tempfile::tempdir().expect("tempdir");
    let storage = Storage::new(dir.path().join("runtime.sqlite3"))
        .bootstrap()
        .expect("bootstrap")
        .into_connection();
    storage
        .set_bot_is_admin(-100123, true)
        .expect("seed bot admin");
    let router = std::rc::Rc::new(
        ExecutionRouter::new(0, false).with_moderation(
            ModerationEngine::new(storage.clone(), TelegramGateway::new(true))
                .with_admin_user_ids([42]),
        ),
    );
    let pipeline = IngressPipeline::new(
        teloxide_core::Bot::new("123456:TEST_TOKEN"),
        storage.clone(),
        router,
    )
    .with_admin_user_ids([42]);

    let mut input = TelegramUpdateInput::message(
        2001,
        crate::event::ChatContext {
            id: -100123,
            chat_type: "supergroup".to_owned(),
            title: Some("Moderation HQ".to_owned()),
            username: Some("mod_hq".to_owned()),
            photo_file_id: None,
            thread_id: None,
        },
        crate::event::SenderContext {
            id: 77,
            username: Some("seen_user".to_owned()),
            display_name: Some("Seen User".to_owned()),
            first_name: "Seen".to_owned(),
            last_name: Some("User".to_owned()),
            photo_file_id: None,
            is_bot: false,
            is_admin: false,
            role: Some("member".to_owned()),
        },
        MessageContext {
            id: 990,
            date: ts(),
            text: Some("hello".to_owned()),
            content_kind: Some(crate::event::MessageContentKind::Text),
            entities: vec![],
            has_media: false,
            file_ids: Vec::new(),
            reply_to_message_id: None,
            media_group_id: None,
        },
    );
    input.received_at = ts();
    let event = EventNormalizer::new()
        .normalize_telegram(input)
        .expect("event normalizes");

    let result = pipeline.process_event(event).await.expect("process event");
    assert!(matches!(result, IngressProcessResult::Processed));
    let user = storage
        .get_user(77)
        .expect("user lookup")
        .expect("user exists");
    assert_eq!(user.username.as_deref(), Some("seen_user"));
}

#[tokio::test]
async fn process_event_persists_chat_admin_flag_for_member_updates() {
    let dir = tempfile::tempdir().expect("tempdir");
    let storage = Storage::new(dir.path().join("runtime.sqlite3"))
        .bootstrap()
        .expect("bootstrap")
        .into_connection();
    storage
        .set_bot_is_admin(-100123, true)
        .expect("seed bot admin");
    storage
        .set_chat_user_is_admin(-100123, 42, false)
        .expect("seed actor cache");
    let router = std::rc::Rc::new(ExecutionRouter::new(0, false));
    let pipeline = IngressPipeline::new(
        teloxide_core::Bot::new("123456:TEST_TOKEN"),
        storage.clone(),
        router,
    );

    let mut event = EventContext::new(
        "evt_tg_chat_member_admin",
        UpdateType::ChatMember,
        ExecutionMode::Realtime,
        SystemContext::realtime(),
    );
    event.update_id = Some(2002);
    event.received_at = ts();
    event.chat = Some(ChatContext {
        id: -100123,
        chat_type: "supergroup".to_owned(),
        title: Some("Moderation HQ".to_owned()),
        username: Some("mod_hq".to_owned()),
        photo_file_id: None,
        thread_id: None,
    });
    event.sender = Some(SenderContext {
        id: 42,
        username: Some("admin".to_owned()),
        display_name: Some("Admin".to_owned()),
        first_name: "Admin".to_owned(),
        last_name: None,
        photo_file_id: None,
        is_bot: false,
        is_admin: false,
        role: Some("member".to_owned()),
    });
    event.chat_member = Some(MemberContext {
        old_status: "Member".to_owned(),
        new_status: "Administrator(AdminChatMember)".to_owned(),
        user: SenderContext {
            id: 77,
            username: Some("moderator".to_owned()),
            display_name: Some("Moderator".to_owned()),
            first_name: "Moderator".to_owned(),
            last_name: None,
            photo_file_id: None,
            is_bot: false,
            is_admin: false,
            role: Some("administrator".to_owned()),
        },
    });

    let result = pipeline.process_event(event).await.expect("process event");

    assert!(matches!(result, IngressProcessResult::Processed));
    assert_eq!(
        storage
            .get_chat_user_is_admin(-100123, 77)
            .expect("admin lookup"),
        Some(true)
    );
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
