use super::types::*;
use super::validation::{
    validate_capabilities, validate_dependencies, validate_service, validate_trigger,
};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

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
                issues.extend(errors.issues.clone());
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

pub const fn default_enabled() -> bool {
    true
}

pub const fn default_timeout_sec() -> u64 {
    3
}

pub const fn default_restart_sec() -> u64 {
    1
}
