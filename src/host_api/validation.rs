use super::{
    AuditFindRequest, DbUserIncrRequest, HostApiError, HostApiErrorDetail, HostApiOperation,
    JobScheduleAfterRequest, MsgByUserRequest, MsgWindowRequest,
};
use crate::event::{EventContext, ExecutionMode};
use crate::parser::duration::ParsedDuration;
use crate::storage::{KvEntry, StorageError, UserPatch, UserRecord};
use chrono::{DateTime, Duration as ChronoDuration, Utc};

pub(crate) const MAX_MSG_WINDOW: usize = 200;
pub(crate) const MAX_MSG_BY_USER_LIMIT: usize = 200;
pub(crate) const MAX_AUDIT_FIND_LIMIT: usize = 200;
pub(crate) const MAX_JOB_DELAY_DAYS: i64 = 365;

pub(crate) fn required_capability(operation: HostApiOperation) -> Option<&'static str> {
    match operation {
        HostApiOperation::MsgWindow | HostApiOperation::MsgByUser => Some("msg.history.read"),
        HostApiOperation::JobScheduleAfter => Some("job.schedule"),
        HostApiOperation::AuditFind => Some("audit.read"),
        HostApiOperation::AuditCompensate => Some("audit.compensate"),
        HostApiOperation::MlHealth => Some("ml.health.read"),
        HostApiOperation::MlEmbedText => Some("ml.embed_text"),
        HostApiOperation::MlChatCompletions => Some("ml.chat"),
        HostApiOperation::MlModels => Some("ml.models.read"),
        HostApiOperation::CtxCurrent
        | HostApiOperation::CtxResolveTarget
        | HostApiOperation::CtxParseDuration
        | HostApiOperation::CtxExpandReason
        | HostApiOperation::DbUserGet
        | HostApiOperation::DbUserPatch
        | HostApiOperation::DbUserIncr
        | HostApiOperation::DbKvGet
        | HostApiOperation::DbKvSet
        | HostApiOperation::UnitStatus => None,
        HostApiOperation::MlTranscribe | HostApiOperation::TgSendMessage => None,
    }
}

pub(crate) fn validate_event(
    event: &EventContext,
    operation: HostApiOperation,
) -> Result<(), HostApiError> {
    event.validate_invariants().map_err(|source| {
        HostApiError::validation(
            operation,
            HostApiErrorDetail::InvalidEventContext {
                message: source.to_string(),
            },
        )
    })
}

pub(crate) fn validate_user_id(
    user_id: i64,
    operation: HostApiOperation,
) -> Result<(), HostApiError> {
    if user_id == 0 {
        return Err(HostApiError::validation(
            operation,
            HostApiErrorDetail::InvalidField {
                field: "user_id".to_owned(),
                message: "must be non-zero".to_owned(),
            },
        ));
    }

    Ok(())
}

pub(crate) fn validate_non_empty(
    value: &str,
    field: &'static str,
    operation: HostApiOperation,
) -> Result<(), HostApiError> {
    if value.trim().is_empty() {
        return Err(HostApiError::validation(
            operation,
            HostApiErrorDetail::InvalidField {
                field: field.to_owned(),
                message: "must not be blank".to_owned(),
            },
        ));
    }

    Ok(())
}

pub(crate) fn validate_user_patch(
    patch: &UserPatch,
    operation: HostApiOperation,
) -> Result<(), HostApiError> {
    validate_user_id(patch.user_id, operation)?;
    validate_non_empty(&patch.seen_at, "seen_at", operation)?;
    validate_non_empty(&patch.updated_at, "updated_at", operation)?;
    if let Some(warn_count) = patch.warn_count
        && warn_count < 0
    {
        return Err(HostApiError::validation(
            operation,
            HostApiErrorDetail::InvalidField {
                field: "warn_count".to_owned(),
                message: "must be non-negative".to_owned(),
            },
        ));
    }

    Ok(())
}

pub(crate) fn validate_user_incr_request(
    request: &DbUserIncrRequest,
    operation: HostApiOperation,
) -> Result<(), HostApiError> {
    validate_user_id(request.user_id, operation)?;
    validate_non_empty(&request.seen_at, "seen_at", operation)?;
    validate_non_empty(&request.updated_at, "updated_at", operation)?;
    Ok(())
}

pub(crate) fn validate_kv_key(
    scope_kind: &str,
    scope_id: &str,
    key: &str,
    operation: HostApiOperation,
) -> Result<(), HostApiError> {
    validate_non_empty(scope_kind, "scope_kind", operation)?;
    validate_non_empty(scope_id, "scope_id", operation)?;
    validate_non_empty(key, "key", operation)?;
    Ok(())
}

