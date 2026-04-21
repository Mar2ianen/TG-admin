use telegram_moderation_os::unit::{
    CapabilitiesSpec, RestartPolicy, ServiceSpec, TriggerSpec, UnitDefinition, UnitManifest,
    UnitManifestCheckError, UnitValidationError,
};

#[test]
fn valid_unit_manifest_passes_validation() {
    let manifest = UnitManifest::from_toml_str(
        r#"
[Unit]
Name = "test.unit"

[Trigger]
Type = "command"
Commands = ["test"]

[Service]
ExecStart = "scripts/test.rhai"
"#,
    )
    .expect("should parse");

    let result = manifest.validate();
    assert!(
        result.is_ok(),
        "valid manifest should pass: {:?}",
        result.err()
    );
}

#[test]
fn unit_with_all_fields_passes_validation() {
    let manifest = UnitManifest::from_toml_str(
        r#"
[Unit]
Name = "full.unit"
Description = "Full test unit"
After = ["dep.unit"]
Requires = ["req.unit"]
Wants = ["want.unit"]
Enabled = true
Tags = ["test", "moderation"]
Owner = "admin"
Version = "1.0.0"

[Trigger]
Type = "command"
Commands = ["test"]

[Service]
ExecStart = "scripts/test.rhai"
EntryPoint = "main"
TimeoutSec = 30
Restart = "on-failure"
RestartSec = 5
MaxRetries = 3
OnFailure = "alert.unit"

[Capabilities]
Allow = ["tg.read_basic", "tg.write_message"]
Deny = ["sys.http.fetch"]

[Runtime]
MaxMemoryKb = 65536
MaxOutputBytes = 16384
DryRunSupported = true
IdempotentByDefault = true
AllowInRecovery = true
AllowManualInvoke = true
"#,
    )
    .expect("should parse");

    let result = manifest.validate();
    assert!(
        result.is_ok(),
        "full manifest should pass: {:?}",
        result.err()
    );
}

#[test]
fn missing_exec_start_fails_validation() {
    let manifest = UnitManifest::from_toml_str(
        r#"
[Unit]
Name = "bad.unit"

[Trigger]
Type = "command"
Commands = ["test"]

[Service]
TimeoutSec = 30
"#,
    )
    .expect("should parse");

    let result = manifest.validate();
    assert!(result.is_err());
}

#[test]
fn empty_commands_fails_validation() {
    let manifest = UnitManifest::from_toml_str(
        r#"
[Unit]
Name = "empty.unit"

[Trigger]
Type = "command"
Commands = []

[Service]
ExecStart = "test.rhai"
"#,
    )
    .expect("should parse");

    let result = manifest.validate();
    assert!(result.is_err());
}

#[test]
fn invalid_regex_fails_validation() {
    let manifest = UnitManifest::new(
        UnitDefinition::new("regex.unit"),
        TriggerSpec::regex("(?invalid"),
        ServiceSpec::new("test.rhai"),
    );

    let result = manifest.validate();
    assert!(result.is_err());
}

#[test]
fn duplicate_unit_names_fail_validation() {
    let a = UnitManifest::from_toml_str(
        r#"
[Unit]
Name = "dup.unit"

[Trigger]
Type = "command"
Commands = ["test"]

[Service]
ExecStart = "a.rhai"
"#,
    )
    .expect("parse");

    let b = UnitManifest::from_toml_str(
        r#"
[Unit]
Name = "dup.unit"

[Trigger]
Type = "command"
Commands = ["test"]

[Service]
ExecStart = "b.rhai"
"#,
    )
    .expect("parse");

    let result = UnitManifest::validate_set(&[a, b]);
    assert!(result.is_err());
}

#[test]
fn missing_dependency_fails_validation() {
    let unit = UnitManifest::from_toml_str(
        r#"
[Unit]
Name = "unit.unit"
Requires = ["missing.unit"]

[Trigger]
Type = "command"
Commands = ["test"]

[Service]
ExecStart = "test.rhai"
"#,
    )
    .expect("parse");

    let result = UnitManifest::validate_set(&[unit]);
    assert!(result.is_err());
}

#[test]
fn dependency_cycle_fails_validation() {
    let a = UnitManifest::new(
        UnitDefinition {
            name: "a.unit".to_owned(),
            requires: vec!["b.unit".to_owned()],
            ..UnitDefinition::new("a.unit")
        },
        TriggerSpec::command(["test"]),
        ServiceSpec::new("a.rhai"),
    );

    let b = UnitManifest::new(
        UnitDefinition {
            name: "b.unit".to_owned(),
            requires: vec!["a.unit".to_owned()],
            ..UnitDefinition::new("b.unit")
        },
        TriggerSpec::command(["test"]),
        ServiceSpec::new("b.rhai"),
    );

    let result = UnitManifest::validate_set(&[a, b]);
    assert!(result.is_err());
}

#[test]
fn retry_policy_validation_enforces_rules() {
    let manifest = UnitManifest::new(
        UnitDefinition::new("retry.unit"),
        TriggerSpec::command(["test"]),
        ServiceSpec {
            exec_start: "test.rhai".to_owned(),
            timeout_sec: 30,
            restart: RestartPolicy::No,
            restart_sec: 5, // non-default but restart=no
            max_retries: 2, // non-zero but restart=no
            ..ServiceSpec::new("test.rhai")
        },
    );

    let result = manifest.validate();
    assert!(result.is_err());
}

#[test]
fn capability_validation_rejects_unknown() {
    let manifest = UnitManifest {
        unit: UnitDefinition::new("cap.unit"),
        trigger: TriggerSpec::command(["test"]),
        service: ServiceSpec::new("test.rhai"),
        capabilities: CapabilitiesSpec {
            allow: vec!["telegram.delete_message".to_owned()],
            deny: Vec::new(),
        },
        runtime: Default::default(),
    };

    let result = manifest
        .validate()
        .expect_err("unknown capability should fail");
    assert!(result
        .issues()
        .contains(&UnitValidationError::UnknownCapability {
            unit: "cap.unit".to_owned(),
            capability: "telegram.delete_message".to_owned(),
            location: telegram_moderation_os::unit::CapabilityListKind::Allow,
        }));
}
