use serde::{Deserialize, Serialize};

pub const PROCESSED_UPDATE_STATUS_PENDING: &str = "pending";
pub const PROCESSED_UPDATE_STATUS_COMPLETED: &str = "completed";
pub const EXTERNAL_EFFECT_STATUS_IN_PROGRESS: &str = "in_progress";
pub const EXTERNAL_EFFECT_STATUS_COMPLETED: &str = "completed";
pub const EXTERNAL_EFFECT_STATUS_ERROR: &str = "error";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserRecord {
    pub user_id: i64,
    pub username: Option<String>,
    pub display_name: Option<String>,
    pub first_seen_at: String,
    pub last_seen_at: String,
    pub warn_count: i64,
    pub shadowbanned: bool,
    pub reputation: i64,
    pub state_json: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserPatch {
    pub user_id: i64,
    pub username: Option<String>,
    pub display_name: Option<String>,
    pub seen_at: String,
    pub warn_count: Option<i64>,
    pub shadowbanned: Option<bool>,
    pub reputation: Option<i64>,
    pub state_json: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KvEntry {
    pub scope_kind: String,
    pub scope_id: String,
    pub key: String,
    pub value_json: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessedUpdateRecord {
    pub update_id: i64,
    pub event_id: String,
    pub processed_at: String,
    pub execution_mode: String,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageJournalRecord {
    pub chat_id: i64,
    pub message_id: i64,
    pub user_id: Option<i64>,
    pub date_utc: String,
    pub update_type: String,
    pub text: Option<String>,
    pub normalized_text: Option<String>,
    pub has_media: bool,
    pub reply_to_message_id: Option<i64>,
    pub file_ids_json: Option<String>,
    pub meta_json: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct AuditLogFilter {
    pub action_id: Option<String>,
    pub trace_id: Option<String>,
    pub request_id: Option<String>,
    pub idempotency_key: Option<String>,
    pub trigger_message_id: Option<i64>,
    pub actor_user_id: Option<i64>,
    pub chat_id: Option<i64>,
    pub op: Option<String>,
    pub target_id: Option<String>,
    pub reversible: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobRecord {
    pub job_id: String,
    pub executor_unit: String,
    pub run_at: String,
    pub scheduled_at: String,
    pub status: String,
    pub dedupe_key: Option<String>,
    pub payload_json: String,
    pub retry_count: i64,
    pub max_retries: i64,
    pub last_error_code: Option<String>,
    pub last_error_text: Option<String>,
    pub audit_action_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditLogEntry {
    pub action_id: String,
    pub trace_id: Option<String>,
    pub request_id: Option<String>,
    pub unit_name: String,
    pub execution_mode: String,
    pub op: String,
    pub actor_user_id: Option<i64>,
    pub chat_id: Option<i64>,
    pub target_kind: Option<String>,
    pub target_id: Option<String>,
    pub trigger_message_id: Option<i64>,
    pub idempotency_key: Option<String>,
    pub reversible: bool,
    pub compensation_json: Option<String>,
    pub args_json: String,
    pub result_json: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalEffectRecord {
    pub idempotency_key: String,
    pub operation: String,
    pub request_json: String,
    pub result_json: Option<String>,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
    pub error_json: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExternalEffectReservation {
    Inserted(ExternalEffectRecord),
    Existing(ExternalEffectRecord),
}
