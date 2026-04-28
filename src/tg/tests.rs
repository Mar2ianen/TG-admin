use super::{
    NoopTelegramTransport, ParseMode, TelegramDeleteManyRequest, TelegramErrorKind,
    TelegramExecutionOptions, TelegramGateway, TelegramMessageResult, TelegramOperation,
    TelegramRequest, TelegramResult, TelegramTransport, TelegramUiResult,
};
use crate::storage::Storage;
use async_trait::async_trait;
use serde_json::{json, to_value};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use uuid::Uuid;

#[derive(Debug)]
struct StaticTransport {
    result: TelegramResult,
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl TelegramTransport for StaticTransport {
    fn name(&self) -> &'static str {
        "static"
    }

    async fn execute(
        &self,
        _request: TelegramRequest,
    ) -> Result<TelegramResult, super::TelegramError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.result.clone())
    }
}

#[derive(Debug)]
struct FailingTransport {
    error: super::TelegramError,
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl TelegramTransport for FailingTransport {
    fn name(&self) -> &'static str {
        "failing"
    }

    async fn execute(
        &self,
        _request: TelegramRequest,
    ) -> Result<TelegramResult, super::TelegramError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(self.error.clone())
    }
}

#[test]
fn gateway_defaults_to_noop_transport() {
    let gateway = TelegramGateway::default();

    assert!(gateway.polling());
    assert_eq!(gateway.transport_name(), "noop");
    assert_eq!(
        format!("{gateway:?}"),
        r#"TelegramGateway { polling: true, transport: "noop" }"#
    );
}

#[tokio::test]
async fn noop_transport_returns_typed_error() {
    let transport = NoopTelegramTransport;
    let error = transport
        .execute(TelegramRequest::SendMessage(
            super::TelegramSendMessageRequest {
                chat_id: -100,
                text: "hello".to_owned(),
                reply_to_message_id: None,
                silent: false,
                parse_mode: ParseMode::PlainText,
                markup: None,
            },
        ))
        .await
        .expect_err("noop transport should fail");

    assert_eq!(error.kind, TelegramErrorKind::TransportUnavailable);
    assert_eq!(error.operation, TelegramOperation::SendMessage);
}

#[test]
fn delete_many_request_serializes_with_canonical_op_tag() {
    let request = TelegramRequest::DeleteMany(TelegramDeleteManyRequest {
        chat_id: -100,
        message_ids: vec![10, 11, 12],
        idempotency_key: Some("del:-100:10-12".to_owned()),
    });

    let json = to_value(&request).expect("request serializes");
    assert_eq!(json["op"], "tg.delete_many");
    assert_eq!(json["chat_id"], -100);
    assert_eq!(json["message_ids"], json!([10, 11, 12]));
    assert_eq!(request.idempotency_key(), Some("del:-100:10-12"));
    assert!(request.operation().requires_idempotency());
}

#[test]
fn result_accessors_return_normalized_identifiers() {
    let message = TelegramResult::Message(TelegramMessageResult {
        chat_id: -100,
        message_id: 42,
        raw_passthrough: false,
    });
    let ui = TelegramResult::Ui(TelegramUiResult {
        chat_id: -100,
        message_id: 43,
        template: "moderation/warn.md".to_owned(),
        edited: true,
        raw_passthrough: false,
    });

    assert_eq!(message.chat_id(), Some(-100));
    assert_eq!(message.message_id(), Some(42));
    assert_eq!(message.operation_kind(), super::TelegramResultKind::Message);
    assert_eq!(ui.chat_id(), Some(-100));
    assert_eq!(ui.message_id(), Some(43));
    assert_eq!(ui.operation_kind(), super::TelegramResultKind::Ui);
}

#[tokio::test]
async fn gateway_dispatches_to_custom_transport() {
    let gateway = TelegramGateway::new(false).with_transport(StaticTransport {
        result: TelegramResult::Ui(TelegramUiResult {
            chat_id: -100,
            message_id: 81,
            template: "ui/session.md".to_owned(),
            edited: false,
            raw_passthrough: false,
        }),
        calls: Arc::new(AtomicUsize::new(0)),
    });

    let result = gateway
        .execute(TelegramRequest::SendUi(super::TelegramSendUiRequest {
            chat_id: -100,
            template: "ui/session.md".to_owned(),
            data: json!({"target":"@spam_user"}),
            reply_to_message_id: Some(80),
            silent: true,
            parse_mode: ParseMode::MarkdownV2,
            markup: None,
        }))
        .await
        .expect("transport should succeed");

    assert!(!gateway.polling());
    assert_eq!(gateway.transport_name(), "static");
    assert_eq!(result.message_id(), Some(81));
}

