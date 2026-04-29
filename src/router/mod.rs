mod classify;
mod dispatch;
#[cfg(test)]
mod tests;
mod types;

pub use classify::*;
pub use dispatch::*;
pub use types::*;

use crate::event::{EventContext, UnitContext};
use crate::moderation::{ModerationEngine, ModerationUnitPolicy};
use crate::unit::UnitRegistry;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

#[derive(Debug, Clone)]
pub struct ExecutionRouter {
    index: RefCell<RouterIndex>,
    registry: RefCell<Option<Rc<UnitRegistry>>>,
    moderation: Option<ModerationEngine>,
    script_executor: Option<ScriptExecutor>,
    gateway: Option<std::sync::Arc<crate::tg::TelegramGateway>>,
    delete_unknown_commands: bool,
}

impl ExecutionRouter {
    pub fn new(_bot_id: i64, delete_unknown_commands: bool) -> Self {
        Self {
            index: RefCell::new(RouterIndex::new()),
            registry: RefCell::new(None),
            moderation: None,
            script_executor: None,
            gateway: None,
            delete_unknown_commands,
        }
    }

    pub fn with_gateway(mut self, gateway: std::sync::Arc<crate::tg::TelegramGateway>) -> Self {
        self.gateway = Some(gateway);
        self
    }

    pub fn with_storage(self, _storage: crate::storage::Storage) -> Self {
        self
    }

    pub fn with_moderation(mut self, moderation: ModerationEngine) -> Self {
        self.moderation = Some(moderation);
        self
    }

    pub fn with_registry(self, registry: UnitRegistry) -> Self {
        *self.index.borrow_mut() = RouterIndex::from_registry(&registry);
        *self.registry.borrow_mut() = Some(Rc::new(registry));
        self
    }

    pub fn with_registry_handle(self, registry: Rc<UnitRegistry>) -> Self {
        *self.index.borrow_mut() = RouterIndex::from_registry(&registry);
        *self.registry.borrow_mut() = Some(registry);
        self
    }

    pub fn with_script_runner(
        mut self,
        runner: crate::script::ScriptRunner,
        host_api: crate::host_api::HostApi,
    ) -> Self {
        self.script_executor = Some(ScriptExecutor { runner, host_api });
        self
    }

    pub fn index_stats(&self) -> RouterIndexStats {
        self.index.borrow().stats()
    }

    pub fn sync_registry(&self, registry: UnitRegistry) {
        *self.index.borrow_mut() = RouterIndex::from_registry(&registry);
        *self.registry.borrow_mut() = Some(Rc::new(registry));
    }

    pub fn plan(&self, event: &EventContext) -> RoutePlan {
        let classified = classify_event(event);
        self.index.borrow().plan(classified)
    }

    pub async fn handle_moderation_error(
        &self,
        event: &EventContext,
        err: crate::moderation::ModerationError,
    ) -> Result<(), crate::tg::TelegramError> {
        if let Some(moderation) = self.moderation.as_ref() {
            moderation.handle_error(event, err).await?;
        }
        Ok(())
    }

    pub async fn route(&self, event: &EventContext) -> Result<ExecutionOutcome, RoutingError> {
        // Инициализация прав при входе бота
        if event.update_type == crate::event::UpdateType::MyChatMember {
            // ... (предыдущий код)
        }

        // Реакции на пользователей
        if (event.update_type == crate::event::UpdateType::ChatMember
            || event.update_type == crate::event::UpdateType::MyChatMember
            || event.update_type == crate::event::UpdateType::ChatMemberUpdated)
            && let Some(moderation) = self.moderation.as_ref()
            && let Some(member) = event.chat_member.as_ref()
        {
            if member.is_joined() {
                let _ = moderation.on_member_joined(event).await;
            } else if member.is_left() {
                let _ = moderation.on_member_left(event).await;
            }
        }

        let plan = self.plan(event);

        // Перехват неизвестных команд
        if let Some(cmd_name) = plan.classified.command_name.as_ref()
            && !self.index.borrow().is_known_command(cmd_name)
            && let Some(moderation) = self.moderation.as_ref()
        {
            let _ = moderation
                .handle_error(
                    event,
                    crate::moderation::ModerationError::UnsupportedCommand(cmd_name.clone()),
                )
                .await;

            if self.delete_unknown_commands
                && let (Some(msg), Some(gateway)) = (event.message.as_ref(), self.gateway.as_ref())
            {
                let chat_id = event.chat.as_ref().map(|c| c.id).unwrap_or(0);
                let del_req =
                    crate::tg::TelegramRequest::Delete(crate::tg::TelegramDeleteRequest {
                        chat_id,
                        message_id: msg.id,
                        idempotency_key: None,
                    });
                let gateway = gateway.clone();
                tokio::spawn(async move {
                    let _ = gateway.execute(del_req).await;
                });
            }

            return Ok(ExecutionOutcome::Unhandled(plan));
        }

        let registry = self
            .registry
            .borrow()
            .clone()
            .or_else(|| {
                self.moderation
                    .as_ref()
                    .and_then(|m| m.unit_registry.clone())
            })
            .unwrap_or_else(|| Rc::new(UnitRegistry::new()));

        let invocations = select_unit_dispatches(&registry, event);

        if plan.lanes.contains(&ExecutionLane::BuiltInModeration)
            && let Some(moderation) = &self.moderation
        {
            let unit_policy = unit_policy_for_builtin_moderation(&invocations);

            let result = moderation
                .handle_event_with_unit_policy(event, unit_policy.as_ref())
                .await
                .map_err(RoutingError::Moderation)?;

            return Ok(ExecutionOutcome::BuiltInModeration {
                plan,
                result,
                deferred_invocations: invocations,
            });
        }

        if plan.lanes.contains(&ExecutionLane::UnitDispatch) || !invocations.is_empty() {
            if let Some(script_executor) = self.script_executor.as_ref() {
                for invocation in &invocations {
                    script_executor.runner.execute(
                        &invocation.exec_start,
                        invocation.entry_point.as_deref(),
                        event,
                        &script_executor.host_api,
                    )?;
                }
            }

            return Ok(ExecutionOutcome::UnitDispatch { plan, invocations });
        }

        Ok(ExecutionOutcome::Unhandled(plan))
    }
}

