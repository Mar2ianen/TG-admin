#![allow(dead_code)]

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnitManifest {
    #[serde(rename = "Unit")]
    pub unit: UnitDefinition,
    #[serde(rename = "Trigger")]
    pub trigger: TriggerSpec,
    #[serde(rename = "Service")]
    pub service: ServiceSpec,
    #[serde(rename = "Capabilities", default)]
    pub capabilities: CapabilitiesSpec,
    #[serde(rename = "Runtime", default)]
    pub runtime: RuntimeSpec,
}

impl UnitManifest {
    pub fn new(unit: UnitDefinition, trigger: TriggerSpec, service: ServiceSpec) -> Self {
        Self {
            unit,
            trigger,
            service,
            capabilities: CapabilitiesSpec::default(),
            runtime: RuntimeSpec::default(),
        }
    }

    pub fn name(&self) -> &str {
        &self.unit.name
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct UnitDefinition {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub after: Vec<String>,
    #[serde(default)]
    pub requires: Vec<String>,
    #[serde(default)]
    pub wants: Vec<String>,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub owner: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
}

impl UnitDefinition {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: None,
            after: Vec::new(),
            requires: Vec::new(),
            wants: Vec::new(),
            enabled: default_enabled(),
            tags: Vec::new(),
            owner: None,
            version: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "Type", rename_all = "snake_case")]
pub enum TriggerSpec {
    Command {
        #[serde(rename = "Commands")]
        commands: Vec<String>,
    },
    Regex {
        #[serde(rename = "Pattern")]
        pattern: String,
    },
    EventType {
        #[serde(rename = "Events")]
        events: Vec<UnitEventType>,
    },
}

impl TriggerSpec {
    pub fn command(commands: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self::Command {
            commands: commands.into_iter().map(Into::into).collect(),
        }
    }

    pub fn regex(pattern: impl Into<String>) -> Self {
        Self::Regex {
            pattern: pattern.into(),
        }
    }

    pub fn event_type(events: impl IntoIterator<Item = UnitEventType>) -> Self {
        Self::EventType {
            events: events.into_iter().collect(),
        }
    }

