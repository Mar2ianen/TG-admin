use super::helpers::{execution_mode_name, trigger_message_id};
use super::types::{AuditEntrySpec, ModerationUnitPolicy};
use crate::event::EventContext;
use crate::storage::AuditLogEntry;
use uuid::Uuid;

pub(crate) fn build_audit_entry(
    event: &EventContext,
    unit_policy: Option<&ModerationUnitPolicy>,
    spec: AuditEntrySpec<'_>,
) -> AuditLogEntry {
    let unit_name = unit_policy
        .map(|policy| policy.unit.id.clone())
        .or_else(|| event.system.unit.as_ref().map(|unit| unit.id.clone()))
        .unwrap_or_else(|| "runtime".to_owned());
    AuditLogEntry {
        action_id: format!("act_{}", Uuid::new_v4().simple()),
        trace_id: event.system.trace_id.clone(),
        request_id: Some(event.event_id.clone()),
        unit_name,
        execution_mode: execution_mode_name(event.execution_mode).to_owned(),
        op: spec.op.to_owned(),
        actor_user_id: event.sender.as_ref().map(|sender| sender.id),
        chat_id: event.chat.as_ref().map(|chat| chat.id),
        target_kind: Some(spec.target.kind.clone()),
        target_id: Some(spec.target.id.clone()),
        trigger_message_id: trigger_message_id(event).map(i64::from),
        idempotency_key: Some(format!("{}:{}", spec.op, event.event_id)),
        reversible: spec.reversible,
        compensation_json: spec
            .compensation
            .map(|recipe| serde_json::to_string(&recipe).expect("compensation recipe serializes")),
        args_json: spec.args_json.to_string(),
        result_json: spec.result_json.map(|value| value.to_string()),
        created_at: event.received_at.to_rfc3339(),
    }
}
