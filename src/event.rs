use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventContext {
    pub event_id: String,
    pub update_id: Option<u64>,
    pub update_type: UpdateType,
    pub received_at: DateTime<Utc>,
    pub execution_mode: ExecutionMode,
    pub recovery: bool,
    pub chat: Option<ChatContext>,
    pub sender: Option<SenderContext>,
    pub message: Option<MessageContext>,
    pub reply: Option<ReplyContext>,
    pub callback: Option<CallbackContext>,
    pub job: Option<JobContext>,
    pub system: SystemContext,
}

impl EventContext {
    pub fn system_event() -> Self {
        Self {
            event_id: "evt_bootstrap".to_owned(),
            update_id: None,
            update_type: UpdateType::System,
            received_at: Utc::now(),
            execution_mode: ExecutionMode::System,
            recovery: false,
            chat: None,
            sender: None,
            message: None,
            reply: None,
            callback: None,
            job: None,
            system: SystemContext::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateType {
    Message,
    EditedMessage,
    ChannelPost,
    EditedChannelPost,
    CallbackQuery,
    ChatMember,
    MyChatMember,
    JoinRequest,
    Job,
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionMode {
    Realtime,
    Recovery,
    Scheduled,
    Manual,
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatContext {
    pub id: i64,
    pub kind: String,
    pub title: Option<String>,
    pub username: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SenderContext {
    pub id: i64,
    pub username: Option<String>,
    pub display_name: Option<String>,
    pub is_bot: bool,
    pub is_admin: bool,
    pub role: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageContext {
    pub id: i32,
    pub date: DateTime<Utc>,
    pub text: Option<String>,
    pub entities: Vec<String>,
    pub has_media: bool,
    pub file_ids: Vec<String>,
    pub reply_to_message_id: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplyContext {
    pub message_id: i32,
    pub sender_user_id: Option<i64>,
    pub sender_username: Option<String>,
    pub text: Option<String>,
    pub has_media: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallbackContext {
    pub query_id: String,
    pub data: Option<String>,
    pub message_id: Option<i32>,
    pub origin_chat_id: Option<i64>,
    pub from_user_id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobContext {
    pub job_id: String,
    pub payload: serde_json::Value,
    pub scheduled_at: DateTime<Utc>,
    pub run_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemContext {
    pub locale: Option<String>,
    pub unit: Option<String>,
    pub trace_id: Option<String>,
    pub build: Option<String>,
}

impl Default for SystemContext {
    fn default() -> Self {
        Self {
            locale: None,
            unit: None,
            trace_id: None,
            build: None,
        }
    }
}