    pub fn trigger_type(&self) -> TriggerType {
        match self {
            Self::Command { .. } => TriggerType::Command,
            Self::Regex { .. } => TriggerType::Regex,
            Self::EventType { .. } => TriggerType::EventType,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerType {
    Command,
    Regex,
    EventType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnitEventType {
    Message,
    CallbackQuery,
    Job,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ServiceSpec {
    pub exec_start: String,
    #[serde(default)]
    pub entry_point: Option<String>,
    #[serde(default = "default_timeout_sec")]
    pub timeout_sec: u64,
    #[serde(default)]
    pub restart: RestartPolicy,
    #[serde(default = "default_restart_sec")]
    pub restart_sec: u64,
    #[serde(default)]
    pub max_retries: u32,
    #[serde(default)]
    pub on_failure: Option<String>,
}

impl ServiceSpec {
    pub fn new(exec_start: impl Into<String>) -> Self {
        Self {
            exec_start: exec_start.into(),
            entry_point: None,
            timeout_sec: default_timeout_sec(),
            restart: RestartPolicy::default(),
            restart_sec: default_restart_sec(),
            max_retries: 0,
            on_failure: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum RestartPolicy {
    #[default]
    #[serde(rename = "no")]
    No,
    #[serde(rename = "on-failure")]
    OnFailure,
    #[serde(rename = "always")]
    Always,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct CapabilitiesSpec {
    #[serde(default)]
    pub allow: Vec<String>,
    #[serde(default)]
    pub deny: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct RuntimeSpec {
    #[serde(default)]
    pub max_memory_kb: Option<u64>,
    #[serde(default)]
    pub max_output_bytes: Option<u64>,
    #[serde(default)]
    pub dry_run_supported: bool,
    #[serde(default)]
    pub idempotent_by_default: bool,
    #[serde(default)]
    pub allow_in_recovery: bool,
    #[serde(default)]
    pub allow_manual_invoke: bool,
}

#[derive(Debug, Clone, Default)]
pub struct UnitRegistry {
    entries: Vec<UnitDescriptor>,
}

impl UnitRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_entries(entries: Vec<UnitDescriptor>) -> Self {
        Self { entries }
    }

    pub fn status_summary(&self) -> UnitRegistryStatus {
        let mut summary = UnitRegistryStatus {
            total_units: self.entries.len(),
            ..UnitRegistryStatus::default()
        };

        for entry in &self.entries {
            match entry.status {
                UnitStatus::Loaded => summary.loaded_units += 1,
                UnitStatus::Active => summary.active_units += 1,
                UnitStatus::Running => summary.running_units += 1,
                UnitStatus::RetryWait => summary.retry_wait_units += 1,
                UnitStatus::Failed => summary.failed_units += 1,
                UnitStatus::Dead => summary.dead_units += 1,
                UnitStatus::Disabled => summary.disabled_units += 1,
            }
        }

        summary
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitDescriptor {
    pub id: String,
    pub manifest: UnitManifest,
    pub status: UnitStatus,
}

impl UnitDescriptor {
    pub fn new(manifest: UnitManifest, status: UnitStatus) -> Self {
        Self {
            id: manifest.name().to_owned(),
            manifest,
            status,
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum UnitStatus {
    Loaded,
    Active,
    Running,
    RetryWait,
    Failed,
    Dead,
    Disabled,
}

#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub struct UnitRegistryStatus {
    pub total_units: usize,
    pub loaded_units: usize,
    pub active_units: usize,
    pub running_units: usize,
    pub retry_wait_units: usize,
    pub failed_units: usize,
    pub dead_units: usize,
    pub disabled_units: usize,
}

const fn default_enabled() -> bool {
    true
}

const fn default_timeout_sec() -> u64 {
    3
}

const fn default_restart_sec() -> u64 {
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_definition_defaults_to_enabled_with_empty_collections() {
        let definition = UnitDefinition::new("crypto_fetcher.unit");

        assert_eq!(definition.name, "crypto_fetcher.unit");
        assert!(definition.enabled);
        assert!(definition.after.is_empty());
        assert!(definition.requires.is_empty());
        assert!(definition.wants.is_empty());
        assert!(definition.tags.is_empty());
        assert!(definition.description.is_none());
        assert!(definition.owner.is_none());
        assert!(definition.version.is_none());
    }

    #[test]
    fn service_spec_uses_documented_safe_defaults() {
        let service = ServiceSpec::new("scripts/crypto.rhai");

        assert_eq!(service.exec_start, "scripts/crypto.rhai");
        assert_eq!(service.timeout_sec, 3);
        assert_eq!(service.restart, RestartPolicy::No);
        assert_eq!(service.restart_sec, 1);
        assert_eq!(service.max_retries, 0);
        assert!(service.entry_point.is_none());
        assert!(service.on_failure.is_none());
    }

    #[test]
    fn trigger_variants_preserve_mvp_shape() {
        let command_trigger = TriggerSpec::command(["warn", "mute"]);
        let regex_trigger = TriggerSpec::regex("(?i)spam");
        let event_trigger =
            TriggerSpec::event_type([UnitEventType::Message, UnitEventType::CallbackQuery]);

        assert_eq!(command_trigger.trigger_type(), TriggerType::Command);
        assert_eq!(regex_trigger.trigger_type(), TriggerType::Regex);
        assert_eq!(event_trigger.trigger_type(), TriggerType::EventType);

        match command_trigger {
            TriggerSpec::Command { commands } => {
                assert_eq!(commands, vec!["warn".to_owned(), "mute".to_owned()]);
            }
            _ => panic!("expected command trigger"),
        }

        match regex_trigger {
            TriggerSpec::Regex { pattern } => assert_eq!(pattern, "(?i)spam"),
            _ => panic!("expected regex trigger"),
        }

        match event_trigger {
            TriggerSpec::EventType { events } => assert_eq!(
                events,
                vec![UnitEventType::Message, UnitEventType::CallbackQuery]
            ),
            _ => panic!("expected event_type trigger"),
        }
    }

    #[test]
    fn registry_summary_counts_runtime_states() {
        let manifest = UnitManifest::new(
            UnitDefinition::new("moderation.warn.unit"),
            TriggerSpec::command(["warn"]),
            ServiceSpec::new("scripts/moderation/warn.rhai"),
        );

        let registry = UnitRegistry::from_entries(vec![
            UnitDescriptor::new(manifest.clone(), UnitStatus::Active),
            UnitDescriptor::new(manifest.clone(), UnitStatus::Failed),
            UnitDescriptor::new(manifest, UnitStatus::Disabled),
        ]);

        assert_eq!(
            registry.status_summary(),
            UnitRegistryStatus {
                total_units: 3,
                active_units: 1,
                failed_units: 1,
                disabled_units: 1,
                ..UnitRegistryStatus::default()
            }
        );
    }

    #[test]
    fn runtime_spec_defaults_to_conservative_limits() {
        let runtime = RuntimeSpec::default();

        assert_eq!(runtime.max_memory_kb, None);
        assert_eq!(runtime.max_output_bytes, None);
        assert!(!runtime.dry_run_supported);
        assert!(!runtime.idempotent_by_default);
        assert!(!runtime.allow_in_recovery);
        assert!(!runtime.allow_manual_invoke);
    }
}
