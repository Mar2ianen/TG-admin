#![allow(dead_code)]

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum UnitManifestLoadError {
    #[error("failed to read unit manifest at {path}: {source}")]
    ReadFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse unit manifest TOML from {source_name}: {source}")]
    ParseToml {
        source_name: String,
        #[source]
        source: toml::de::Error,
    },
}

#[derive(Debug, Error)]
pub enum UnitManifestCheckError {
    #[error(transparent)]
    Load(#[from] UnitManifestLoadError),
    #[error(transparent)]
    Validation(#[from] UnitValidationErrors),
}

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

    pub fn from_toml_str(input: &str) -> Result<Self, UnitManifestLoadError> {
        Self::from_toml_source(input, "<inline unit manifest>")
    }

    pub fn from_path(path: impl AsRef<Path>) -> Result<Self, UnitManifestLoadError> {
        let path = path.as_ref();
        let contents =
            fs::read_to_string(path).map_err(|source| UnitManifestLoadError::ReadFile {
                path: path.to_path_buf(),
                source,
            })?;

        Self::from_toml_source(&contents, path.display().to_string())
    }

    pub fn load_and_validate_toml_str(input: &str) -> Result<Self, UnitManifestCheckError> {
        let manifest = Self::from_toml_str(input)?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn load_and_validate_path(path: impl AsRef<Path>) -> Result<Self, UnitManifestCheckError> {
        let manifest = Self::from_path(path)?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn validate(&self) -> Result<(), UnitValidationErrors> {
        let mut issues = Vec::new();
        let unit_name = self.name().to_owned();

        if self.service.exec_start.trim().is_empty() {
            issues.push(UnitValidationError::MissingExecStart {
                unit: unit_name.clone(),
            });
        }

        validate_trigger(&unit_name, &self.trigger, &mut issues);
        validate_service(&unit_name, &self.service, &mut issues);
        validate_capabilities(&unit_name, &self.capabilities, &mut issues);

        UnitValidationErrors::from_issues(issues)
    }

    pub fn validate_set(manifests: &[Self]) -> Result<(), UnitValidationErrors> {
        let mut issues = Vec::new();

        for manifest in manifests {
            if let Err(errors) = manifest.validate() {
                issues.extend(errors.into_issues());
            }
        }

        validate_dependencies(manifests, &mut issues);

        UnitValidationErrors::from_issues(issues)
    }

    fn from_toml_source(
        input: &str,
        source_name: impl Into<String>,
    ) -> Result<Self, UnitManifestLoadError> {
        toml::from_str(input).map_err(|source| UnitManifestLoadError::ParseToml {
            source_name: source_name.into(),
            source,
        })
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

static VALID_CAPABILITIES: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    HashSet::from([
        "tg.read_basic",
        "tg.write_message",
        "tg.moderate.delete",
        "tg.moderate.restrict",
        "tg.moderate.ban",
        "db.user.read",
        "db.user.write",
        "rules.read",
        "rules.write",
        "filter.read",
        "filter.write",
        "msg.history.read",
        "job.schedule",
        "audit.read",
        "audit.compensate",
        "ui.session.read",
        "ui.session.write",
        "sys.http.fetch",
        "ml.stt",
        "ml.embed_text",
        "unit.control",
    ])
});

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{message}")]
pub struct UnitValidationErrors {
    issues: Vec<UnitValidationError>,
    message: String,
}

impl UnitValidationErrors {
    fn from_issues(issues: Vec<UnitValidationError>) -> Result<(), Self> {
        if issues.is_empty() {
            Ok(())
        } else {
            Err(Self {
                message: format_issues(&issues),
                issues,
            })
        }
    }

    pub fn issues(&self) -> &[UnitValidationError] {
        &self.issues
    }

    fn into_issues(self) -> Vec<UnitValidationError> {
        self.issues
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Error)]
pub enum UnitValidationError {
    #[error("unit `{unit}` is missing Service.ExecStart")]
    MissingExecStart { unit: String },
    #[error("unit `{unit}` has invalid trigger shape for `{trigger_type:?}`: {detail}")]
    InvalidTriggerShape {
        unit: String,
        trigger_type: TriggerType,
        detail: TriggerValidationDetail,
    },
    #[error("unit `{unit}` has invalid timeout shape: {detail}")]
    InvalidTimeoutShape {
        unit: String,
        detail: TimeoutValidationDetail,
    },
    #[error("unit `{unit}` has invalid retry policy: {detail}")]
    InvalidRetryPolicy {
        unit: String,
        detail: RetryValidationDetail,
    },
    #[error("unit `{unit}` references missing dependency `{dependency}` in `{relation:?}`")]
    MissingDependency {
        unit: String,
        dependency: String,
        relation: UnitDependencyRelation,
    },
    #[error("unit dependency cycle detected: {cycle:?}")]
    DependencyCycle { cycle: Vec<String> },
    #[error("unit `{unit}` requests unknown capability `{capability}` in `{location:?}`")]
    UnknownCapability {
        unit: String,
        capability: String,
        location: CapabilityListKind,
    },
    #[error("duplicate unit name `{unit}` in manifest set")]
    DuplicateUnitName { unit: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TriggerValidationDetail {
    EmptyCommands,
    EmptyRegexPattern,
    InvalidRegexPattern { message: String },
    EmptyEventList,
    BlankCommandName,
}

impl std::fmt::Display for TriggerValidationDetail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyCommands => f.write_str("command trigger requires at least one command"),
            Self::EmptyRegexPattern => f.write_str("regex trigger requires a non-empty pattern"),
            Self::InvalidRegexPattern { message } => {
                write!(f, "regex pattern failed to compile: {message}")
            }
            Self::EmptyEventList => f.write_str("event_type trigger requires at least one event"),
            Self::BlankCommandName => f.write_str("command trigger contains a blank command name"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TimeoutValidationDetail {
    NonPositiveTimeout,
}

impl std::fmt::Display for TimeoutValidationDetail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NonPositiveTimeout => f.write_str("Service.TimeoutSec must be greater than zero"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RetryValidationDetail {
    RestartDelayRequiresRetries,
    RetryCountRequiresRestart,
    NonPositiveRestartDelay,
}

impl std::fmt::Display for RetryValidationDetail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RestartDelayRequiresRetries => f.write_str(
                "Service.RestartSec must stay at the safe default when Restart = \"no\"",
            ),
            Self::RetryCountRequiresRestart => {
                f.write_str("Service.MaxRetries must be zero when Restart = \"no\"")
            }
            Self::NonPositiveRestartDelay => {
                f.write_str("Service.RestartSec must be greater than zero when restart is enabled")
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CapabilityListKind {
    Allow,
    Deny,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UnitDependencyRelation {
    After,
    Requires,
    Wants,
}

fn validate_trigger(unit_name: &str, trigger: &TriggerSpec, issues: &mut Vec<UnitValidationError>) {
    match trigger {
        TriggerSpec::Command { commands } => {
            if commands.is_empty() {
                issues.push(UnitValidationError::InvalidTriggerShape {
                    unit: unit_name.to_owned(),
                    trigger_type: TriggerType::Command,
                    detail: TriggerValidationDetail::EmptyCommands,
                });
            }

            for command in commands {
                if command.trim().is_empty() {
                    issues.push(UnitValidationError::InvalidTriggerShape {
                        unit: unit_name.to_owned(),
                        trigger_type: TriggerType::Command,
                        detail: TriggerValidationDetail::BlankCommandName,
                    });
                }
            }
        }
        TriggerSpec::Regex { pattern } => {
            if pattern.trim().is_empty() {
                issues.push(UnitValidationError::InvalidTriggerShape {
                    unit: unit_name.to_owned(),
                    trigger_type: TriggerType::Regex,
                    detail: TriggerValidationDetail::EmptyRegexPattern,
                });
            } else if let Err(error) = Regex::new(pattern) {
                issues.push(UnitValidationError::InvalidTriggerShape {
                    unit: unit_name.to_owned(),
                    trigger_type: TriggerType::Regex,
                    detail: TriggerValidationDetail::InvalidRegexPattern {
                        message: error.to_string(),
                    },
                });
            }
        }
        TriggerSpec::EventType { events } => {
            if events.is_empty() {
                issues.push(UnitValidationError::InvalidTriggerShape {
                    unit: unit_name.to_owned(),
                    trigger_type: TriggerType::EventType,
                    detail: TriggerValidationDetail::EmptyEventList,
                });
            }
        }
    }
}

fn validate_service(unit_name: &str, service: &ServiceSpec, issues: &mut Vec<UnitValidationError>) {
    if service.timeout_sec == 0 {
        issues.push(UnitValidationError::InvalidTimeoutShape {
            unit: unit_name.to_owned(),
            detail: TimeoutValidationDetail::NonPositiveTimeout,
        });
    }

    match service.restart {
        RestartPolicy::No => {
            if service.max_retries > 0 {
                issues.push(UnitValidationError::InvalidRetryPolicy {
                    unit: unit_name.to_owned(),
                    detail: RetryValidationDetail::RetryCountRequiresRestart,
                });
            }

            if service.restart_sec != default_restart_sec() {
                issues.push(UnitValidationError::InvalidRetryPolicy {
                    unit: unit_name.to_owned(),
                    detail: RetryValidationDetail::RestartDelayRequiresRetries,
                });
            }
        }
        RestartPolicy::OnFailure | RestartPolicy::Always => {
            if service.restart_sec == 0 {
                issues.push(UnitValidationError::InvalidRetryPolicy {
                    unit: unit_name.to_owned(),
                    detail: RetryValidationDetail::NonPositiveRestartDelay,
                });
            }
        }
    }
}

fn validate_capabilities(
    unit_name: &str,
    capabilities: &CapabilitiesSpec,
    issues: &mut Vec<UnitValidationError>,
) {
    for capability in &capabilities.allow {
        if !VALID_CAPABILITIES.contains(capability.as_str()) {
            issues.push(UnitValidationError::UnknownCapability {
                unit: unit_name.to_owned(),
                capability: capability.clone(),
                location: CapabilityListKind::Allow,
            });
        }
    }

    for capability in &capabilities.deny {
        if !VALID_CAPABILITIES.contains(capability.as_str()) {
            issues.push(UnitValidationError::UnknownCapability {
                unit: unit_name.to_owned(),
                capability: capability.clone(),
                location: CapabilityListKind::Deny,
            });
        }
    }
}

fn validate_dependencies(manifests: &[UnitManifest], issues: &mut Vec<UnitValidationError>) {
    let mut names = HashMap::new();

    for manifest in manifests {
        let unit_name = manifest.name().to_owned();
        if names.insert(unit_name.clone(), manifest).is_some() {
            issues.push(UnitValidationError::DuplicateUnitName { unit: unit_name });
        }
    }

    for manifest in manifests {
        let unit_name = manifest.name().to_owned();
        for dependency in &manifest.unit.after {
            if !names.contains_key(dependency) {
                issues.push(UnitValidationError::MissingDependency {
                    unit: unit_name.clone(),
                    dependency: dependency.clone(),
                    relation: UnitDependencyRelation::After,
                });
            }
        }
        for dependency in &manifest.unit.requires {
            if !names.contains_key(dependency) {
                issues.push(UnitValidationError::MissingDependency {
                    unit: unit_name.clone(),
                    dependency: dependency.clone(),
                    relation: UnitDependencyRelation::Requires,
                });
            }
        }
        for dependency in &manifest.unit.wants {
            if !names.contains_key(dependency) {
                issues.push(UnitValidationError::MissingDependency {
                    unit: unit_name.clone(),
                    dependency: dependency.clone(),
                    relation: UnitDependencyRelation::Wants,
                });
            }
        }
    }

    let graph = manifests
        .iter()
        .map(|manifest| {
            let dependencies = manifest
                .unit
                .after
                .iter()
                .chain(&manifest.unit.requires)
                .chain(&manifest.unit.wants)
                .filter(|dependency| names.contains_key((*dependency).as_str()))
                .cloned()
                .collect::<Vec<_>>();
            (manifest.name().to_owned(), dependencies)
        })
        .collect::<HashMap<_, _>>();

    let mut visited = HashSet::new();
    let mut active = HashSet::new();
    let mut stack = Vec::new();
    let mut cycles = BTreeSet::new();

    for name in graph.keys() {
        collect_dependency_cycles(
            name,
            &graph,
            &mut visited,
            &mut active,
            &mut stack,
            &mut cycles,
        );
    }

    for cycle in cycles {
        issues.push(UnitValidationError::DependencyCycle {
            cycle: cycle.into_iter().collect(),
        });
    }
}

fn collect_dependency_cycles(
    current: &str,
    graph: &HashMap<String, Vec<String>>,
    visited: &mut HashSet<String>,
    active: &mut HashSet<String>,
    stack: &mut Vec<String>,
    cycles: &mut BTreeSet<Vec<String>>,
) {
    if active.contains(current) {
        if let Some(index) = stack.iter().position(|entry| entry == current) {
            let mut cycle = stack[index..].to_vec();
            cycle.push(current.to_owned());
            let canonical = canonicalize_cycle(cycle);
            cycles.insert(canonical);
        }
        return;
    }

    if !visited.insert(current.to_owned()) {
        return;
    }

    active.insert(current.to_owned());
    stack.push(current.to_owned());

    if let Some(dependencies) = graph.get(current) {
        for dependency in dependencies {
            collect_dependency_cycles(dependency, graph, visited, active, stack, cycles);
        }
    }

    stack.pop();
    active.remove(current);
}

fn canonicalize_cycle(mut cycle: Vec<String>) -> Vec<String> {
    if cycle.len() <= 1 {
        return cycle;
    }

    cycle.pop();
    let pivot = cycle
        .iter()
        .enumerate()
        .min_by(|(_, left), (_, right)| left.cmp(right))
        .map(|(index, _)| index)
        .unwrap_or(0);
    cycle.rotate_left(pivot);
    cycle.push(cycle[0].clone());
    cycle
}

fn format_issues(issues: &[UnitValidationError]) -> String {
    issues
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

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

    #[test]
    fn parses_valid_minimal_unit_manifest_from_inline_toml() {
        let manifest = UnitManifest::from_toml_str(
            r#"
[Unit]
Name = "healthcheck.unit"

[Trigger]
Type = "event_type"
Events = ["job"]

[Service]
ExecStart = "scripts/healthcheck.rhai"
"#,
        )
        .expect("minimal unit manifest should parse");

        assert_eq!(manifest.name(), "healthcheck.unit");
        assert_eq!(manifest.unit, UnitDefinition::new("healthcheck.unit"));
        assert_eq!(
            manifest.trigger,
            TriggerSpec::EventType {
                events: vec![UnitEventType::Job],
            }
        );
        assert_eq!(
            manifest.service,
            ServiceSpec::new("scripts/healthcheck.rhai")
        );
        assert_eq!(manifest.capabilities, CapabilitiesSpec::default());
        assert_eq!(manifest.runtime, RuntimeSpec::default());
    }

    #[test]
    fn parses_valid_moderation_command_unit_manifest_from_file() {
        let manifest_path = write_manifest(
            "warn-moderation.unit.toml",
            r#"
[Unit]
Name = "moderation.warn.unit"
Description = "Warn or mute users based on moderation command invocations"
After = ["bootstrap.runtime"]
Requires = ["storage.sqlite"]
Wants = ["audit.log"]
Enabled = true
Tags = ["moderation", "command"]
Owner = "ops"
Version = "1.0.0"

[Trigger]
Type = "command"
Commands = ["warn", "mute", "del"]

[Service]
ExecStart = "scripts/moderation/warn.rhai"
EntryPoint = "main"
TimeoutSec = 8
Restart = "on-failure"
RestartSec = 5
MaxRetries = 3
OnFailure = "moderation.alert.unit"

[Capabilities]
Allow = ["telegram.delete_message", "telegram.restrict_member", "audit.write"]
Deny = ["http.external"]

[Runtime]
MaxMemoryKb = 65536
MaxOutputBytes = 16384
DryRunSupported = true
IdempotentByDefault = true
AllowInRecovery = true
AllowManualInvoke = true
"#,
        );

        let manifest = UnitManifest::from_path(&manifest_path)
            .expect("moderation command unit manifest should parse from file");

        assert_eq!(manifest.name(), "moderation.warn.unit");
        assert_eq!(
            manifest.unit.description.as_deref(),
            Some("Warn or mute users based on moderation command invocations")
        );
        assert_eq!(manifest.unit.after, vec!["bootstrap.runtime"]);
        assert_eq!(manifest.unit.requires, vec!["storage.sqlite"]);
        assert_eq!(manifest.unit.wants, vec!["audit.log"]);
        assert!(manifest.unit.enabled);
        assert_eq!(manifest.unit.tags, vec!["moderation", "command"]);
        assert_eq!(manifest.unit.owner.as_deref(), Some("ops"));
        assert_eq!(manifest.unit.version.as_deref(), Some("1.0.0"));

        assert_eq!(
            manifest.trigger,
            TriggerSpec::Command {
                commands: vec!["warn".into(), "mute".into(), "del".into()],
            }
        );
        assert_eq!(manifest.trigger.trigger_type(), TriggerType::Command);

        assert_eq!(manifest.service.exec_start, "scripts/moderation/warn.rhai");
        assert_eq!(manifest.service.entry_point.as_deref(), Some("main"));
        assert_eq!(manifest.service.timeout_sec, 8);
        assert_eq!(manifest.service.restart, RestartPolicy::OnFailure);
        assert_eq!(manifest.service.restart_sec, 5);
        assert_eq!(manifest.service.max_retries, 3);
        assert_eq!(
            manifest.service.on_failure.as_deref(),
            Some("moderation.alert.unit")
        );

        assert_eq!(
            manifest.capabilities,
            CapabilitiesSpec {
                allow: vec![
                    "telegram.delete_message".into(),
                    "telegram.restrict_member".into(),
                    "audit.write".into(),
                ],
                deny: vec!["http.external".into()],
            }
        );
        assert_eq!(
            manifest.runtime,
            RuntimeSpec {
                max_memory_kb: Some(65536),
                max_output_bytes: Some(16384),
                dry_run_supported: true,
                idempotent_by_default: true,
                allow_in_recovery: true,
                allow_manual_invoke: true,
            }
        );
    }

    #[test]
    fn parses_regex_trigger_and_service_runtime_shapes() {
        let manifest = UnitManifest::from_toml_str(
            r#"
[Unit]
Name = "spam.regex.unit"
Enabled = false

[Trigger]
Type = "regex"
Pattern = "(?i)free\\s+nitro"

[Service]
ExecStart = "scripts/moderation/spam.rhai"

[Capabilities]
Allow = ["telegram.delete_message"]

[Runtime]
AllowManualInvoke = true
"#,
        )
        .expect("regex unit manifest should parse");

        assert!(!manifest.unit.enabled);
        assert_eq!(
            manifest.trigger,
            TriggerSpec::Regex {
                pattern: "(?i)free\\s+nitro".into(),
            }
        );
        assert_eq!(manifest.service.timeout_sec, 3);
        assert_eq!(manifest.service.restart, RestartPolicy::No);
        assert_eq!(manifest.service.restart_sec, 1);
        assert_eq!(manifest.service.max_retries, 0);
        assert_eq!(
            manifest.capabilities,
            CapabilitiesSpec {
                allow: vec!["telegram.delete_message".into()],
                deny: Vec::new(),
            }
        );
        assert_eq!(
            manifest.runtime,
            RuntimeSpec {
                allow_manual_invoke: true,
                ..RuntimeSpec::default()
            }
        );
    }

    #[test]
    fn load_and_validate_rejects_unknown_trigger_type() {
        let error = UnitManifest::load_and_validate_toml_str(
            r#"
[Unit]
Name = "unknown-trigger.unit"

[Trigger]
Type = "semantic"
Namespace = "moderation"

[Service]
ExecStart = "scripts/moderation/warn.rhai"
"#,
        )
        .expect_err("unknown trigger type should fail during load");

        match error {
            UnitManifestCheckError::Load(UnitManifestLoadError::ParseToml {
                source_name,
                source,
            }) => {
                assert_eq!(source_name, "<inline unit manifest>");
                assert!(source.to_string().contains("semantic"));
            }
            other => panic!("expected parse error, got {other:?}"),
        }
    }

    #[test]
    fn load_and_validate_rejects_missing_exec_start() {
        let error = UnitManifest::load_and_validate_toml_str(
            r#"
[Unit]
Name = "missing-exec-start.unit"

[Trigger]
Type = "event_type"
Events = ["job"]

[Service]
TimeoutSec = 3
"#,
        )
        .expect_err("missing ExecStart should fail during load");

        match error {
            UnitManifestCheckError::Load(UnitManifestLoadError::ParseToml { source, .. }) => {
                assert!(source.to_string().contains("ExecStart"));
            }
            other => panic!("expected parse error, got {other:?}"),
        }
    }

    #[test]
    fn validate_rejects_invalid_trigger_timeout_retry_and_capability_shapes() {
        let manifest = UnitManifest {
            unit: UnitDefinition::new("moderation.invalid.unit"),
            trigger: TriggerSpec::Command {
                commands: vec!["warn".into(), " ".into()],
            },
            service: ServiceSpec {
                exec_start: "  ".into(),
                timeout_sec: 0,
                restart: RestartPolicy::No,
                restart_sec: 5,
                max_retries: 2,
                ..ServiceSpec::new("scripts/moderation/warn.rhai")
            },
            capabilities: CapabilitiesSpec {
                allow: vec![
                    "tg.moderate.restrict".into(),
                    "telegram.delete_message".into(),
                ],
                deny: vec!["sys.shell.exec".into()],
            },
            runtime: RuntimeSpec::default(),
        };

        let error = manifest
            .validate()
            .expect_err("invalid manifest should not validate");

        assert!(
            error
                .issues()
                .contains(&UnitValidationError::MissingExecStart {
                    unit: "moderation.invalid.unit".into(),
                })
        );
        assert!(
            error
                .issues()
                .contains(&UnitValidationError::InvalidTriggerShape {
                    unit: "moderation.invalid.unit".into(),
                    trigger_type: TriggerType::Command,
                    detail: TriggerValidationDetail::BlankCommandName,
                })
        );
        assert!(
            error
                .issues()
                .contains(&UnitValidationError::InvalidTimeoutShape {
                    unit: "moderation.invalid.unit".into(),
                    detail: TimeoutValidationDetail::NonPositiveTimeout,
                })
        );
        assert!(
            error
                .issues()
                .contains(&UnitValidationError::InvalidRetryPolicy {
                    unit: "moderation.invalid.unit".into(),
                    detail: RetryValidationDetail::RetryCountRequiresRestart,
                })
        );
        assert!(
            error
                .issues()
                .contains(&UnitValidationError::InvalidRetryPolicy {
                    unit: "moderation.invalid.unit".into(),
                    detail: RetryValidationDetail::RestartDelayRequiresRetries,
                })
        );
        assert!(
            error
                .issues()
                .contains(&UnitValidationError::UnknownCapability {
                    unit: "moderation.invalid.unit".into(),
                    capability: "telegram.delete_message".into(),
                    location: CapabilityListKind::Allow,
                })
        );
        assert!(
            error
                .issues()
                .contains(&UnitValidationError::UnknownCapability {
                    unit: "moderation.invalid.unit".into(),
                    capability: "sys.shell.exec".into(),
                    location: CapabilityListKind::Deny,
                })
        );
    }

    #[test]
    fn validate_rejects_invalid_regex_trigger_shape() {
        let manifest = UnitManifest::new(
            UnitDefinition::new("moderation.regex.unit"),
            TriggerSpec::Regex {
                pattern: "(".into(),
            },
            ServiceSpec::new("scripts/moderation/regex.rhai"),
        );

        let error = manifest
            .validate()
            .expect_err("invalid regex pattern should fail validation");

        assert!(matches!(
            error.issues(),
            [UnitValidationError::InvalidTriggerShape {
                trigger_type: TriggerType::Regex,
                detail: TriggerValidationDetail::InvalidRegexPattern { .. },
                ..
            }]
        ));
    }

    #[test]
    fn validate_set_rejects_missing_dependency_and_dependency_cycle() {
        let alpha = UnitManifest {
            unit: UnitDefinition {
                name: "alpha.unit".into(),
                requires: vec!["beta.unit".into()],
                ..UnitDefinition::new("alpha.unit")
            },
            trigger: TriggerSpec::event_type([UnitEventType::Job]),
            service: ServiceSpec::new("scripts/alpha.rhai"),
            capabilities: CapabilitiesSpec {
                allow: vec!["job.schedule".into()],
                deny: Vec::new(),
            },
            runtime: RuntimeSpec::default(),
        };
        let beta = UnitManifest {
            unit: UnitDefinition {
                name: "beta.unit".into(),
                requires: vec!["alpha.unit".into()],
                wants: vec!["missing.unit".into()],
                ..UnitDefinition::new("beta.unit")
            },
            trigger: TriggerSpec::event_type([UnitEventType::Job]),
            service: ServiceSpec::new("scripts/beta.rhai"),
            capabilities: CapabilitiesSpec {
                allow: vec!["job.schedule".into()],
                deny: Vec::new(),
            },
            runtime: RuntimeSpec::default(),
        };

        let error = UnitManifest::validate_set(&[alpha, beta])
            .expect_err("invalid dependency graph should fail validation");

        assert!(
            error
                .issues()
                .contains(&UnitValidationError::MissingDependency {
                    unit: "beta.unit".into(),
                    dependency: "missing.unit".into(),
                    relation: UnitDependencyRelation::Wants,
                })
        );
        assert!(
            error
                .issues()
                .contains(&UnitValidationError::DependencyCycle {
                    cycle: vec!["alpha.unit".into(), "beta.unit".into(), "alpha.unit".into()],
                })
        );
    }

    #[test]
    fn validate_set_accepts_valid_manifest_graph() {
        let storage = UnitManifest {
            unit: UnitDefinition::new("storage.sqlite"),
            trigger: TriggerSpec::event_type([UnitEventType::Job]),
            service: ServiceSpec::new("scripts/storage.rhai"),
            capabilities: CapabilitiesSpec {
                allow: vec!["db.user.read".into()],
                deny: Vec::new(),
            },
            runtime: RuntimeSpec::default(),
        };
        let warn = UnitManifest {
            unit: UnitDefinition {
                name: "moderation.warn.unit".into(),
                requires: vec!["storage.sqlite".into()],
                ..UnitDefinition::new("moderation.warn.unit")
            },
            trigger: TriggerSpec::command(["warn"]),
            service: ServiceSpec {
                restart: RestartPolicy::OnFailure,
                restart_sec: 2,
                max_retries: 3,
                ..ServiceSpec::new("scripts/moderation/warn.rhai")
            },
            capabilities: CapabilitiesSpec {
                allow: vec!["tg.moderate.restrict".into(), "audit.compensate".into()],
                deny: vec!["sys.http.fetch".into()],
            },
            runtime: RuntimeSpec::default(),
        };

        UnitManifest::validate_set(&[storage, warn]).expect("valid manifest set should pass");
    }

    fn write_manifest(file_name: &str, contents: &str) -> PathBuf {
        let file = NamedTempFile::with_suffix(file_name).expect("temp manifest file");
        std::fs::write(file.path(), contents).expect("write manifest fixture");
        file.into_temp_path()
            .keep()
            .expect("persist manifest fixture")
    }
}