pub(crate) fn validate_kv_entry(
    entry: &KvEntry,
    operation: HostApiOperation,
) -> Result<(), HostApiError> {
    validate_kv_key(&entry.scope_kind, &entry.scope_id, &entry.key, operation)?;
    validate_non_empty(&entry.value_json, "value_json", operation)?;
    validate_non_empty(&entry.updated_at, "updated_at", operation)?;
    Ok(())
}

pub(crate) fn validate_msg_window_request(
    request: &MsgWindowRequest,
    operation: HostApiOperation,
) -> Result<(), HostApiError> {
    if request.chat_id == 0 {
        return Err(HostApiError::validation(
            operation,
            HostApiErrorDetail::InvalidField {
                field: "chat_id".to_owned(),
                message: "must be non-zero".to_owned(),
            },
        ));
    }
    if request.anchor_message_id <= 0 {
        return Err(HostApiError::validation(
            operation,
            HostApiErrorDetail::InvalidField {
                field: "anchor_message_id".to_owned(),
                message: "must be positive".to_owned(),
            },
        ));
    }
    let total = request.up + request.down + usize::from(request.include_anchor);
    if total > MAX_MSG_WINDOW {
        return Err(HostApiError::validation(
            operation,
            HostApiErrorDetail::MessageWindowTooLarge {
                requested: total,
                max: MAX_MSG_WINDOW,
            },
        ));
    }

    Ok(())
}

pub(crate) fn validate_msg_by_user_request(
    request: &MsgByUserRequest,
    operation: HostApiOperation,
) -> Result<(), HostApiError> {
    if request.chat_id == 0 {
        return Err(HostApiError::validation(
            operation,
            HostApiErrorDetail::InvalidField {
                field: "chat_id".to_owned(),
                message: "must be non-zero".to_owned(),
            },
        ));
    }
    validate_user_id(request.user_id, operation)?;
    validate_non_empty(&request.since, "since", operation)?;
    parse_rfc3339(&request.since, operation, "since")?;
    if request.limit == 0 || request.limit > MAX_MSG_BY_USER_LIMIT {
        return Err(HostApiError::validation(
            operation,
            HostApiErrorDetail::InvalidField {
                field: "limit".to_owned(),
                message: format!("must be between 1 and {MAX_MSG_BY_USER_LIMIT}"),
            },
        ));
    }

    Ok(())
}

pub(crate) fn validate_job_schedule_request(
    request: &JobScheduleAfterRequest,
    operation: HostApiOperation,
) -> Result<(), HostApiError> {
    validate_non_empty(&request.delay, "delay", operation)?;
    validate_non_empty(&request.executor_unit, "executor_unit", operation)?;
    if let Some(dedupe_key) = request.dedupe_key.as_deref() {
        validate_non_empty(dedupe_key, "dedupe_key", operation)?;
    }
    if let Some(audit_action_id) = request.audit_action_id.as_deref() {
        validate_non_empty(audit_action_id, "audit_action_id", operation)?;
    }
    if let Some(max_retries) = request.max_retries
        && max_retries < 0
    {
        return Err(HostApiError::validation(
            operation,
            HostApiErrorDetail::InvalidField {
                field: "max_retries".to_owned(),
                message: "must be non-negative".to_owned(),
            },
        ));
    }

    Ok(())
}

pub(crate) fn validate_audit_find_request(
    request: &AuditFindRequest,
    operation: HostApiOperation,
) -> Result<(), HostApiError> {
    if request.limit == 0 || request.limit > MAX_AUDIT_FIND_LIMIT {
        return Err(HostApiError::validation(
            operation,
            HostApiErrorDetail::InvalidField {
                field: "limit".to_owned(),
                message: format!("must be between 1 and {MAX_AUDIT_FIND_LIMIT}"),
            },
        ));
    }
    if request.filters.action_id.is_none()
        && request.filters.trace_id.is_none()
        && request.filters.request_id.is_none()
        && request.filters.idempotency_key.is_none()
        && request.filters.trigger_message_id.is_none()
        && request.filters.actor_user_id.is_none()
        && request.filters.chat_id.is_none()
        && request.filters.op.is_none()
        && request.filters.target_id.is_none()
        && request.filters.reversible.is_none()
    {
        return Err(HostApiError::validation(
            operation,
            HostApiErrorDetail::MissingAuditFilter,
        ));
    }

    validate_optional_non_empty(&request.filters.action_id, "filters.action_id", operation)?;
    validate_optional_non_empty(&request.filters.trace_id, "filters.trace_id", operation)?;
    validate_optional_non_empty(&request.filters.request_id, "filters.request_id", operation)?;
    validate_optional_non_empty(
        &request.filters.idempotency_key,
        "filters.idempotency_key",
        operation,
    )?;
    validate_optional_non_empty(&request.filters.op, "filters.op", operation)?;
    validate_optional_non_empty(&request.filters.target_id, "filters.target_id", operation)?;

    Ok(())
}

