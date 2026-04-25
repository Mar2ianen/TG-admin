use super::*;
use std::path::{Path, PathBuf};
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
Allow = ["tg.moderate.delete", "tg.moderate.restrict", "audit.compensate"]
Deny = ["sys.http.fetch"]

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
                "tg.moderate.delete".into(),
                "tg.moderate.restrict".into(),
                "audit.compensate".into(),
            ],
            deny: vec!["sys.http.fetch".into()],
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
Allow = ["tg.moderate.delete"]

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
            allow: vec!["tg.moderate.delete".into()],
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

#[test]
fn valid_unit_loads_as_active_in_registry() {
    let report = UnitRegistry::load_manifests(vec![valid_warn_unit()]);

    let entry = report
        .registry
        .get("moderation.warn.unit")
        .expect("valid unit should exist in registry");

    assert!(report.is_fully_valid());
    assert_eq!(entry.status, UnitStatus::Active);
    assert!(entry.diagnostics.is_empty());
    assert!(entry.manifest.is_some());
}

#[test]
fn invalid_unit_becomes_failed_without_breaking_other_runtime_entries() {
    let report = UnitRegistry::load_manifests(vec![valid_warn_unit(), invalid_warn_unit()]);

    assert!(!report.is_fully_valid());
    assert_eq!(
        report.registry.status_summary(),
        UnitRegistryStatus {
            total_units: 2,
            active_units: 1,
            failed_units: 1,
            ..UnitRegistryStatus::default()
        }
    );

    let failed = report
        .registry
        .get("moderation.invalid.unit")
        .expect("invalid unit should remain visible in registry");
    assert_eq!(failed.status, UnitStatus::Failed);
    assert!(!failed.diagnostics.is_empty());
}

#[test]
fn disabled_unit_loads_with_disabled_state() {
    let mut manifest = valid_warn_unit();
    manifest.unit.name = "moderation.warn.disabled.unit".into();
    manifest.unit.enabled = false;

    let report = UnitRegistry::load_manifests(vec![manifest]);
    let entry = report
        .registry
        .get("moderation.warn.disabled.unit")
        .expect("disabled unit should be present");

    assert!(report.is_fully_valid());
    assert_eq!(entry.status, UnitStatus::Disabled);
    assert!(entry.diagnostics.is_empty());
}

#[test]
fn reload_keeps_old_registry_when_new_set_fails_validation() {
    let mut registry = UnitRegistry::load_manifests(vec![valid_warn_unit()]).registry;

    let outcome = registry.apply_reload_manifests(vec![invalid_warn_unit()]);

    assert!(!outcome.applied);
    assert_eq!(registry.len(), 1);
    assert_eq!(
        registry
            .get("moderation.warn.unit")
            .expect("original registry entry should remain")
            .status,
        UnitStatus::Active
    );
    assert_eq!(
        outcome.candidate.registry.status_summary(),
        UnitRegistryStatus {
            total_units: 1,
            failed_units: 1,
            ..UnitRegistryStatus::default()
        }
    );
}

#[test]
fn path_load_keeps_parse_failures_as_failed_entries() {
    let valid_path = write_manifest(
        "warn.unit.toml",
        r#"
[Unit]
Name = "moderation.warn.unit"

[Trigger]
Type = "command"
Commands = ["warn"]

[Service]
ExecStart = "scripts/moderation/warn.rhai"
"#,
    );
    let invalid_path = write_manifest(
        "broken.unit.toml",
        r#"
[Unit]
Name = "broken.unit"

[Trigger]
Type = "command"
Commands = ["warn"

[Service]
ExecStart = "scripts/moderation/warn.rhai"
"#,
    );

    let report = UnitRegistry::load_paths([valid_path.as_path(), invalid_path.as_path()]);

    assert_eq!(
        report.registry.status_summary(),
        UnitRegistryStatus {
            total_units: 2,
            active_units: 1,
            failed_units: 1,
            ..UnitRegistryStatus::default()
        }
    );
    assert!(report.registry.entries().iter().any(|entry| matches!(
        entry.diagnostics.as_slice(),
        [UnitDiagnostic::Load(UnitLoadDiagnostic::ParseToml { .. })]
    )));
}

fn valid_warn_unit() -> UnitManifest {
    UnitManifest {
        unit: UnitDefinition::new("moderation.warn.unit"),
        trigger: TriggerSpec::command(["warn"]),
        service: ServiceSpec::new("scripts/moderation/warn.rhai"),
        capabilities: CapabilitiesSpec {
            allow: vec!["tg.moderate.restrict".into()],
            deny: Vec::new(),
        },
        runtime: RuntimeSpec::default(),
    }
}

fn invalid_warn_unit() -> UnitManifest {
    UnitManifest {
        unit: UnitDefinition::new("moderation.invalid.unit"),
        trigger: TriggerSpec::command(["warn"]),
        service: ServiceSpec {
            exec_start: " ".into(),
            ..ServiceSpec::new("scripts/moderation/warn.rhai")
        },
        capabilities: CapabilitiesSpec {
            allow: vec!["tg.moderate.restrict".into()],
            deny: Vec::new(),
        },
        runtime: RuntimeSpec::default(),
    }
}

fn write_manifest(file_name: &str, contents: &str) -> PathBuf {
    let file = NamedTempFile::with_suffix(file_name).expect("temp manifest file");
    std::fs::write(file.path(), contents).expect("write manifest fixture");
    file.into_temp_path()
        .keep()
        .expect("persist manifest fixture")
}
