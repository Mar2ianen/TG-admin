use super::test_support::{manual_event, unit_registry_api};
use super::{
    HostApi, HostApiErrorDetail, HostApiErrorKind, HostApiOperation, HostApiRequest, HostApiValue,
    UnitStatusEntry, UnitStatusRequest,
};
use crate::unit::{
    ServiceSpec, TriggerSpec, UnitDefinition, UnitManifest, UnitRegistry, UnitStatus,
};

mod tests_db {
    include!("tests_db.rs");
}

#[path = "tests_ctx.rs"]
mod tests_ctx;

#[path = "tests_audit.rs"]
mod tests_audit;

#[path = "tests_history.rs"]
mod tests_history;

#[test]
fn unit_status_returns_summary_and_specific_entry() {
    let event = manual_event();
    let api = unit_registry_api();

    let response = api
        .unit_status(
            &event,
            UnitStatusRequest {
                unit_id: Some("moderation.warn".to_owned()),
            },
        )
        .expect("unit status succeeds");

    assert_eq!(response.operation, HostApiOperation::UnitStatus);
    assert_eq!(response.value.summary.total_units, 2);
    assert_eq!(response.value.summary.active_units, 1);
    assert_eq!(response.value.summary.disabled_units, 1);
    assert_eq!(
        response.value.unit,
        Some(UnitStatusEntry {
            unit_id: "moderation.warn".to_owned(),
            status: UnitStatus::Active,
            enabled: Some(true),
            diagnostics: Vec::new(),
        })
    );
}

#[test]
fn unit_status_returns_structured_not_found_error() {
    let event = manual_event();
    let api = unit_registry_api();

    let error = api
        .unit_status(
            &event,
            UnitStatusRequest {
                unit_id: Some("missing.unit".to_owned()),
            },
        )
        .expect_err("unknown unit must fail");

    assert_eq!(error.kind, HostApiErrorKind::Validation);
    assert_eq!(
        error.detail,
        HostApiErrorDetail::UnknownUnit {
            unit_id: "missing.unit".to_owned(),
        }
    );
}

#[test]
fn unit_status_preserves_dry_run_metadata() {
    let active = UnitManifest::new(
        UnitDefinition::new("moderation.warn"),
        TriggerSpec::command(["warn"]),
        ServiceSpec::new("cargo run"),
    );
    let report = UnitRegistry::load_manifests(vec![active]);
    let api = HostApi::new(true).with_unit_registry(report.registry);
    let event = manual_event();

    let response = api
        .unit_status(&event, UnitStatusRequest { unit_id: None })
        .expect("unit status succeeds");

    assert!(response.dry_run);
    assert_eq!(response.value.summary.total_units, 1);
}

#[test]
fn call_surface_routes_db_and_unit_requests() {
    let event = manual_event();
    let api = unit_registry_api();

    let response = api
        .call(
            &event,
            HostApiRequest::UnitStatus(UnitStatusRequest { unit_id: None }),
        )
        .expect("typed call succeeds");

    match response.value {
        HostApiValue::UnitStatus(value) => assert_eq!(value.summary.total_units, 2),
        other => panic!("unexpected host api value: {other:?}"),
    }
}
