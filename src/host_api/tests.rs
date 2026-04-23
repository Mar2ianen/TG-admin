use super::test_support::{manual_event, unit_registry_api};
use super::{HostApiRequest, HostApiValue, UnitStatusRequest};

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

#[path = "tests_unit_status.rs"]
mod tests_unit_status;
