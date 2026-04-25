use super::manifest::UnitManifest;
use super::types::*;
use super::validation::validate_dependencies;
use std::collections::{BTreeSet, HashMap};
use std::path::Path;

#[derive(Debug, Clone, Default)]
pub struct UnitRegistry {
    pub(crate) entries: Vec<UnitDescriptor>,
}

impl UnitRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_entries(entries: Vec<UnitDescriptor>) -> Self {
        Self { entries }
    }

    pub fn load_manifests(manifests: Vec<UnitManifest>) -> UnitRegistryLoadReport {
        build_registry_from_manifests(manifests)
    }

    pub fn load_paths(paths: impl IntoIterator<Item = impl AsRef<Path>>) -> UnitRegistryLoadReport {
        let mut parsed_manifests = Vec::new();
        let mut entries = Vec::new();

        for path in paths {
            let path = path.as_ref();
            match UnitManifest::from_path(path) {
                Ok(manifest) => parsed_manifests.push(manifest),
                Err(error) => entries.push(UnitDescriptor::failed_without_manifest(
                    path.display().to_string(),
                    vec![UnitDiagnostic::Load(UnitLoadDiagnostic::from_load_error(
                        &error,
                    ))],
                )),
            }
        }

        let mut report = build_registry_from_manifests(parsed_manifests);
        entries.append(&mut report.registry.entries);
        report.registry = UnitRegistry::from_entries(entries);
        report
    }

    pub fn apply_reload_manifests(
        &mut self,
        manifests: Vec<UnitManifest>,
    ) -> UnitRegistryApplyOutcome {
        let candidate = Self::load_manifests(manifests);
        if candidate.is_fully_valid() {
            *self = candidate.registry.clone();
            UnitRegistryApplyOutcome {
                applied: true,
                candidate,
            }
        } else {
            UnitRegistryApplyOutcome {
                applied: false,
                candidate,
            }
        }
    }

    pub fn apply_reload_paths(
        &mut self,
        paths: impl IntoIterator<Item = impl AsRef<Path>>,
    ) -> UnitRegistryApplyOutcome {
        let candidate = Self::load_paths(paths);
        if candidate.is_fully_valid() {
            *self = candidate.registry.clone();
            UnitRegistryApplyOutcome {
                applied: true,
                candidate,
            }
        } else {
            UnitRegistryApplyOutcome {
                applied: false,
                candidate,
            }
        }
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

    pub fn entries(&self) -> &[UnitDescriptor] {
        &self.entries
    }

    pub fn get(&self, id: &str) -> Option<&UnitDescriptor> {
        self.entries.iter().find(|entry| entry.id == id)
    }
}

#[derive(Debug, Clone)]
pub struct UnitRegistryLoadReport {
    pub registry: UnitRegistry,
}

impl UnitRegistryLoadReport {
    pub fn is_fully_valid(&self) -> bool {
        self.registry
            .entries
            .iter()
            .all(|entry| entry.status != UnitStatus::Failed)
    }
}

#[derive(Debug, Clone)]
pub struct UnitRegistryApplyOutcome {
    pub applied: bool,
    pub candidate: UnitRegistryLoadReport,
}

pub fn build_registry_from_manifests(manifests: Vec<UnitManifest>) -> UnitRegistryLoadReport {
    let diagnostics = collect_manifest_diagnostics(&manifests);
    let entries = manifests
        .into_iter()
        .map(|manifest| {
            let entry_diagnostics = diagnostics
                .get(manifest.name())
                .cloned()
                .unwrap_or_default();
            let status = status_for_manifest(&manifest, &entry_diagnostics);
            UnitDescriptor::from_manifest(manifest, status, entry_diagnostics)
        })
        .collect();

    UnitRegistryLoadReport {
        registry: UnitRegistry::from_entries(entries),
    }
}

pub fn collect_manifest_diagnostics(
    manifests: &[UnitManifest],
) -> HashMap<String, Vec<UnitDiagnostic>> {
    let mut diagnostics = HashMap::<String, Vec<UnitDiagnostic>>::new();

    if let Err(errors) = UnitManifest::validate_set(manifests) {
        for issue in errors.issues {
            attach_validation_issue(&mut diagnostics, issue);
        }
    }

    diagnostics
}

pub fn attach_validation_issue(
    diagnostics: &mut HashMap<String, Vec<UnitDiagnostic>>,
    issue: UnitValidationError,
) {
    match &issue {
        UnitValidationError::DependencyCycle { cycle } => {
            let mut units = BTreeSet::new();
            for unit in cycle.iter().take(cycle.len().saturating_sub(1)) {
                units.insert(unit.clone());
            }

            for unit in units {
                diagnostics
                    .entry(unit)
                    .or_default()
                    .push(UnitDiagnostic::Validation(issue.clone()));
            }
        }
        UnitValidationError::MissingExecStart { unit }
        | UnitValidationError::InvalidTriggerShape { unit, .. }
        | UnitValidationError::InvalidTimeoutShape { unit, .. }
        | UnitValidationError::InvalidRetryPolicy { unit, .. }
        | UnitValidationError::UnknownCapability { unit, .. }
        | UnitValidationError::DuplicateUnitName { unit }
        | UnitValidationError::MissingDependency { unit, .. } => {
            diagnostics
                .entry(unit.clone())
                .or_default()
                .push(UnitDiagnostic::Validation(issue));
        }
    }
}

pub fn status_for_manifest(manifest: &UnitManifest, diagnostics: &[UnitDiagnostic]) -> UnitStatus {
    if diagnostics.iter().any(|diagnostic| {
        matches!(
            diagnostic,
            UnitDiagnostic::Load(_) | UnitDiagnostic::Validation(_)
        )
    }) {
        UnitStatus::Failed
    } else if manifest.unit.enabled {
        UnitStatus::Active
    } else {
        UnitStatus::Disabled
    }
}