#[derive(Debug, Clone)]
struct ScriptExecutor {
    runner: crate::script::ScriptRunner,
    host_api: crate::host_api::HostApi,
}

#[derive(Debug, Clone)]
pub struct RouterIndex {
    command_index: HashMap<String, Vec<ExecutionLane>>,
    event_type_index: HashMap<crate::unit::UnitEventType, Vec<ExecutionLane>>,
    trait_route_count: usize,
}

impl RouterIndex {
    pub fn new() -> Self {
        let mut command_index = HashMap::new();
        for cmd in ["warn", "mute", "ban", "del", "undo", "msg", "help", "ping"] {
            command_index.insert(cmd.to_owned(), vec![ExecutionLane::BuiltInModeration]);
        }
        Self {
            command_index,
            event_type_index: HashMap::new(),
            trait_route_count: 0,
        }
    }

    pub fn from_registry(registry: &UnitRegistry) -> Self {
        let mut index = Self::new();
        for entry in registry.entries() {
            if let Some(manifest) = &entry.manifest {
                match &manifest.trigger {
                    crate::unit::TriggerSpec::Command { commands } => {
                        for cmd in commands {
                            index
                                .command_index
                                .entry(cmd.to_lowercase())
                                .or_default()
                                .push(ExecutionLane::UnitDispatch);
                        }
                    }
                    crate::unit::TriggerSpec::EventType { events } => {
                        for event_type in events {
                            index
                                .event_type_index
                                .entry(*event_type)
                                .or_default()
                                .push(ExecutionLane::UnitDispatch);
                        }
                        index.trait_route_count += 1;
                    }
                    crate::unit::TriggerSpec::Regex { .. } => {
                        // Regex triggers are not yet supported in indexed routing.
                        index.trait_route_count += 1;
                    }
                }
            }
        }
        index
    }

    pub fn is_known_command(&self, command: &str) -> bool {
        self.command_index.contains_key(&command.to_lowercase())
    }

    pub fn stats(&self) -> RouterIndexStats {
        RouterIndexStats {
            command_routes: self.command_index.len(),
            ingress_routes: self.event_type_index.len(),
            trait_routes: self.trait_route_count,
            ..RouterIndexStats::default()
        }
    }

    pub fn plan(&self, classified: ClassifiedEvent) -> RoutePlan {
        let mut lanes = Vec::new();
        let mut matched_buckets = vec![
            RouteBucket::IngressClass(classified.ingress_class),
            RouteBucket::ChatScope(classified.chat_scope),
            RouteBucket::AuthorKind(classified.author_kind),
        ];
        matched_buckets.extend(
            classified
                .traits
                .iter()
                .copied()
                .map(RouteBucket::EventTrait),
        );
        if let Some(command_name) = &classified.command_name {
            matched_buckets.push(RouteBucket::CommandIndex(command_name.clone()));
        }
        if let Some(cmd) = &classified.command_name
            && let Some(mapped_lanes) = self.command_index.get(cmd)
        {
            lanes.extend(mapped_lanes);
        }

        // Check for event type dispatch if unit exists
        if let Some(event_type) = map_update_to_unit_event(classified.update_type)
            && let Some(mapped_lanes) = self.event_type_index.get(&event_type)
        {
            lanes.extend(mapped_lanes);
        }

        RoutePlan {
            classified,
            matched_buckets,
            lanes,
        }
    }
}

impl Default for RouterIndex {
    fn default() -> Self {
        Self::new()
    }
}

fn map_update_to_unit_event(
    update_type: crate::event::UpdateType,
) -> Option<crate::unit::UnitEventType> {
    match update_type {
        crate::event::UpdateType::Message => Some(crate::unit::UnitEventType::Message),
        crate::event::UpdateType::CallbackQuery => Some(crate::unit::UnitEventType::CallbackQuery),
        crate::event::UpdateType::ChatMember => Some(crate::unit::UnitEventType::MemberJoined),
        crate::event::UpdateType::Job => Some(crate::unit::UnitEventType::Job),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub struct RouterIndexStats {
    pub ingress_routes: usize,
    pub chat_scope_routes: usize,
    pub author_routes: usize,
    pub trait_routes: usize,
    pub command_routes: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum RoutingError {
    #[error("moderation error: {0}")]
    Moderation(#[from] crate::moderation::ModerationError),
    #[error("script error: {0}")]
    Script(#[from] crate::script::ScriptError),
    #[error("missing lane executor")]
    MissingLaneExecutor { lane: ExecutionLane },
}

pub fn unit_policy_for_builtin_moderation(
    invocations: &[UnitDispatchInvocation],
) -> Option<ModerationUnitPolicy> {
    invocations.first().map(|invocation| {
        ModerationUnitPolicy::new(
            UnitContext::new(invocation.unit_id.clone())
                .with_trigger(unit_trigger_name(&invocation.trigger)),
        )
    })
}

fn unit_trigger_name(trigger: &UnitDispatchTrigger) -> &'static str {
    match trigger {
        UnitDispatchTrigger::Command { .. }
        | UnitDispatchTrigger::Regex { .. }
        | UnitDispatchTrigger::EventType { .. } => "telegram",
    }
}
