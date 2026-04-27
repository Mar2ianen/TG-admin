use super::manifest::UnitManifest;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
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
pub enum UnitDiagnostic {
    Load(UnitLoadDiagnostic),
    Validation(UnitValidationError),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum UnitLoadDiagnostic {
    ReadFile {
        path: PathBuf,
        message: String,
    },
    ParseToml {
        source_name: String,
        message: String,
    },
}

impl UnitLoadDiagnostic {
    pub fn from_load_error(error: &UnitManifestLoadError) -> Self {
        match error {
            UnitManifestLoadError::ReadFile { path, source } => Self::ReadFile {
                path: path.clone(),
                message: source.to_string(),
            },
            UnitManifestLoadError::ParseToml {
                source_name,
                source,
            } => Self::ParseToml {
                source_name: source_name.clone(),
                message: source.to_string(),
            },
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnitStatus {
    Loaded,
    Active,
    Running,
    RetryWait,
    Failed,
    Dead,
    Disabled,
}

#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitDescriptor {
    pub id: String,
    pub manifest: Option<UnitManifest>,
    pub status: UnitStatus,
    pub diagnostics: Vec<UnitDiagnostic>,
}

impl UnitDescriptor {
    pub fn new(manifest: UnitManifest, status: UnitStatus) -> Self {
        Self {
            id: manifest.name().to_owned(),
            manifest: Some(manifest),
            status,
            diagnostics: Vec::new(),
        }
    }

    pub fn from_manifest(
        manifest: UnitManifest,
        status: UnitStatus,
        diagnostics: Vec<UnitDiagnostic>,
    ) -> Self {
        Self {
            id: manifest.name().to_owned(),
            manifest: Some(manifest),
            status,
            diagnostics,
        }
    }

    pub fn failed_without_manifest(id: String, diagnostics: Vec<UnitDiagnostic>) -> Self {
        Self {
            id,
            manifest: None,
            status: UnitStatus::Failed,
            diagnostics,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnitEventType {
    Message,
    CallbackQuery,
    MemberJoined,
    MemberLeft,
    MemberUpdated,
    ReactionAdded,
    ReactionRemoved,
    Job,
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

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{message}")]
pub struct UnitValidationErrors {
    pub issues: Vec<UnitValidationError>,
    pub message: String,
}

impl UnitValidationErrors {
    pub fn from_issues(issues: Vec<UnitValidationError>) -> Result<(), Self> {
        if issues.is_empty() {
            Ok(())
        } else {
            Err(Self {
                message: super::validation::format_issues(&issues),
                issues,
            })
        }
    }

    pub fn issues(&self) -> &[UnitValidationError] {
        &self.issues
    }

    pub fn into_issues(self) -> Vec<UnitValidationError> {
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
