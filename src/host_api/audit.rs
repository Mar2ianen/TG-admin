use chrono::Duration as ChronoDuration;
use serde_json::{Value, json};
use uuid::Uuid;

use super::validation::{
    MAX_JOB_DELAY_DAYS, duration_to_chrono, validate_audit_find_request, validate_event,
    validate_job_schedule_request, validate_non_empty,
};
use super::{
    AuditCompensateRequest, AuditCompensateValue, AuditFindRequest, AuditFindValue, HostApi,
    HostApiError, HostApiErrorDetail, HostApiOperation, HostApiResponse, JobScheduleAfterRequest,
    JobScheduleAfterValue, execution_mode_label, storage_error, to_rfc3339,
};
use crate::event::EventContext;
use crate::parser::duration::parse_duration;
use crate::storage::{AuditLogEntry, JobRecord};

impl HostApi {
    pub fn job_schedule_after(
        &self,
        event: &EventContext,
        request: JobScheduleAfterRequest,
    ) -> Result<HostApiResponse<JobScheduleAfterValue>, HostApiError> {
        validate_event(event, HostApiOperation::JobScheduleAfter)?;
        self.require_operation_capability(event, HostApiOperation::JobScheduleAfter)?;
        validate_job_schedule_request(&request, HostApiOperation::JobScheduleAfter)?;

        let parsed_delay = parse_duration(request.delay.trim()).map_err(|source| {
            HostApiError::parse(
                HostApiOperation::JobScheduleAfter,
                HostApiErrorDetail::InvalidDuration {
                    value: request.delay.clone(),
                    source,
                },
            )
        })?;
        let delay = duration_to_chrono(parsed_delay, HostApiOperation::JobScheduleAfter)?;
        if delay > ChronoDuration::days(MAX_JOB_DELAY_DAYS) {
            return Err(HostApiError::validation(
                HostApiOperation::JobScheduleAfter,
                HostApiErrorDetail::JobTooFarInFuture {
                    delay: request.delay,
                    max_days: MAX_JOB_DELAY_DAYS,
                },
            ));
        }

        let scheduled_at = event.received_at;
        let run_at = scheduled_at + delay;
        let job = JobRecord {
            job_id: format!("job_{}", Uuid::new_v4().simple()),
            executor_unit: request.executor_unit,
            run_at: to_rfc3339(run_at),
            scheduled_at: to_rfc3339(scheduled_at),
            status: "scheduled".to_owned(),
            dedupe_key: request.dedupe_key,
            payload_json: request.payload.to_string(),
            retry_count: 0,
            max_retries: request.max_retries.unwrap_or(0),
            last_error_code: None,
            last_error_text: None,
            audit_action_id: request.audit_action_id,
            created_at: to_rfc3339(scheduled_at),
            updated_at: to_rfc3339(scheduled_at),
        };

        if !self.dry_run {
            self.storage(HostApiOperation::JobScheduleAfter)?
                .insert_job(&job)
                .map_err(|source| storage_error(HostApiOperation::JobScheduleAfter, source))?;
        }

        Ok(self.response(
            HostApiOperation::JobScheduleAfter,
            JobScheduleAfterValue { job },
        ))
    }

    pub fn audit_find(
        &self,
        event: &EventContext,
        request: AuditFindRequest,
    ) -> Result<HostApiResponse<AuditFindValue>, HostApiError> {
        validate_event(event, HostApiOperation::AuditFind)?;
        self.require_operation_capability(event, HostApiOperation::AuditFind)?;
        validate_audit_find_request(&request, HostApiOperation::AuditFind)?;

        let entries = self
            .storage(HostApiOperation::AuditFind)?
            .find_audit_entries(&request.filters, request.limit)
            .map_err(|source| storage_error(HostApiOperation::AuditFind, source))?;

        Ok(self.response(HostApiOperation::AuditFind, AuditFindValue { entries }))
    }

