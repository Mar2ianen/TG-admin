use super::ml::{
    MlChatCompletionsRequest, MlChatCompletionsValue, MlEmbedTextRequest, MlEmbedTextValue,
    MlHealthRequest, MlHealthValue, MlModelsRequest, MlModelsValue,
};
use crate::event::EventContext;
use crate::parser::command::ReasonExpr;
use crate::parser::duration::ParsedDuration;
use crate::parser::reason::ExpandedReason;
use crate::parser::target::{ParsedTargetSelector, ResolvedTarget};
use crate::storage::{
    AuditLogEntry, AuditLogFilter, JobRecord, KvEntry, MessageJournalRecord, UserPatch, UserRecord,
};
use crate::unit::{UnitDescriptor, UnitDiagnostic, UnitRegistryStatus, UnitStatus};
use serde::{Deserialize, Serialize};
use serde_json::Value;

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
    MlTranscribe(MlTranscribeRequest),
    TgSendMessage(TgSendMessageRequest),
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
    MlTranscribe(MlTranscribeValue),
    TgSendMessage(TgSendMessageValue),
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
    MlTranscribe,
    TgSendMessage,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct HostApiResponse<T> {
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
pub(crate) struct CtxCurrentValue {
    pub event: EventContext,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct CtxResolveTargetRequest {
    pub positional: Option<String>,
    pub selector_flag: Option<String>,
    pub implicit: Option<ParsedTargetSelector>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct CtxParseDurationRequest {
    pub input: String,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct CtxExpandReasonRequest {
    pub reason: ReasonExpr,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct DbUserGetRequest {
    pub user_id: i64,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct DbUserPatchRequest {
    pub patch: UserPatch,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct DbUserIncrRequest {
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
pub(crate) struct DbKvGetRequest {
    pub scope_kind: String,
    pub scope_id: String,
    pub key: String,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct DbKvSetRequest {
    pub entry: KvEntry,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct MsgWindowRequest {
    pub chat_id: i64,
    pub anchor_message_id: i64,
    pub up: usize,
    pub down: usize,
    pub include_anchor: bool,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct MsgByUserRequest {
    pub chat_id: i64,
    pub user_id: i64,
    pub since: String,
    pub limit: usize,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct JobScheduleAfterRequest {
    pub delay: String,
    pub executor_unit: String,
    pub payload: Value,
    pub dedupe_key: Option<String>,
    pub max_retries: Option<i64>,
    pub audit_action_id: Option<String>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct AuditFindRequest {
    pub filters: AuditLogFilter,
    pub limit: usize,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct AuditCompensateRequest {
    pub action_id: String,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct UnitStatusRequest {
    pub unit_id: Option<String>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct DbUserGetValue {
    pub user: Option<UserRecord>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct DbUserPatchValue {
    pub user: UserRecord,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct DbUserIncrValue {
    pub user: UserRecord,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct DbKvGetValue {
    pub entry: Option<KvEntry>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct DbKvSetValue {
    pub entry: KvEntry,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct MsgWindowValue {
    pub messages: Vec<MessageJournalRecord>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct MsgByUserValue {
    pub messages: Vec<MessageJournalRecord>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct JobScheduleAfterValue {
    pub job: JobRecord,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct AuditFindValue {
    pub entries: Vec<AuditLogEntry>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct AuditCompensateValue {
    pub compensated: bool,
    pub new_action_id: Option<String>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct UnitStatusValue {
    pub requested_unit_id: Option<String>,
    pub summary: UnitRegistryStatus,
    pub unit: Option<UnitStatusEntry>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct UnitStatusEntry {
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

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct MlTranscribeRequest {
    pub base_url: Option<String>,
    pub file_id: String,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct MlTranscribeValue {
    pub base_url: Option<String>,
    pub file_id: String,
    pub text: Option<String>,
    pub transport_ready: bool,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct TgSendMessageRequest {
    pub chat_id: i64,
    pub text: String,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct TgSendMessageValue {
    pub message_id: i32,
}
