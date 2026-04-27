mod audit;
mod commands;
mod events;
mod helpers;
mod reactions;
mod types;
mod undo;

use std::rc::Rc;

pub use audit::*;
pub use commands::*;
pub use events::*;
pub use helpers::*;
pub use reactions::*;
pub use types::*;
pub use undo::*;

use crate::event::EventContext;
use crate::parser::dispatch::{CommandDispatchResult, EventCommandDispatcher};
use crate::parser::reason::ReasonAliasRegistry;
use crate::reputation::ReputationClient;
use crate::storage::{
    PROCESSED_UPDATE_STATUS_COMPLETED, PROCESSED_UPDATE_STATUS_PENDING, ProcessedUpdateRecord,
    StorageConnection,
};
use crate::tg::TelegramGateway;
use crate::unit::UnitRegistry;

#[derive(Debug, Clone)]
pub struct ModerationEngine {
    pub(crate) dry_run: bool,
    pub(crate) storage: Rc<StorageConnection>,
    pub(crate) unit_registry: Option<Rc<UnitRegistry>>,
    pub(crate) reputation: Option<Rc<ReputationClient>>,
    pub(crate) dispatcher: EventCommandDispatcher,
    pub(crate) gateway: TelegramGateway,
    pub(crate) admin_user_ids: Vec<i64>,
    pub(crate) processed_update_guard: bool,
}

impl ModerationEngine {
    pub fn new(storage: StorageConnection, gateway: TelegramGateway) -> Self {
        Self {
            dry_run: false,
            storage: Rc::new(storage),
            unit_registry: None,
            reputation: None,
            dispatcher: EventCommandDispatcher::new(),
            gateway,
            admin_user_ids: Vec::new(),
            processed_update_guard: true,
        }
    }

    pub fn with_reputation_client(mut self, client: Rc<ReputationClient>) -> Self {
        self.reputation = Some(client);
        self
    }

    pub fn with_storage_handle(mut self, storage: Rc<StorageConnection>) -> Self {
        self.storage = storage;
        self
    }

    pub fn with_unit_registry(mut self, registry: UnitRegistry) -> Self {
        self.unit_registry = Some(Rc::new(registry));
        self
    }

    pub fn with_unit_registry_handle(mut self, registry: Rc<UnitRegistry>) -> Self {
        self.unit_registry = Some(registry);
        self
    }

    pub fn with_reason_aliases(mut self, aliases: ReasonAliasRegistry) -> Self {
        self.dispatcher = EventCommandDispatcher::with_aliases(aliases);
        self
    }

    pub fn with_dry_run(mut self, dry_run: bool) -> Self {
        self.dry_run = dry_run;
        self
    }

    pub fn with_admin_user_ids<I>(mut self, admin_user_ids: I) -> Self
    where
        I: IntoIterator<Item = i64>,
    {
        self.admin_user_ids = admin_user_ids.into_iter().collect();
        self
    }

    pub fn without_processed_update_guard(mut self) -> Self {
        self.processed_update_guard = false;
        self
    }

    pub async fn handle_event(
        &self,
        event: &EventContext,
    ) -> Result<ModerationEventResult, ModerationError> {
        self.handle_event_with_unit_policy(event, None).await
    }

    pub async fn handle_event_with_unit_policy(
        &self,
        event: &EventContext,
        unit_policy: Option<&ModerationUnitPolicy>,
    ) -> Result<ModerationEventResult, ModerationError> {
        event
            .validate_invariants()
            .map_err(|source| ModerationError::InvalidEvent(source.to_string()))?;

        if let Some(record) = self.claim_processed_update(event)? {
            if record.status == PROCESSED_UPDATE_STATUS_COMPLETED {
                return Ok(ModerationEventResult::Replayed(record));
            }

            return Err(ModerationError::ProcessingInterrupted(record.event_id));
        }

        let result = match self.dispatcher.dispatch(event) {
            CommandDispatchResult::Skipped(skip) => ModerationEventResult::Skipped(skip),
            CommandDispatchResult::ParseError(error) => ModerationEventResult::ParseError(error),
            CommandDispatchResult::Parsed(dispatched) => {
                let execution = self
                    .execute_command_line(
                        event,
                        &dispatched.parsed,
                        &dispatched.expanded,
                        unit_policy,
                    )
                    .await?;
                ModerationEventResult::Executed(execution)
            }
        };

        self.mark_processed_update(event)?;

        Ok(result)
    }

    fn claim_processed_update(
        &self,
        event: &EventContext,
    ) -> Result<Option<ProcessedUpdateRecord>, ModerationError> {
        if self.dry_run || !self.processed_update_guard {
            return Ok(None);
        }

        let Some(update_id) = event.update_id else {
            return Ok(None);
        };

        let existing = self
            .storage
            .mark_processed_update(&ProcessedUpdateRecord {
                update_id: update_id as i64,
                event_id: event.event_id.clone(),
                processed_at: event.received_at.to_rfc3339(),
                execution_mode: execution_mode_name(event.execution_mode).to_owned(),
                status: PROCESSED_UPDATE_STATUS_PENDING.to_owned(),
            })
            .map_err(ModerationError::Storage)?;

        Ok(existing)
    }

