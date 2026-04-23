use super::ml::{
    MlChatCompletionsRequest, MlChatCompletionsValue, MlEmbedTextRequest, MlEmbedTextValue,
    MlHealthRequest, MlHealthValue, MlModelsRequest, MlModelsValue,
};
use crate::event::EventContext;
use crate::parser::command::ReasonExpr;
use crate::parser::duration::{DurationParseError, ParsedDuration};
use crate::parser::reason::ExpandedReason;
use crate::parser::target::{ParsedTargetSelector, ResolvedTarget, TargetParseError};
use crate::storage::{
    AuditLogEntry, AuditLogFilter, JobRecord, KvEntry, MessageJournalRecord, UserPatch, UserRecord,
};
use crate::unit::{UnitDescriptor, UnitDiagnostic, UnitRegistryStatus, UnitStatus};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub enum HostApiRequest {
    CtxCurrent,
    CtxResolveTarget(CtxResolveTargetRequest),
    CtxParseDuration(CtxParseDurationRequest),
    CtxExpandReason(CtxExpandReasonRequest),
    DbUserGet(DbUserGetRequest),
    DbUserPatch(DbUserPatchRequest),
    DbUserIncr(DbUserIncrRequest),
    DbKvGet(DbKvGetRequest),
    DbKvSet(DbKvSetRequest),
    MsgWindow(MsgWindowRequest),
    MsgByUser(MsgByUserRequest),
    JobScheduleAfter(JobScheduleAfterRequest),
    AuditFind(AuditFindRequest),
    AuditCompensate(AuditCompensateRequest),
    UnitStatus(UnitStatusRequest),
    MlHealth(MlHealthRequest),
    MlEmbedText(MlEmbedTextRequest),
    MlChatCompletions(MlChatCompletionsRequest),
    MlModels(MlModelsRequest),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HostApiValue {
    CtxCurrent(Box<CtxCurrentValue>),
    ResolvedTarget(ResolvedTarget),
    ParsedDuration(ParsedDuration),
    ExpandedReason(ExpandedReason),
    DbUserGet(DbUserGetValue),
    DbUserPatch(DbUserPatchValue),
    DbUserIncr(DbUserIncrValue),
    DbKvGet(DbKvGetValue),
    DbKvSet(DbKvSetValue),
    MsgWindow(MsgWindowValue),
    MsgByUser(MsgByUserValue),
    JobScheduleAfter(JobScheduleAfterValue),
    AuditFind(AuditFindValue),
    AuditCompensate(AuditCompensateValue),
    UnitStatus(UnitStatusValue),
    MlHealth(MlHealthValue),
    MlEmbedText(MlEmbedTextValue),
    MlChatCompletions(MlChatCompletionsValue),
    MlModels(MlModelsValue),
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostApiOperation {
    CtxCurrent,
    CtxResolveTarget,
    CtxParseDuration,
    CtxExpandReason,
    DbUserGet,
    DbUserPatch,
    DbUserIncr,
    DbKvGet,
    DbKvSet,
    MsgWindow,
    MsgByUser,
    JobScheduleAfter,
    AuditFind,
    AuditCompensate,
    UnitStatus,
    MlHealth,
    MlEmbedText,
    MlChatCompletions,
    MlModels,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct HostApiResponse<T> {
    pub operation: HostApiOperation,
    pub dry_run: bool,
    pub value: T,
}

impl<T> HostApiResponse<T> {
    pub(crate) fn map<U>(self, map: impl FnOnce(T) -> U) -> HostApiResponse<U> {
        HostApiResponse {
            operation: self.operation,
            dry_run: self.dry_run,
            value: map(self.value),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CtxCurrentValue {
    pub event: EventContext,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct CtxResolveTargetRequest {
    pub positional: Option<String>,
    pub selector_flag: Option<String>,
    pub implicit: Option<ParsedTargetSelector>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct CtxParseDurationRequest {
    pub input: String,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct CtxExpandReasonRequest {
    pub reason: ReasonExpr,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct DbUserGetRequest {
    pub user_id: i64,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct DbUserPatchRequest {
    pub patch: UserPatch,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct DbUserIncrRequest {
    pub user_id: i64,
    pub username: Option<String>,
    pub display_name: Option<String>,
    pub seen_at: String,
    pub updated_at: String,
    pub warn_count_delta: i64,
    pub reputation_delta: i64,
    pub shadowbanned: Option<bool>,
    pub state_json: Option<String>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct DbKvGetRequest {
    pub scope_kind: String,
    pub scope_id: String,
    pub key: String,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct DbKvSetRequest {
    pub entry: KvEntry,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct MsgWindowRequest {
    pub chat_id: i64,
    pub anchor_message_id: i64,
    pub up: usize,
    pub down: usize,
    pub include_anchor: bool,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct MsgByUserRequest {
    pub chat_id: i64,
    pub user_id: i64,
    pub since: String,
    pub limit: usize,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct JobScheduleAfterRequest {
    pub delay: String,
    pub executor_unit: String,
    pub payload: Value,
    pub dedupe_key: Option<String>,
    pub max_retries: Option<i64>,
    pub audit_action_id: Option<String>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct AuditFindRequest {
    pub filters: AuditLogFilter,
    pub limit: usize,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct AuditCompensateRequest {
    pub action_id: String,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct UnitStatusRequest {
    pub unit_id: Option<String>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct DbUserGetValue {
    pub user: Option<UserRecord>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct DbUserPatchValue {
    pub user: UserRecord,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct DbUserIncrValue {
    pub user: UserRecord,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct DbKvGetValue {
    pub entry: Option<KvEntry>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct DbKvSetValue {
    pub entry: KvEntry,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct MsgWindowValue {
    pub messages: Vec<MessageJournalRecord>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct MsgByUserValue {
    pub messages: Vec<MessageJournalRecord>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct JobScheduleAfterValue {
    pub job: JobRecord,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct AuditFindValue {
    pub entries: Vec<AuditLogEntry>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct AuditCompensateValue {
    pub compensated: bool,
    pub new_action_id: Option<String>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct UnitStatusValue {
    pub requested_unit_id: Option<String>,
    pub summary: UnitRegistryStatus,
    pub unit: Option<UnitStatusEntry>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct UnitStatusEntry {
    pub unit_id: String,
    pub status: UnitStatus,
    pub enabled: Option<bool>,
    pub diagnostics: Vec<UnitDiagnostic>,
}

impl UnitStatusEntry {
    pub(crate) fn from_descriptor(descriptor: &UnitDescriptor) -> Self {
        Self {
            unit_id: descriptor.id.clone(),
            status: descriptor.status,
            enabled: descriptor
                .manifest
                .as_ref()
                .map(|manifest| manifest.unit.enabled),
            diagnostics: descriptor.diagnostics.clone(),
        }
    }
}

#[derive(Debug, Clone, Error, Eq, PartialEq, Serialize, Deserialize)]
#[error("{kind:?} host api error in {operation:?}: {detail}")]
pub struct HostApiError {
    pub operation: HostApiOperation,
    pub kind: HostApiErrorKind,
    pub detail: HostApiErrorDetail,
}

impl HostApiError {
    pub(crate) fn validation(operation: HostApiOperation, detail: HostApiErrorDetail) -> Self {
        Self {
            operation,
            kind: HostApiErrorKind::Validation,
            detail,
        }
    }

    pub(crate) fn parse(operation: HostApiOperation, detail: HostApiErrorDetail) -> Self {
        Self {
            operation,
            kind: HostApiErrorKind::Parse,
            detail,
        }
    }

    pub(crate) fn denied(operation: HostApiOperation, detail: HostApiErrorDetail) -> Self {
        Self {
            operation,
            kind: HostApiErrorKind::Denied,
            detail,
        }
    }

    pub(crate) fn internal(operation: HostApiOperation, detail: HostApiErrorDetail) -> Self {
        Self {
            operation,
            kind: HostApiErrorKind::Internal,
            detail,
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostApiErrorKind {
    Validation,
    Parse,
    Denied,
    Internal,
}

#[derive(Debug, Clone, Error, Eq, PartialEq, Serialize, Deserialize)]
pub enum HostApiErrorDetail {
    #[error("invalid event context: {message}")]
    InvalidEventContext { message: String },
    #[error("invalid target `{value}`: {source}")]
    InvalidTarget {
        value: String,
        source: TargetParseError,
    },
    #[error("no target could be resolved from request or event context")]
    NoResolvableTarget,
    #[error("invalid duration `{value}`: {source}")]
    InvalidDuration {
        value: String,
        source: DurationParseError,
    },
    #[error("invalid field `{field}`: {message}")]
    InvalidField { field: String, message: String },
    #[error("invalid counter change for `{field}`: current={current}, delta={delta}")]
    InvalidCounterChange {
        field: String,
        current: i64,
        delta: i64,
    },
    #[error("message window too large: requested {requested}, max {max}")]
    MessageWindowTooLarge { requested: usize, max: usize },
    #[error("scheduled job delay `{delay}` exceeds max {max_days} days")]
    JobTooFarInFuture { delay: String, max_days: i64 },
    #[error("audit.find requires at least one filter")]
    MissingAuditFilter,
    #[error("unknown unit `{unit_id}`")]
    UnknownUnit { unit_id: String },
    #[error("operation denied for unit `{unit_id}`: missing capability `{capability}`")]
    CapabilityDenied { capability: String, unit_id: String },
    #[error("unknown audit action `{action_id}`")]
    UnknownAuditAction { action_id: String },
    #[error("required host resource `{resource}` is unavailable")]
    ResourceUnavailable { resource: String },
    #[error("storage failure: {message}")]
    StorageFailure { message: String },
    #[error("internal conversion failed: {message}")]
    InternalConversionFailure { message: String },
    #[error("reason expansion unexpectedly returned no result")]
    ReasonExpansionUnavailable,
}