pub(crate) fn validate_optional_non_empty(
    value: &Option<String>,
    field: &'static str,
    operation: HostApiOperation,
) -> Result<(), HostApiError> {
    if let Some(value) = value.as_deref() {
        validate_non_empty(value, field, operation)?;
    }

    Ok(())
}

pub(crate) fn storage_error(operation: HostApiOperation, source: StorageError) -> HostApiError {
    HostApiError::internal(
        operation,
        HostApiErrorDetail::StorageFailure {
            message: source.to_string(),
        },
    )
}

pub(crate) fn duration_to_chrono(
    parsed: ParsedDuration,
    operation: HostApiOperation,
) -> Result<ChronoDuration, HostApiError> {
    ChronoDuration::from_std(parsed.into_std()).map_err(|error| {
        HostApiError::internal(
            operation,
            HostApiErrorDetail::InternalConversionFailure {
                message: error.to_string(),
            },
        )
    })
}

pub(crate) fn to_rfc3339(value: DateTime<Utc>) -> String {
    value.to_rfc3339()
}

pub(crate) fn parse_rfc3339(
    value: &str,
    operation: HostApiOperation,
    field: &'static str,
) -> Result<DateTime<Utc>, HostApiError> {
    DateTime::parse_from_rfc3339(value)
        .map(|parsed| parsed.with_timezone(&Utc))
        .map_err(|error| {
            HostApiError::validation(
                operation,
                HostApiErrorDetail::InvalidField {
                    field: field.to_owned(),
                    message: format!("must be RFC3339 timestamp: {error}"),
                },
            )
        })
}

pub(crate) fn execution_mode_label(event: &EventContext) -> String {
    match event.execution_mode {
        ExecutionMode::Realtime => "realtime",
        ExecutionMode::Recovery => "recovery",
        ExecutionMode::Scheduled => "scheduled",
        ExecutionMode::Manual => "manual",
    }
    .to_owned()
}

pub(crate) fn apply_user_patch(current: Option<&UserRecord>, patch: &UserPatch) -> UserRecord {
    let first_seen_at = match current {
        Some(existing) if existing.first_seen_at < patch.seen_at => existing.first_seen_at.clone(),
        _ => patch.seen_at.clone(),
    };
    let last_seen_at = match current {
        Some(existing) if existing.last_seen_at > patch.seen_at => existing.last_seen_at.clone(),
        _ => patch.seen_at.clone(),
    };

    UserRecord {
        user_id: patch.user_id,
        username: patch
            .username
            .clone()
            .or_else(|| current.and_then(|existing| existing.username.clone())),
        display_name: patch
            .display_name
            .clone()
            .or_else(|| current.and_then(|existing| existing.display_name.clone())),
        first_seen_at,
        last_seen_at,
        warn_count: patch
            .warn_count
            .unwrap_or_else(|| current.map_or(0, |existing| existing.warn_count)),
        shadowbanned: patch
            .shadowbanned
            .unwrap_or_else(|| current.is_some_and(|existing| existing.shadowbanned)),
        reputation: patch
            .reputation
            .unwrap_or_else(|| current.map_or(0, |existing| existing.reputation)),
        state_json: patch
            .state_json
            .clone()
            .or_else(|| current.and_then(|existing| existing.state_json.clone())),
        updated_at: patch.updated_at.clone(),
    }
}

pub(crate) fn user_patch_from_increment(
    current: Option<&UserRecord>,
    request: &DbUserIncrRequest,
    operation: HostApiOperation,
) -> Result<UserPatch, HostApiError> {
    let current_warn_count = current.map_or(0, |user| user.warn_count);
    let warn_count = current_warn_count
        .checked_add(request.warn_count_delta)
        .ok_or_else(|| {
            counter_error(
                operation,
                "warn_count",
                current_warn_count,
                request.warn_count_delta,
            )
        })?;
    if warn_count < 0 {
        return Err(counter_error(
            operation,
            "warn_count",
            current_warn_count,
            request.warn_count_delta,
        ));
    }

    let current_reputation = current.map_or(0, |user| user.reputation);
    let reputation = current_reputation
        .checked_add(request.reputation_delta)
        .ok_or_else(|| {
            counter_error(
                operation,
                "reputation",
                current_reputation,
                request.reputation_delta,
            )
        })?;

    Ok(UserPatch {
        user_id: request.user_id,
        username: request.username.clone(),
        display_name: request.display_name.clone(),
        seen_at: request.seen_at.clone(),
        warn_count: Some(warn_count),
        shadowbanned: request.shadowbanned,
        reputation: Some(reputation),
        state_json: request.state_json.clone(),
        updated_at: request.updated_at.clone(),
    })
}

pub(crate) fn counter_error(
    operation: HostApiOperation,
    field: &'static str,
    current: i64,
    delta: i64,
) -> HostApiError {
    HostApiError::validation(
        operation,
        HostApiErrorDetail::InvalidCounterChange {
            field: field.to_owned(),
            current,
            delta,
        },
    )
}
