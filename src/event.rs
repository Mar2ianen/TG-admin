#![allow(dead_code)]

use anyhow::{Result, bail};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

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
        Self::new(
            "evt_bootstrap",
            UpdateType::System,
            ExecutionMode::Manual,
            SystemContext::runtime_bootstrap(),
        )
    }

    pub fn synthetic_for_unit(
        event_id: impl Into<String>,
        execution_mode: ExecutionMode,
        unit_id: impl Into<String>,
    ) -> Self {
        let unit = UnitContext::new(unit_id);
        let system = SystemContext::synthetic(SystemOrigin::Manual).with_unit(unit);

        Self::new(event_id, UpdateType::System, execution_mode, system)
    }

    pub fn new(
        event_id: impl Into<String>,
        update_type: UpdateType,
        execution_mode: ExecutionMode,
        system: SystemContext,
    ) -> Self {
        let recovery = matches!(execution_mode, ExecutionMode::Recovery);

        Self {
            event_id: event_id.into(),
            update_id: None,
            update_type,
            received_at: Utc::now(),
            execution_mode,
            recovery,
            chat: None,
            sender: None,
            message: None,
            reply: None,
            callback: None,
            job: None,
            system,
        }
    }

    pub fn bind_unit(mut self, unit: UnitContext) -> Self {
        self.system = self.system.with_unit(unit);
        self
    }

    pub fn is_synthetic(&self) -> bool {
        self.system.synthetic
    }

    pub fn validate_invariants(&self) -> Result<()> {
        if self.event_id.trim().is_empty() {
            bail!("event_id must not be empty");
        }

        if self.recovery != matches!(self.execution_mode, ExecutionMode::Recovery) {
            bail!("recovery flag must match execution_mode");
        }

        if matches!(self.update_type, UpdateType::CallbackQuery) != self.callback.is_some() {
            bail!("callback context must exist only for callback_query events");
        }

        if matches!(self.update_type, UpdateType::Job) != self.job.is_some() {
            bail!("job context must exist only for job events");
        }

        if self.system.requires_unit_context() && self.system.unit.is_none() {
            bail!("system.unit is required for unit-scoped execution");
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionMode {
    Realtime,
    Recovery,
    Scheduled,
    Manual,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatContext {
    pub id: i64,
    #[serde(rename = "type")]
    pub chat_type: String,
    pub title: Option<String>,
    pub username: Option<String>,
    pub thread_id: Option<i64>,
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
    pub media_group_id: Option<String>,
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
    pub payload: Value,
    pub scheduled_at: DateTime<Utc>,
    pub run_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemContext {
    pub locale: Option<String>,
    pub unit: Option<UnitContext>,
    pub trace_id: Option<String>,
    pub build: Option<String>,
    pub origin: SystemOrigin,
    pub synthetic: bool,
}

impl SystemContext {
    pub fn runtime_bootstrap() -> Self {
        Self {
            locale: None,
            unit: None,
            trace_id: None,
            build: None,
            origin: SystemOrigin::Runtime,
            synthetic: true,
        }
    }

    pub fn synthetic(origin: SystemOrigin) -> Self {
        Self {
            locale: None,
            unit: None,
            trace_id: None,
            build: None,
            origin,
            synthetic: true,
        }
    }

    pub fn realtime() -> Self {
        Self {
            locale: None,
            unit: None,
            trace_id: None,
            build: None,
            origin: SystemOrigin::Telegram,
            synthetic: false,
        }
    }

    pub fn with_unit(mut self, unit: UnitContext) -> Self {
        self.unit = Some(unit);
        self
    }

    fn requires_unit_context(&self) -> bool {
        matches!(
            self.origin,
            SystemOrigin::Manual | SystemOrigin::Scheduler | SystemOrigin::RecoveryReplay
        )
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SystemOrigin {
    Telegram,
    Scheduler,
    Manual,
    RecoveryReplay,
    Runtime,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnitContext {
    pub id: String,
    pub version: Option<String>,
    pub trigger: Option<String>,
}

impl UnitContext {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            version: None,
            trigger: None,
        }
    }

    pub fn with_trigger(mut self, trigger: impl Into<String>) -> Self {
        self.trigger = Some(trigger.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::{
        EventContext, ExecutionMode, SystemContext, SystemOrigin, UnitContext, UpdateType,
    };

    #[test]
    fn system_event_uses_synthetic_runtime_context() {
        let event = EventContext::system_event();

        assert_eq!(event.update_type, UpdateType::System);
        assert_eq!(event.execution_mode, ExecutionMode::Manual);
        assert!(event.is_synthetic());
        assert_eq!(event.system.origin, SystemOrigin::Runtime);
        assert!(event.system.unit.is_none());
        event.validate_invariants().expect("valid system event");
    }

    #[test]
    fn synthetic_unit_context_satisfies_manual_invariants() {
        let event = EventContext::synthetic_for_unit(
            "evt_manual_warn",
            ExecutionMode::Manual,
            "moderation.warn",
        );

        assert_eq!(event.system.origin, SystemOrigin::Manual);
        assert_eq!(
            event.system.unit.as_ref().map(|unit| unit.id.as_str()),
            Some("moderation.warn")
        );
        event.validate_invariants().expect("valid unit event");
    }

    #[test]
    fn manual_origin_requires_unit_context() {
        let event = EventContext::new(
            "evt_invalid",
            UpdateType::System,
            ExecutionMode::Manual,
            SystemContext::synthetic(SystemOrigin::Manual),
        );

        let err = event
            .validate_invariants()
            .expect_err("missing unit must fail");
        assert!(err.to_string().contains("system.unit"));
    }

    #[test]
    fn recovery_flag_must_match_execution_mode() {
        let mut event = EventContext::new(
            "evt_replay",
            UpdateType::Message,
            ExecutionMode::Recovery,
            SystemContext::synthetic(SystemOrigin::RecoveryReplay)
                .with_unit(UnitContext::new("moderation.mute")),
        );
        event.recovery = false;

        let err = event
            .validate_invariants()
            .expect_err("recovery mismatch must fail");
        assert!(err.to_string().contains("recovery flag"));
    }
}