#[tokio::test]
async fn execute_checked_rejects_missing_idempotency_for_destructive_ops() {
    let gateway = TelegramGateway::default();

    let error = gateway
        .execute_checked(
            TelegramRequest::DeleteMany(TelegramDeleteManyRequest {
                chat_id: -100,
                message_ids: vec![10, 11],
                idempotency_key: None,
            }),
            TelegramExecutionOptions::default(),
        )
        .await
        .expect_err("destructive op without idempotency must fail");

    assert_eq!(error.kind, TelegramErrorKind::Validation);
    assert_eq!(error.operation, TelegramOperation::DeleteMany);
}

#[tokio::test]
async fn execute_checked_dry_run_predicts_without_transport_call() {
    let calls = Arc::new(AtomicUsize::new(0));
    let gateway = TelegramGateway::default().with_transport(StaticTransport {
        result: TelegramResult::Delete(super::TelegramDeleteResult {
            chat_id: -100,
            deleted: vec![77],
            failed: Vec::new(),
        }),
        calls: Arc::clone(&calls),
    });

    let execution = gateway
        .execute_checked(
            TelegramRequest::Delete(super::TelegramDeleteRequest {
                chat_id: -100,
                message_id: 77,
                idempotency_key: Some("delete:-100:77".to_owned()),
            }),
            TelegramExecutionOptions { dry_run: true },
        )
        .await
        .expect("dry run must succeed");

    assert!(execution.metadata.dry_run);
    assert!(!execution.metadata.replayed);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(execution.result.chat_id(), Some(-100));
}

#[tokio::test]
async fn execute_checked_replays_cached_idempotent_result() {
    let calls = Arc::new(AtomicUsize::new(0));
    let gateway = TelegramGateway::default().with_transport(StaticTransport {
        result: TelegramResult::Delete(super::TelegramDeleteResult {
            chat_id: -100,
            deleted: vec![77, 78],
            failed: Vec::new(),
        }),
        calls: Arc::clone(&calls),
    });

    let request = TelegramRequest::DeleteMany(TelegramDeleteManyRequest {
        chat_id: -100,
        message_ids: vec![77, 78],
        idempotency_key: Some("del-window:-100:77-78".to_owned()),
    });

    let first = gateway
        .execute_checked(request.clone(), TelegramExecutionOptions::default())
        .await
        .expect("first call succeeds");
    let second = gateway
        .execute_checked(request, TelegramExecutionOptions::default())
        .await
        .expect("second call succeeds");

    assert!(!first.metadata.replayed);
    assert!(second.metadata.replayed);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(first.result, second.result);
}

