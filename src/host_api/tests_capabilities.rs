use super::super::test_support::{manual_event, storage_api_with_registry};
use super::super::{
    HostApiErrorDetail, HostApiErrorKind, HostApiOperation, MlTranscribeRequest,
    TgSendMessageRequest,
};

#[test]
fn ml_transcribe_requires_ml_stt_capability() {
    let event = manual_event();
    let (_dir, api) = storage_api_with_registry(&["ml.stt"], &[], false);

    let response = api
        .ml_transcribe(
            &event,
            MlTranscribeRequest {
                base_url: None,
                file_id: "voice-123".to_owned(),
            },
        )
        .expect("ml.transcribe succeeds when capability is allowed");

    assert_eq!(response.operation, HostApiOperation::MlTranscribe);
    assert_eq!(response.value.file_id, "voice-123");
    assert_eq!(response.value.text.as_deref(), Some("transcribed text"));
}

#[test]
fn ml_transcribe_denies_when_capability_is_missing() {
    let event = manual_event();
    let (_dir, api) = storage_api_with_registry(&["tg.write_message"], &[], false);

    let error = api
        .ml_transcribe(
            &event,
            MlTranscribeRequest {
                base_url: None,
                file_id: "voice-123".to_owned(),
            },
        )
        .expect_err("missing capability must fail");

    assert_eq!(error.kind, HostApiErrorKind::Denied);
    assert_eq!(error.operation, HostApiOperation::MlTranscribe);
    assert_eq!(
        error.detail,
        HostApiErrorDetail::CapabilityDenied {
            capability: "ml.stt".to_owned(),
            unit_id: "moderation.test".to_owned(),
        }
    );
}

#[test]
fn tg_send_message_requires_tg_write_message_capability() {
    let event = manual_event();
    let (_dir, api) = storage_api_with_registry(&["tg.write_message"], &[], true);

    let response = api
        .tg_send_message(
            &event,
            TgSendMessageRequest {
                chat_id: -100123,
                text: "hello".to_owned(),
            },
        )
        .expect("tg.send_message succeeds when capability is allowed");

    assert_eq!(response.operation, HostApiOperation::TgSendMessage);
    assert!(response.dry_run);
    assert_eq!(response.value.message_id, 1);
}

#[test]
fn tg_send_message_denies_when_capability_is_missing() {
    let event = manual_event();
    let (_dir, api) = storage_api_with_registry(&["ml.stt"], &[], true);

    let error = api
        .tg_send_message(
            &event,
            TgSendMessageRequest {
                chat_id: -100123,
                text: "hello".to_owned(),
            },
        )
        .expect_err("missing capability must fail");

    assert_eq!(error.kind, HostApiErrorKind::Denied);
    assert_eq!(error.operation, HostApiOperation::TgSendMessage);
    assert_eq!(
        error.detail,
        HostApiErrorDetail::CapabilityDenied {
            capability: "tg.write_message".to_owned(),
            unit_id: "moderation.test".to_owned(),
        }
    );
}