    pub fn audit_compensate(
        &self,
        event: &EventContext,
        request: AuditCompensateRequest,
    ) -> Result<HostApiResponse<AuditCompensateValue>, HostApiError> {
        validate_event(event, HostApiOperation::AuditCompensate)?;
        self.require_operation_capability(event, HostApiOperation::AuditCompensate)?;
        validate_non_empty(
            &request.action_id,
            "action_id",
            HostApiOperation::AuditCompensate,
        )?;

        let storage = self.storage(HostApiOperation::AuditCompensate)?;
        let original = storage
            .get_audit_entry(&request.action_id)
            .map_err(|source| storage_error(HostApiOperation::AuditCompensate, source))?
            .ok_or_else(|| {
                HostApiError::validation(
                    HostApiOperation::AuditCompensate,
                    HostApiErrorDetail::UnknownAuditAction {
                        action_id: request.action_id.clone(),
                    },
                )
            })?;

        if !original.reversible {
            return Err(HostApiError::validation(
                HostApiOperation::AuditCompensate,
                HostApiErrorDetail::InvalidField {
                    field: "action_id".to_owned(),
                    message: format!("audit action `{}` is not reversible", request.action_id),
                },
            ));
        }
        let compensation_recipe = original.compensation_json.as_deref().ok_or_else(|| {
            HostApiError::validation(
                HostApiOperation::AuditCompensate,
                HostApiErrorDetail::InvalidField {
                    field: "action_id".to_owned(),
                    message: format!(
                        "audit action `{}` has no compensation recipe",
                        request.action_id
                    ),
                },
            )
        })?;
        serde_json::from_str::<Value>(compensation_recipe).map_err(|source| {
            HostApiError::validation(
                HostApiOperation::AuditCompensate,
                HostApiErrorDetail::InvalidField {
                    field: "compensation_json".to_owned(),
                    message: format!("invalid compensation recipe: {source}"),
                },
            )
        })?;

        let compensation_idempotency_key = format!("compensate:{}", original.action_id);
        let existing_compensations = storage
            .find_audit_by_idempotency_key(&compensation_idempotency_key)
            .map_err(|source| storage_error(HostApiOperation::AuditCompensate, source))?;
        if !existing_compensations.is_empty() {
            return Err(HostApiError::validation(
                HostApiOperation::AuditCompensate,
                HostApiErrorDetail::InvalidField {
                    field: "action_id".to_owned(),
                    message: format!(
                        "audit action `{}` is already compensated",
                        request.action_id
                    ),
                },
            ));
        }

        let new_action_id = format!("act_{}", Uuid::new_v4().simple());
        let compensation_entry = AuditLogEntry {
            action_id: new_action_id.clone(),
            trace_id: event
                .system
                .trace_id
                .clone()
                .or_else(|| original.trace_id.clone()),
            request_id: None,
            unit_name: event
                .system
                .unit
                .as_ref()
                .map(|unit| unit.id.clone())
                .unwrap_or_else(|| original.unit_name.clone()),
            execution_mode: execution_mode_label(event),
            op: "audit.compensate".to_owned(),
            actor_user_id: event
                .sender
                .as_ref()
                .map(|sender| sender.id)
                .or(original.actor_user_id),
            chat_id: event.chat.as_ref().map(|chat| chat.id).or(original.chat_id),
            target_kind: Some("audit_action".to_owned()),
            target_id: Some(original.action_id.clone()),
            trigger_message_id: event.message.as_ref().map(|message| i64::from(message.id)),
            idempotency_key: Some(compensation_idempotency_key),
            reversible: false,
            compensation_json: None,
            args_json: json!({
                "action_id": original.action_id,
                "recipe": compensation_recipe,
            })
            .to_string(),
            result_json: Some(json!({ "compensated": true }).to_string()),
            created_at: to_rfc3339(event.received_at),
        };

        if !self.dry_run {
            storage
                .append_audit_entry(&compensation_entry)
                .map_err(|source| storage_error(HostApiOperation::AuditCompensate, source))?;
        }

        Ok(self.response(
            HostApiOperation::AuditCompensate,
            AuditCompensateValue {
                compensated: true,
                new_action_id: Some(new_action_id),
            },
        ))
    }
}