    fn mark_processed_update(&self, event: &EventContext) -> Result<(), ModerationError> {
        if self.dry_run || !self.processed_update_guard {
            return Ok(());
        }

        let Some(update_id) = event.update_id else {
            return Ok(());
        };

        let _ = self
            .storage
            .complete_processed_update(update_id as i64, &event.received_at.to_rfc3339())
            .map_err(ModerationError::Storage)?;

        Ok(())
    }

    pub(crate) fn require_capability(
        &self,
        event: &EventContext,
        unit_policy: Option<&ModerationUnitPolicy>,
        capability: &'static str,
    ) -> Result<(), ModerationError> {
        let unit = unit_policy
            .map(|policy| &policy.unit)
            .or(event.system.unit.as_ref());

        println!("Checking capability {} for unit {:?}", capability, unit);

        let unit = unit.ok_or_else(|| ModerationError::CapabilityDenied {
            capability: capability.to_owned(),
            unit_id: "runtime".to_owned(),
        })?;
        let registry =
            self.unit_registry
                .as_deref()
                .ok_or_else(|| ModerationError::CapabilityDenied {
                    capability: capability.to_owned(),
                    unit_id: unit.id.clone(),
                })?;
        let descriptor = registry
            .get(&unit.id)
            .ok_or_else(|| ModerationError::UnknownUnit(unit.id.clone()))?;
        let manifest = descriptor
            .manifest
            .as_ref()
            .ok_or_else(|| ModerationError::UnknownUnit(unit.id.clone()))?;

        if manifest
            .capabilities
            .deny
            .iter()
            .any(|value| value == capability)
        {
            return Err(ModerationError::CapabilityDenied {
                capability: capability.to_owned(),
                unit_id: unit.id.clone(),
            });
        }
        if !manifest.capabilities.allow.is_empty()
            && !manifest
                .capabilities
                .allow
                .iter()
                .any(|value| value == capability)
        {
            return Err(ModerationError::CapabilityDenied {
                capability: capability.to_owned(),
                unit_id: unit.id.clone(),
            });
        }

        Ok(())
    }

    pub(crate) fn require_admin(&self, event: &EventContext) -> Result<(), ModerationError> {
        if event.is_synthetic() && event.sender.is_none() {
            return Ok(());
        }

        let Some(sender) = event.sender.as_ref() else {
            return Err(ModerationError::AuthorizationDenied { user_id: None });
        };

        if sender.is_admin || self.admin_user_ids.contains(&sender.id) {
            return Ok(());
        }

        Err(ModerationError::AuthorizationDenied {
            user_id: Some(sender.id),
        })
    }

    pub async fn register_member(
        &self,
        user_id: i64,
        username: Option<String>,
        display_name: Option<String>,
    ) -> Result<(), ModerationError> {
        let now = chrono::Utc::now().to_rfc3339();
        let patch = crate::storage::UserPatch {
            user_id,
            username,
            display_name,
            seen_at: now.clone(),
            warn_count: None,
            shadowbanned: None,
            reputation: None,
            state_json: None,
            updated_at: now,
        };
        self.storage
            .upsert_user(&patch)
            .map_err(ModerationError::Storage)?;
        Ok(())
    }

    pub(crate) fn require_bot_admin(&self, chat_id: i64) -> Result<(), ModerationError> {
        if !self.storage.get_bot_is_admin(chat_id)? {
            return Err(ModerationError::BotPermissionDenied);
        }
        Ok(())
    }

    pub async fn handle_error(
        &self,
        event: &EventContext,
        err: ModerationError,
    ) -> Result<(), crate::tg::TelegramError> {
        let (template_name, _context) = crate::moderation::reactions::error_to_template(&err);

        // Временная реализация: отправляем простой текст, так как полноценный UI шаблон требует HostApi
        let text = format!("Ошибка: {}", err);
        let request =
            crate::tg::TelegramRequest::SendMessage(crate::tg::TelegramSendMessageRequest {
                chat_id: event.chat.as_ref().map(|c| c.id).unwrap_or(0),
                text,
                reply_to_message_id: event.message.as_ref().map(|m| m.id),
                silent: false,
                parse_mode: crate::tg::ParseMode::PlainText,
                markup: None,
            });

        self.gateway.execute(request).await?;
        Ok(())
    }

    pub(crate) fn build_audit_entry(
        &self,
        event: &EventContext,
        unit_policy: Option<&ModerationUnitPolicy>,
        spec: AuditEntrySpec<'_>,
    ) -> crate::storage::AuditLogEntry {
        audit::build_audit_entry(event, unit_policy, spec)
    }
}

fn execution_mode_name(mode: crate::event::ExecutionMode) -> &'static str {
    match mode {
        crate::event::ExecutionMode::Realtime => "realtime",
        crate::event::ExecutionMode::Recovery => "recovery",
        crate::event::ExecutionMode::Scheduled => "scheduled",
        crate::event::ExecutionMode::Manual => "manual",
    }
}

#[cfg(test)]
mod tests;
