use super::manifest::{
    CapabilitiesSpec, ServiceSpec, TriggerSpec, UnitManifest, default_restart_sec,
};
use super::types::*;
use regex::Regex;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::LazyLock;

pub(crate) static VALID_CAPABILITIES: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
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
        "ml.health.read",
        "ml.stt",
        "ml.embed_text",
        "ml.chat",
        "ml.models.read",
        "unit.control",
    ])
});

pub fn validate_trigger(
    unit_name: &str,
    trigger: &TriggerSpec,
    issues: &mut Vec<UnitValidationError>,
) {
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

pub fn validate_service(
    unit_name: &str,
    service: &ServiceSpec,
    issues: &mut Vec<UnitValidationError>,
) {
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

pub fn validate_capabilities(
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

pub fn validate_dependencies(manifests: &[UnitManifest], issues: &mut Vec<UnitValidationError>) {
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

pub fn collect_dependency_cycles(
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

pub fn canonicalize_cycle(mut cycle: Vec<String>) -> Vec<String> {
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

pub fn format_issues(issues: &[UnitValidationError]) -> String {
    issues
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("; ")
}