#[tokio::test]
async fn execute_checked_replays_persisted_result_after_gateway_recreate() {
    let calls = Arc::new(AtomicUsize::new(0));
    let storage = bootstrapped_storage();
    let transport = StaticTransport {
        result: TelegramResult::Delete(super::TelegramDeleteResult {
            chat_id: -100,
            deleted: vec![77, 78],
            failed: Vec::new(),
        }),
        calls: Arc::clone(&calls),
    };

    let request = TelegramRequest::DeleteMany(TelegramDeleteManyRequest {
        chat_id: -100,
        message_ids: vec![77, 78],
        idempotency_key: Some("del-window:-100:77-78".to_owned()),
    });

    let first_gateway = TelegramGateway::default()
        .with_idempotency_storage(storage.clone())
        .with_transport(transport);
    let first = first_gateway
        .execute_checked(request.clone(), TelegramExecutionOptions::default())
        .await
        .expect("first call succeeds");

    let second_gateway = TelegramGateway::default()
        .with_idempotency_storage(storage)
        .with_transport(StaticTransport {
            result: TelegramResult::Delete(super::TelegramDeleteResult {
                chat_id: -100,
                deleted: vec![1],
                failed: Vec::new(),
            }),
            calls: Arc::clone(&calls),
        });
    let second = second_gateway
        .execute_checked(request, TelegramExecutionOptions::default())
        .await
        .expect("replayed call succeeds");

    assert!(!first.metadata.replayed);
    assert!(second.metadata.replayed);
    assert_eq!(first.result, second.result);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn execute_checked_fails_closed_for_in_progress_external_effect() {
    let storage = bootstrapped_storage();
    let request = TelegramRequest::DeleteMany(TelegramDeleteManyRequest {
        chat_id: -100,
        message_ids: vec![10, 11],
        idempotency_key: Some("del-window:-100:10-11".to_owned()),
    });
    let now = "2026-04-21T17:30:00Z";
    let connection = storage
        .open()
        .expect("failed to open storage for seeding external effect");
    connection
        .reserve_external_effect(&crate::storage::ExternalEffectRecord {
            idempotency_key: String::from("del-window:-100:10-11"),
            operation: String::from(TelegramOperation::DeleteMany.as_str()),
            request_json: serde_json::to_string(&request).expect("request json serializes"),
            result_json: None,
            status: String::from(crate::storage::EXTERNAL_EFFECT_STATUS_IN_PROGRESS),
            created_at: String::from(now),
            updated_at: String::from(now),
            error_json: None,
        })
        .expect("failed to seed in-progress external effect");

    let calls = Arc::new(AtomicUsize::new(0));
    let gateway = TelegramGateway::default()
        .with_idempotency_storage(storage)
        .with_transport(StaticTransport {
            result: TelegramResult::Delete(super::TelegramDeleteResult {
                chat_id: -100,
                deleted: vec![10, 11],
                failed: Vec::new(),
            }),
            calls: Arc::clone(&calls),
        });

    let error = gateway
        .execute_checked(request, TelegramExecutionOptions::default())
        .await
        .expect_err("in-progress external effect must fail closed");

    assert_eq!(error.kind, TelegramErrorKind::Conflict);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn execute_checked_fails_closed_after_transport_error_is_persisted() {
    let storage = bootstrapped_storage();
    let calls = Arc::new(AtomicUsize::new(0));
    let transport_error = super::TelegramError::new(
        TelegramOperation::Delete,
        TelegramErrorKind::TransportUnavailable,
        "telegram transport down",
    )
    .with_retryable(true);

    let gateway = TelegramGateway::default()
        .with_idempotency_storage(storage.clone())
        .with_transport(FailingTransport {
            error: transport_error.clone(),
            calls: Arc::clone(&calls),
        });

    let request = TelegramRequest::Delete(super::TelegramDeleteRequest {
        chat_id: -100,
        message_id: 77,
        idempotency_key: Some("delete:-100:77".to_owned()),
    });

    let first_error = gateway
        .execute_checked(request.clone(), TelegramExecutionOptions::default())
        .await
        .expect_err("transport error must be returned");
    assert_eq!(first_error, transport_error);
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    let replay_gateway = TelegramGateway::default()
        .with_idempotency_storage(storage)
        .with_transport(FailingTransport {
            error: transport_error,
            calls: Arc::clone(&calls),
        });

    let replay_error = replay_gateway
        .execute_checked(request, TelegramExecutionOptions::default())
        .await
        .expect_err("persisted transport error must fail closed");

    assert_eq!(replay_error.kind, TelegramErrorKind::Conflict);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn execute_checked_validates_non_empty_message_text() {
    let gateway = TelegramGateway::default();
    let error = gateway
        .execute_checked(
            TelegramRequest::SendMessage(super::TelegramSendMessageRequest {
                chat_id: -100,
                text: "   ".to_owned(),
                reply_to_message_id: None,
                silent: false,
                parse_mode: ParseMode::PlainText,
                markup: None,
            }),
            TelegramExecutionOptions::default(),
        )
        .await
        .expect_err("empty text must fail");

    assert_eq!(error.kind, TelegramErrorKind::Validation);
    assert_eq!(error.operation, TelegramOperation::SendMessage);
}

#[tokio::test]
async fn execute_checked_dry_run_rejects_zero_chat_id() {
    let gateway = TelegramGateway::default();

    let error = gateway
        .execute_checked(
            TelegramRequest::SendMessage(super::TelegramSendMessageRequest {
                chat_id: 0,
                text: "hello".to_owned(),
                reply_to_message_id: None,
                silent: false,
                parse_mode: ParseMode::PlainText,
                markup: None,
            }),
            TelegramExecutionOptions { dry_run: true },
        )
        .await
        .expect_err("zero chat id must fail before prediction");

    assert_eq!(error.kind, TelegramErrorKind::Validation);
    assert_eq!(error.operation, TelegramOperation::SendMessage);
    assert_eq!(
        error.details,
        Some(json!({
            "field": "chat_id",
        }))
    );
}

#[tokio::test]
async fn execute_checked_live_rejects_zero_chat_id_before_transport() {
    let calls = Arc::new(AtomicUsize::new(0));
    let gateway = TelegramGateway::default().with_transport(StaticTransport {
        result: TelegramResult::Message(TelegramMessageResult {
            chat_id: -100,
            message_id: 1,
            raw_passthrough: false,
        }),
        calls: Arc::clone(&calls),
    });

    let error = gateway
        .execute_checked(
            TelegramRequest::SendMessage(super::TelegramSendMessageRequest {
                chat_id: 0,
                text: "hello".to_owned(),
                reply_to_message_id: None,
                silent: false,
                parse_mode: ParseMode::PlainText,
                markup: None,
            }),
            TelegramExecutionOptions::default(),
        )
        .await
        .expect_err("zero chat id must fail before transport");

    assert_eq!(error.kind, TelegramErrorKind::Validation);
    assert_eq!(error.operation, TelegramOperation::SendMessage);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

fn bootstrapped_storage() -> Storage {
    let dir = std::env::temp_dir().join(format!("teloxide_tg_test_{}", Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("failed to create temp dir");
    let storage = Storage::new(dir.join("runtime.sqlite3"));
    let bootstrap = storage
        .bootstrap()
        .expect("failed to bootstrap test storage");
    drop(bootstrap);
    storage
}
