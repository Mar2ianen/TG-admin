use crate::host_api::test_support::{manual_event, storage_api};
use crate::moderation::{ModerationEngine, ModerationEngineConfig};
use crate::router::ModerationRouter;

#[tokio::test]
async fn test_moderation_full_flow() {
    let (_tmp, api) = storage_api();
    let engine = ModerationEngine::new(
        ModerationEngineConfig::default(),
        api.storage(crate::host_api::HostApiOperation::Storage)
            .unwrap(),
        api.gateway().clone(),
        api.unit_registry().clone(),
    );

    let event = manual_event();

    // Simulate Router classification and dispatch
    let router = ModerationRouter::new(api.unit_registry().clone());
    let dispatch_set = router.classify(&event);

    assert!(!dispatch_set.is_empty(), "event should be classified");

    // Execute built-in moderation
    let result = engine.execute(&event, &dispatch_set, false).await;

    assert!(
        result.is_ok(),
        "execution should succeed: {:?}",
        result.err()
    );

    // Verify audit entry
    let storage = api
        .storage(crate::host_api::HostApiOperation::AuditFind)
        .unwrap();
    let audit = storage.get_audit_entries(None, None, None).unwrap();
    assert!(!audit.is_empty(), "audit entry should exist");
}
