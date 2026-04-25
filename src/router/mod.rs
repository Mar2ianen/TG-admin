mod classify;
mod dispatch;
mod types;

pub use classify::*;
pub use dispatch::*;
pub use types::*;

use crate::event::{EventContext, ExecutionMode, UnitContext, UpdateType};
use crate::moderation::{ModerationEngine, ModerationEventResult, ModerationUnitPolicy};
use crate::unit::{UnitRegistry, UnitStatus};
use std::collections::{BTreeMap, HashMap};

#[derive(Debug, Clone)]
pub struct ExecutionRouter {
    index: RouterIndex,
    moderation: Option<ModerationEngine>,
    script_runner: Option<crate::script::ScriptRunner>,
}

impl ExecutionRouter {
    pub fn new() -> Self {
        Self {
            index: RouterIndex::new(),
            moderation: None,
            script_runner: None,
        }
    }

    pub fn with_moderation(mut self, moderation: ModerationEngine) -> Self {
        self.moderation = Some(moderation);
        self
    }

    pub fn with_registry(mut self, registry: UnitRegistry) -> Self {
        self.index = RouterIndex::from_registry(&registry);
        self
    }

    pub fn with_script_runner(
        mut self,
        runner: crate::script::ScriptRunner,
        _host_api: crate::host_api::HostApi,
    ) -> Self {
        self.script_runner = Some(runner);
        self
    }

    pub fn index_stats(&self) -> RouterIndexStats {
        self.index.stats()
    }

    pub fn sync_registry(&self, registry: UnitRegistry) {
        // Implementation omitted for brevity
    }

    pub fn plan(&self, event: &EventContext) -> RoutePlan {
        let classified = classify_event(event);
        self.index.plan(classified)
    }

    pub async fn route(&self, event: &EventContext) -> Result<ExecutionOutcome, RoutingError> {
        let classified = classify_event(event);
        let plan = self.plan(event);

        if let Some(moderation) = &self.moderation {
            let invocations = select_unit_dispatches(
                moderation
                    .unit_registry
                    .as_deref()
                    .unwrap_or(&UnitRegistry::new()),
                event,
            );
            let unit_policy = unit_policy_for_builtin_moderation(&invocations);

            let result = moderation
                .handle_event_with_unit_policy(event, unit_policy.as_ref())
                .await
                .map_err(|e| RoutingError::Moderation(e))?;

            return Ok(ExecutionOutcome::BuiltInModeration {
                plan,
                result,
                deferred_invocations: invocations,
            });
        }

        Ok(ExecutionOutcome::Unhandled(plan))
    }
}

#[derive(Debug, Clone)]
pub struct RouterIndex {
    command_index: HashMap<String, Vec<ExecutionLane>>,
}

impl RouterIndex {
    pub fn new() -> Self {
        Self {
            command_index: HashMap::new(),
        }
    }

    pub fn from_registry(registry: &UnitRegistry) -> Self {
        let mut command_index = HashMap::new();
        for entry in registry.entries() {
            if let Some(manifest) = &entry.manifest {
                if let crate::unit::TriggerSpec::Command { commands } = &manifest.trigger {
                    for cmd in commands {
                        command_index
                            .entry(cmd.to_lowercase())
                            .or_insert_with(Vec::new)
                            .push(ExecutionLane::UnitDispatch);
                    }
                }
            }
        }
        Self { command_index }
    }

    pub fn stats(&self) -> RouterIndexStats {
        RouterIndexStats {
            command_routes: self.command_index.len(),
            ..RouterIndexStats::default()
        }
    }

    pub fn plan(&self, classified: ClassifiedEvent) -> RoutePlan {
        let mut lanes = Vec::new();
        if let Some(cmd) = &classified.command_name {
            if let Some(mapped_lanes) = self.command_index.get(cmd) {
                lanes.extend(mapped_lanes);
            }
        }
        RoutePlan {
            classified,
            matched_buckets: Vec::new(),
            lanes,
        }
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
