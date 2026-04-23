    use super::{
        AuditCompensateRequest, AuditFindRequest, CtxExpandReasonRequest, CtxParseDurationRequest,
        CtxResolveTargetRequest, DbKvGetRequest, DbKvSetRequest, DbUserGetRequest,
        DbUserIncrRequest, DbUserPatchRequest, HostApi, HostApiError, HostApiErrorDetail,
        HostApiErrorKind, HostApiOperation, HostApiRequest, HostApiValue, JobScheduleAfterRequest,
        MsgByUserRequest, MsgWindowRequest, UnitStatusEntry, UnitStatusRequest,
    };
    use crate::event::{
        ChatContext, EventContext, EventNormalizer, ExecutionMode, ManualInvocationInput,
        ReplyContext, SystemContext, SystemOrigin, UnitContext, UpdateType,
    };
    use crate::parser::command::ReasonExpr;
    use crate::parser::duration::{DurationParseError, DurationUnit, ParsedDuration};
    use crate::parser::reason::{ExpandedReason, ReasonAliasDefinition, ReasonAliasRegistry};
    use crate::parser::target::{ParsedTargetSelector, TargetParseError, TargetSource};
    use crate::storage::{
        AuditLogEntry, AuditLogFilter, KvEntry, MessageJournalRecord, Storage, UserPatch,
    };
    use crate::unit::{
        CapabilitiesSpec, ServiceSpec, TriggerSpec, UnitDefinition, UnitManifest, UnitRegistry,
        UnitStatus,
    };
    use chrono::{TimeZone, Utc};
    use serde_json::json;
    use tempfile::TempDir;

    fn ts() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 4, 21, 12, 0, 0)
            .single()
            .expect("valid timestamp")
    }

    fn manual_event() -> EventContext {
        let normalizer = EventNormalizer::new();
        let mut input = ManualInvocationInput::new(
            UnitContext::new("moderation.test").with_trigger("manual"),
            "/warn @spam spam",
        );
        input.event_id = Some("evt_host_api_manual".to_owned());
        input.received_at = ts();
        input.chat = Some(ChatContext {
            id: -100123,
            chat_type: "supergroup".to_owned(),
            title: Some("Moderation HQ".to_owned()),
            username: Some("mod_hq".to_owned()),
            thread_id: Some(7),
        });
        input.reply = Some(ReplyContext {
            message_id: 99,
            sender_user_id: Some(77),
            sender_username: Some("reply_user".to_owned()),
            text: Some("reply".to_owned()),
            has_media: false,
        });

        normalizer
            .normalize_manual(input)
            .expect("manual event normalizes")
    }

    fn storage_api() -> (TempDir, HostApi) {
        let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        let path = dir.path().join("host-api.sqlite3");
        let storage = Storage::new(path)
            .init()
            .unwrap_or_else(|error| panic!("storage init failed: {error}"));
        (dir, HostApi::new(false).with_storage(storage))
    }

    fn dry_run_storage_api() -> (TempDir, HostApi) {
        let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        let path = dir.path().join("host-api.sqlite3");
        let storage = Storage::new(path)
            .init()
            .unwrap_or_else(|error| panic!("storage init failed: {error}"));
        (dir, HostApi::new(true).with_storage(storage))
    }

    fn storage_api_with_registry(
        allow: &[&str],
        deny: &[&str],
        dry_run: bool,
    ) -> (TempDir, HostApi) {
        let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        let path = dir.path().join("host-api.sqlite3");
        let storage = Storage::new(path)
            .init()
            .unwrap_or_else(|error| panic!("storage init failed: {error}"));

        let mut manifest = UnitManifest::new(
            UnitDefinition::new("moderation.test"),
            TriggerSpec::command(["warn"]),
            ServiceSpec::new("cargo run"),
        );
        manifest.capabilities = CapabilitiesSpec {
            allow: allow.iter().map(|value| (*value).to_owned()).collect(),
            deny: deny.iter().map(|value| (*value).to_owned()).collect(),
        };
        let registry = UnitRegistry::load_manifests(vec![manifest]).registry;

        let api = HostApi::new(dry_run)
            .with_storage(storage)
            .with_unit_registry(registry);
        (dir, api)
    }

    fn unit_registry_api() -> HostApi {
        let active = UnitManifest::new(
            UnitDefinition::new("moderation.warn"),
            TriggerSpec::command(["warn"]),
            ServiceSpec::new("cargo run"),
        );
        let mut disabled = UnitManifest::new(
            UnitDefinition::new("moderation.mute"),
            TriggerSpec::command(["mute"]),
            ServiceSpec::new("cargo run"),
        );
        disabled.unit.enabled = false;

        let report = UnitRegistry::load_manifests(vec![active, disabled]);
        assert!(report.is_fully_valid());

        HostApi::new(false).with_unit_registry(report.registry)
    }

    fn seed_message_journal(api: &HostApi) {
        let storage = api
            .storage(HostApiOperation::MsgWindow)
            .expect("storage available");
        for (message_id, user_id, text, date_utc) in [
            (
                81229_i64,
                Some(99887766_i64),
                Some("spam 1"),
                "2026-04-21T11:59:00Z",
            ),
            (
                81230,
                Some(99887766),
                Some("spam 2"),
                "2026-04-21T11:59:10Z",
            ),
            (
                81231,
                Some(99887766),
                Some("spam 3"),
                "2026-04-21T11:59:20Z",
            ),
            (
                81232,
                Some(99887766),
                Some("spam 4"),
                "2026-04-21T11:59:30Z",
            ),
            (
                81233,
                Some(99887766),
                Some("spam 5"),
                "2026-04-21T11:59:40Z",
            ),
            (81234, Some(42), Some("admin note"), "2026-04-21T12:05:00Z"),
        ] {
            storage
                .append_message_journal(&MessageJournalRecord {
                    chat_id: -100123,
                    message_id,
                    user_id,
                    date_utc: date_utc.to_owned(),
                    update_type: "message".to_owned(),
                    text: text.map(str::to_owned),
                    normalized_text: text.map(str::to_owned),
                    has_media: false,
                    reply_to_message_id: None,
                    file_ids_json: None,
                    meta_json: None,
                })
                .expect("seed message journal");
        }
    }

    fn seed_audit_entries(api: &HostApi) {
        let storage = api
            .storage(HostApiOperation::AuditFind)
            .expect("storage available");
        for entry in [
            AuditLogEntry {
                action_id: "act_1".to_owned(),
                trace_id: Some("trace-1".to_owned()),
                request_id: Some("req-1".to_owned()),
                unit_name: "moderation.test".to_owned(),
                execution_mode: "manual".to_owned(),
                op: "mute".to_owned(),
                actor_user_id: Some(42),
                chat_id: Some(-100123),
                target_kind: Some("user".to_owned()),
                target_id: Some("99887766".to_owned()),
                trigger_message_id: Some(81231),
                idempotency_key: Some("idem-1".to_owned()),
                reversible: true,
                compensation_json: Some(
                    "{\"kind\":\"host_op\",\"op\":\"tg.unrestrict\"}".to_owned(),
                ),
                args_json: "{\"duration\":\"7d\"}".to_owned(),
                result_json: Some("{\"ok\":true}".to_owned()),
                created_at: "2026-04-21T12:00:00Z".to_owned(),
            },
            AuditLogEntry {
                action_id: "act_2".to_owned(),
                trace_id: Some("trace-2".to_owned()),
                request_id: Some("req-2".to_owned()),
                unit_name: "moderation.test".to_owned(),
                execution_mode: "manual".to_owned(),
                op: "del".to_owned(),
                actor_user_id: Some(42),
                chat_id: Some(-100123),
                target_kind: Some("message".to_owned()),
                target_id: Some("81231".to_owned()),
                trigger_message_id: Some(81231),
                idempotency_key: Some("idem-2".to_owned()),
                reversible: false,
                compensation_json: None,
                args_json: "{\"count\":1}".to_owned(),
                result_json: Some("{\"deleted\":1}".to_owned()),
                created_at: "2026-04-21T12:01:00Z".to_owned(),
            },
        ] {
            storage
                .append_audit_entry(&entry)
                .expect("seed audit entry");
        }
    }

    #[test]
    fn ctx_current_returns_cloned_event_with_operation_metadata() {
        let event = manual_event();
        let api = HostApi::new(false);

        let response = api.ctx_current(&event).expect("ctx.current succeeds");

        assert_eq!(response.operation, HostApiOperation::CtxCurrent);
        assert!(!response.dry_run);
        assert_eq!(response.value.event.event_id, event.event_id);
        assert_eq!(response.value.event.execution_mode, ExecutionMode::Manual);
    }

    #[test]
    fn call_surface_routes_ctx_current_request() {
        let event = manual_event();
        let api = HostApi::new(false);

        let response = api
            .call(&event, HostApiRequest::CtxCurrent)
            .expect("typed call succeeds");

        assert_eq!(response.operation, HostApiOperation::CtxCurrent);
        assert!(!response.dry_run);
        match response.value {
            HostApiValue::CtxCurrent(value) => assert_eq!(value.event.event_id, event.event_id),
            other => panic!("unexpected host api value: {other:?}"),
        }
    }

    #[test]
    fn ctx_resolve_target_uses_parser_and_reply_fallback() {
        let event = manual_event();
        let api = HostApi::new(false);

        let explicit = api
            .ctx_resolve_target(
                &event,
                CtxResolveTargetRequest {
                    positional: Some("@spam_user".to_owned()),
                    selector_flag: None,
                    implicit: None,
                },
            )
            .expect("explicit target resolves");
        assert_eq!(explicit.value.source, TargetSource::ExplicitPositional);
        assert_eq!(
            explicit.value.selector,
            ParsedTargetSelector::Username {
                username: "spam_user".to_owned(),
            }
        );

        let reply = api
            .ctx_resolve_target(
                &event,
                CtxResolveTargetRequest {
                    positional: None,
                    selector_flag: None,
                    implicit: None,
                },
            )
            .expect("reply fallback resolves");
        assert_eq!(reply.value.source, TargetSource::ReplyContext);
        assert_eq!(
            reply.value.selector,
            ParsedTargetSelector::UserId { user_id: 77 }
        );
    }

    #[test]
    fn ctx_resolve_target_returns_structured_parse_error() {
        let event = manual_event();
        let api = HostApi::new(false);

        let error = api
            .ctx_resolve_target(
                &event,
                CtxResolveTargetRequest {
                    positional: Some("@bad-name".to_owned()),
                    selector_flag: None,
                    implicit: None,
                },
            )
            .expect_err("invalid target must fail");

        assert_eq!(error.kind, HostApiErrorKind::Parse);
        assert_eq!(error.operation, HostApiOperation::CtxResolveTarget);
        assert_eq!(
            error.detail,
            HostApiErrorDetail::InvalidTarget {
                value: "@bad-name".to_owned(),
                source: TargetParseError::InvalidUsername("@bad-name".to_owned()),
            }
        );
    }

    #[test]
    fn ctx_parse_duration_returns_typed_value() {
        let event = manual_event();
        let api = HostApi::new(false);

        let response = api
            .ctx_parse_duration(
                &event,
                CtxParseDurationRequest {
                    input: "15m".to_owned(),
                },
            )
            .expect("duration parses");

        assert_eq!(response.operation, HostApiOperation::CtxParseDuration);
        assert_eq!(
            response.value,
            ParsedDuration {
                value: 15,
                unit: DurationUnit::Minutes,
            }
        );
    }

    #[test]
    fn ctx_parse_duration_returns_structured_error() {
        let event = manual_event();
        let api = HostApi::new(false);

        let error = api
            .ctx_parse_duration(
                &event,
                CtxParseDurationRequest {
                    input: "30".to_owned(),
                },
            )
            .expect_err("missing unit must fail");

        assert_eq!(error.kind, HostApiErrorKind::Parse);
        assert_eq!(
            error.detail,
            HostApiErrorDetail::InvalidDuration {
                value: "30".to_owned(),
                source: DurationParseError::MissingUnit,
            }
        );
    }

    #[test]
    fn ctx_expand_reason_uses_alias_registry() {
        let event = manual_event();
        let mut aliases = ReasonAliasRegistry::new();
        aliases.insert(
            "spam",
            ReasonAliasDefinition::new("spam or scam promotion")
                .with_rule_code("2.8")
                .with_title("Spam"),
        );
        let api = HostApi::new(false).with_reason_aliases(aliases);

        let response = api
            .ctx_expand_reason(
                &event,
                CtxExpandReasonRequest {
                    reason: ReasonExpr::Alias("spam".to_owned()),
                },
            )
            .expect("reason expands");

        assert_eq!(response.operation, HostApiOperation::CtxExpandReason);
        assert_eq!(
            response.value,
            ExpandedReason::Alias {
                alias: "spam".to_owned(),
                definition: ReasonAliasDefinition {
                    canonical: "spam or scam promotion".to_owned(),
                    rule_code: Some("2.8".to_owned()),
                    title: Some("Spam".to_owned()),
                },
            }
        );
    }

    #[test]
    fn db_user_get_returns_typed_user_value() {
        let event = manual_event();
        let (_dir, api) = storage_api();
        api.storage(HostApiOperation::DbUserGet)
            .expect("storage")
            .upsert_user(&UserPatch {
                user_id: 77,
                username: Some("reply_user".to_owned()),
                display_name: Some("Reply User".to_owned()),
                seen_at: "2026-04-21T12:00:00Z".to_owned(),
                warn_count: Some(1),
                shadowbanned: Some(false),
                reputation: Some(4),
                state_json: Some("{\"state\":\"ok\"}".to_owned()),
                updated_at: "2026-04-21T12:00:00Z".to_owned(),
            })
            .expect("seed user");

        let response = api
            .db_user_get(&event, DbUserGetRequest { user_id: 77 })
            .expect("db.user_get succeeds");

        assert_eq!(response.operation, HostApiOperation::DbUserGet);
        assert_eq!(
            response
                .value
                .user
                .expect("user exists")
                .username
                .as_deref(),
            Some("reply_user")
        );
    }

    #[test]
    fn db_user_get_rejects_zero_user_id() {
        let event = manual_event();
        let (_dir, api) = storage_api();

        let error = api
            .db_user_get(&event, DbUserGetRequest { user_id: 0 })
            .expect_err("zero user id must fail");

        assert_eq!(error.kind, HostApiErrorKind::Validation);
        assert_eq!(error.operation, HostApiOperation::DbUserGet);
        assert_eq!(
            error.detail,
            HostApiErrorDetail::InvalidField {
                field: "user_id".to_owned(),
                message: "must be non-zero".to_owned(),
            }
        );
    }

    #[test]
    fn db_user_get_requires_storage_resource() {
        let event = manual_event();
        let api = HostApi::new(false);

        let error = api
            .db_user_get(&event, DbUserGetRequest { user_id: 77 })
            .expect_err("missing storage must fail");

        assert_eq!(error.kind, HostApiErrorKind::Internal);
        assert_eq!(error.operation, HostApiOperation::DbUserGet);
        assert_eq!(
            error.detail,
            HostApiErrorDetail::ResourceUnavailable {
                resource: "storage".to_owned(),
            }
        );
    }

    #[test]
    fn db_user_patch_persists_user_on_happy_path() {
        let event = manual_event();
        let (_dir, api) = storage_api();

        let response = api
            .db_user_patch(
                &event,
                DbUserPatchRequest {
                    patch: UserPatch {
                        user_id: 77,
                        username: Some("patched_user".to_owned()),
                        display_name: Some("Patched User".to_owned()),
                        seen_at: "2026-04-21T12:05:00Z".to_owned(),
                        warn_count: Some(2),
                        shadowbanned: Some(false),
                        reputation: Some(9),
                        state_json: Some("{\"state\":\"patched\"}".to_owned()),
                        updated_at: "2026-04-21T12:05:00Z".to_owned(),
                    },
                },
            )
            .expect("patch succeeds");

        assert!(!response.dry_run);
        assert_eq!(
            response.value.user.username.as_deref(),
            Some("patched_user")
        );
        assert_eq!(
            api.storage(HostApiOperation::DbUserPatch)
                .expect("storage")
                .get_user(77)
                .expect("query succeeds")
                .expect("user exists")
                .username
                .as_deref(),
            Some("patched_user")
        );
    }

    #[test]
    fn db_user_patch_dry_run_validates_without_mutation() {
        let event = manual_event();
        let (_dir, api) = dry_run_storage_api();

        let response = api
            .db_user_patch(
                &event,
                DbUserPatchRequest {
                    patch: UserPatch {
                        user_id: 77,
                        username: Some("dry_run_user".to_owned()),
                        display_name: Some("Dry Run".to_owned()),
                        seen_at: "2026-04-21T12:05:00Z".to_owned(),
                        warn_count: Some(2),
                        shadowbanned: Some(true),
                        reputation: Some(5),
                        state_json: Some("{\"mode\":\"dry\"}".to_owned()),
                        updated_at: "2026-04-21T12:05:00Z".to_owned(),
                    },
                },
            )
            .expect("dry-run patch succeeds");

        assert!(response.dry_run);
        assert_eq!(response.value.user.warn_count, 2);
        assert!(
            api.storage(HostApiOperation::DbUserPatch)
                .expect("storage")
                .get_user(77)
                .expect("query succeeds")
                .is_none()
        );
    }

    #[test]
    fn db_user_patch_returns_structured_validation_error() {
        let event = manual_event();
        let (_dir, api) = storage_api();

        let error = api
            .db_user_patch(
                &event,
                DbUserPatchRequest {
                    patch: UserPatch {
                        user_id: 0,
                        username: None,
                        display_name: None,
                        seen_at: "".to_owned(),
                        warn_count: Some(-1),
                        shadowbanned: None,
                        reputation: None,
                        state_json: None,
                        updated_at: "".to_owned(),
                    },
                },
            )
            .expect_err("invalid patch must fail");

        assert_eq!(error.kind, HostApiErrorKind::Validation);
        assert_eq!(error.operation, HostApiOperation::DbUserPatch);
        assert_eq!(
            error.detail,
            HostApiErrorDetail::InvalidField {
                field: "user_id".to_owned(),
                message: "must be non-zero".to_owned(),
            }
        );
    }

    #[test]
    fn db_user_incr_updates_existing_user() {
        let event = manual_event();
        let (_dir, api) = storage_api();
        api.storage(HostApiOperation::DbUserIncr)
            .expect("storage")
            .upsert_user(&UserPatch {
                user_id: 77,
                username: Some("reply_user".to_owned()),
                display_name: Some("Reply User".to_owned()),
                seen_at: "2026-04-21T12:00:00Z".to_owned(),
                warn_count: Some(1),
                shadowbanned: Some(false),
                reputation: Some(4),
                state_json: None,
                updated_at: "2026-04-21T12:00:00Z".to_owned(),
            })
            .expect("seed user");

        let response = api
            .db_user_incr(
                &event,
                DbUserIncrRequest {
                    user_id: 77,
                    username: None,
                    display_name: Some("Reply User Updated".to_owned()),
                    seen_at: "2026-04-21T12:10:00Z".to_owned(),
                    updated_at: "2026-04-21T12:10:00Z".to_owned(),
                    warn_count_delta: 2,
                    reputation_delta: -1,
                    shadowbanned: Some(true),
                    state_json: Some("{\"escalated\":true}".to_owned()),
                },
            )
            .expect("increment succeeds");

        assert_eq!(response.value.user.warn_count, 3);
        assert_eq!(response.value.user.reputation, 3);
        assert!(response.value.user.shadowbanned);
        assert_eq!(
            api.storage(HostApiOperation::DbUserIncr)
                .expect("storage")
                .get_user(77)
                .expect("query succeeds")
                .expect("user exists")
                .warn_count,
            3
        );
    }

    #[test]
    fn db_user_incr_returns_structured_counter_error() {
        let event = manual_event();
        let (_dir, api) = storage_api();

        let error = api
            .db_user_incr(
                &event,
                DbUserIncrRequest {
                    user_id: 77,
                    username: None,
                    display_name: None,
                    seen_at: "2026-04-21T12:10:00Z".to_owned(),
                    updated_at: "2026-04-21T12:10:00Z".to_owned(),
                    warn_count_delta: -1,
                    reputation_delta: 0,
                    shadowbanned: None,
                    state_json: None,
                },
            )
            .expect_err("negative increment from zero must fail");

        assert_eq!(error.kind, HostApiErrorKind::Validation);
        assert_eq!(
            error.detail,
            HostApiErrorDetail::InvalidCounterChange {
                field: "warn_count".to_owned(),
                current: 0,
                delta: -1,
            }
        );
    }

    #[test]
    fn db_user_incr_dry_run_does_not_mutate_storage() {
        let event = manual_event();
        let (_dir, api) = dry_run_storage_api();

        let response = api
            .db_user_incr(
                &event,
                DbUserIncrRequest {
                    user_id: 77,
                    username: Some("dry_increment".to_owned()),
                    display_name: Some("Dry Increment".to_owned()),
                    seen_at: "2026-04-21T12:10:00Z".to_owned(),
                    updated_at: "2026-04-21T12:10:00Z".to_owned(),
                    warn_count_delta: 2,
                    reputation_delta: 4,
                    shadowbanned: Some(false),
                    state_json: Some("{\"dry\":true}".to_owned()),
                },
            )
            .expect("dry-run increment succeeds");

        assert!(response.dry_run);
        assert_eq!(response.value.user.warn_count, 2);
        assert!(
            api.storage(HostApiOperation::DbUserIncr)
                .expect("storage")
                .get_user(77)
                .expect("query succeeds")
                .is_none()
        );
    }

    #[test]
    fn db_kv_set_dry_run_does_not_mutate_storage() {
        let event = manual_event();
        let (_dir, api) = dry_run_storage_api();

        let response = api
            .db_kv_set(
                &event,
                DbKvSetRequest {
                    entry: KvEntry {
                        scope_kind: "chat".to_owned(),
                        scope_id: "-100123".to_owned(),
                        key: "policy".to_owned(),
                        value_json: "{\"mode\":\"strict\"}".to_owned(),
                        updated_at: "2026-04-21T12:00:00Z".to_owned(),
                    },
                },
            )
            .expect("dry-run kv set succeeds");

        assert!(response.dry_run);
        assert_eq!(response.value.entry.key, "policy");
        assert!(
            api.storage(HostApiOperation::DbKvSet)
                .expect("storage")
                .get_kv("chat", "-100123", "policy")
                .expect("query succeeds")
                .is_none()
        );
    }

    #[test]
    fn db_kv_get_returns_seeded_entry() {
        let event = manual_event();
        let (_dir, api) = storage_api();
        api.storage(HostApiOperation::DbKvGet)
            .expect("storage")
            .set_kv(&KvEntry {
                scope_kind: "chat".to_owned(),
                scope_id: "-100123".to_owned(),
                key: "policy".to_owned(),
                value_json: "{\"mode\":\"strict\"}".to_owned(),
                updated_at: "2026-04-21T12:00:00Z".to_owned(),
            })
            .expect("seed kv");

        let response = api
            .db_kv_get(
                &event,
                DbKvGetRequest {
                    scope_kind: "chat".to_owned(),
                    scope_id: "-100123".to_owned(),
                    key: "policy".to_owned(),
                },
            )
            .expect("kv get succeeds");

        assert_eq!(
            response.value.entry.expect("entry exists").value_json,
            "{\"mode\":\"strict\"}"
        );
    }

    #[test]
    fn db_kv_get_rejects_blank_key() {
        let event = manual_event();
        let (_dir, api) = storage_api();

        let error = api
            .db_kv_get(
                &event,
                DbKvGetRequest {
                    scope_kind: "chat".to_owned(),
                    scope_id: "-100123".to_owned(),
                    key: "   ".to_owned(),
                },
            )
            .expect_err("blank key must fail");

        assert_eq!(error.kind, HostApiErrorKind::Validation);
        assert_eq!(error.operation, HostApiOperation::DbKvGet);
        assert_eq!(
            error.detail,
            HostApiErrorDetail::InvalidField {
                field: "key".to_owned(),
                message: "must not be blank".to_owned(),
            }
        );
    }

    #[test]
    fn db_kv_set_persists_entry_on_happy_path() {
        let event = manual_event();
        let (_dir, api) = storage_api();

        let response = api
            .db_kv_set(
                &event,
                DbKvSetRequest {
                    entry: KvEntry {
                        scope_kind: "chat".to_owned(),
                        scope_id: "-100123".to_owned(),
                        key: "policy".to_owned(),
                        value_json: "{\"mode\":\"strict\"}".to_owned(),
                        updated_at: "2026-04-21T12:00:00Z".to_owned(),
                    },
                },
            )
            .expect("kv set succeeds");

        assert!(!response.dry_run);
        assert_eq!(
            api.storage(HostApiOperation::DbKvSet)
                .expect("storage")
                .get_kv("chat", "-100123", "policy")
                .expect("query succeeds")
                .expect("entry exists")
                .value_json,
            "{\"mode\":\"strict\"}"
        );
    }

    #[test]
    fn msg_window_returns_anchor_window() {
        let event = manual_event();
        let (_dir, api) = storage_api_with_registry(&["msg.history.read"], &[], false);
        seed_message_journal(&api);

        let response = api
            .msg_window(
                &event,
                MsgWindowRequest {
                    chat_id: -100123,
                    anchor_message_id: 81231,
                    up: 2,
                    down: 2,
                    include_anchor: true,
                },
            )
            .expect("msg window succeeds");

        assert_eq!(response.operation, HostApiOperation::MsgWindow);
        assert_eq!(response.value.messages.len(), 5);
        assert_eq!(response.value.messages[2].message_id, 81231);
    }

    #[test]
    fn msg_window_rejects_oversized_request() {
        let event = manual_event();
        let (_dir, api) = storage_api_with_registry(&["msg.history.read"], &[], false);

        let error = api
            .msg_window(
                &event,
                MsgWindowRequest {
                    chat_id: -100123,
                    anchor_message_id: 81231,
                    up: 200,
                    down: 1,
                    include_anchor: true,
                },
            )
            .expect_err("oversized msg window must fail");

        assert_eq!(error.kind, HostApiErrorKind::Validation);
        assert_eq!(
            error.detail,
            HostApiErrorDetail::MessageWindowTooLarge {
                requested: 202,
                max: 200,
            }
        );
    }

    #[test]
    fn msg_window_denies_when_capability_is_missing() {
        let event = manual_event();
        let (_dir, api) = storage_api_with_registry(&["audit.read"], &[], false);

        let error = api
            .msg_window(
                &event,
                MsgWindowRequest {
                    chat_id: -100123,
                    anchor_message_id: 81231,
                    up: 1,
                    down: 1,
                    include_anchor: true,
                },
            )
            .expect_err("missing capability must fail");

        assert_eq!(error.kind, HostApiErrorKind::Denied);
        assert_eq!(error.operation, HostApiOperation::MsgWindow);
        assert_eq!(
            error.detail,
            HostApiErrorDetail::CapabilityDenied {
                capability: "msg.history.read".to_owned(),
                unit_id: "moderation.test".to_owned(),
            }
        );
    }

    #[test]
    fn msg_window_fails_closed_when_unit_registry_is_unavailable() {
        let event = manual_event();
        let (_dir, api) = storage_api();

        let error = api
            .msg_window(
                &event,
                MsgWindowRequest {
                    chat_id: -100123,
                    anchor_message_id: 81231,
                    up: 1,
                    down: 1,
                    include_anchor: true,
                },
            )
            .expect_err("missing registry must fail closed");

        assert_eq!(error.kind, HostApiErrorKind::Internal);
        assert_eq!(error.operation, HostApiOperation::MsgWindow);
        assert_eq!(
            error.detail,
            HostApiErrorDetail::ResourceUnavailable {
                resource: "unit_registry".to_owned(),
            }
        );
    }

    #[test]
    fn msg_window_preserves_dry_run_metadata_for_reads() {
        let event = manual_event();
        let (_dir, api) = storage_api_with_registry(&["msg.history.read"], &[], true);
        seed_message_journal(&api);

        let response = api
            .msg_window(
                &event,
                MsgWindowRequest {
                    chat_id: -100123,
                    anchor_message_id: 81231,
                    up: 1,
                    down: 1,
                    include_anchor: true,
                },
            )
            .expect("msg window succeeds");

        assert!(response.dry_run);
        assert_eq!(response.value.messages.len(), 3);
    }

    #[test]
    fn msg_by_user_returns_recent_messages_for_user() {
        let event = manual_event();
        let (_dir, api) = storage_api_with_registry(&["msg.history.read"], &[], false);
        seed_message_journal(&api);

        let response = api
            .msg_by_user(
                &event,
                MsgByUserRequest {
                    chat_id: -100123,
                    user_id: 99887766,
                    since: "2026-04-21T11:59:05Z".to_owned(),
                    limit: 3,
                },
            )
            .expect("msg.by_user succeeds");

        assert_eq!(response.operation, HostApiOperation::MsgByUser);
        assert_eq!(response.value.messages.len(), 3);
        assert_eq!(response.value.messages[0].message_id, 81233);
    }

    #[test]
    fn msg_by_user_rejects_invalid_since_timestamp() {
        let event = manual_event();
        let (_dir, api) = storage_api_with_registry(&["msg.history.read"], &[], false);

        let error = api
            .msg_by_user(
                &event,
                MsgByUserRequest {
                    chat_id: -100123,
                    user_id: 99887766,
                    since: "yesterday".to_owned(),
                    limit: 3,
                },
            )
            .expect_err("invalid since must fail");

        assert_eq!(error.kind, HostApiErrorKind::Validation);
        assert_eq!(error.operation, HostApiOperation::MsgByUser);
        assert!(
            matches!(
                error.detail,
                HostApiErrorDetail::InvalidField { ref field, .. } if field == "since"
            ),
            "unexpected error detail: {:?}",
            error.detail
        );
    }

    #[test]
    fn msg_by_user_denies_when_capability_is_missing() {
        let event = manual_event();
        let (_dir, api) = storage_api_with_registry(&["audit.read"], &[], false);

        let error = api
            .msg_by_user(
                &event,
                MsgByUserRequest {
                    chat_id: -100123,
                    user_id: 99887766,
                    since: "2026-04-21T11:59:05Z".to_owned(),
                    limit: 3,
                },
            )
            .expect_err("missing capability must fail");

        assert_eq!(error.kind, HostApiErrorKind::Denied);
        assert_eq!(
            error.detail,
            HostApiErrorDetail::CapabilityDenied {
                capability: "msg.history.read".to_owned(),
                unit_id: "moderation.test".to_owned(),
            }
        );
    }

    #[test]
    fn msg_by_user_preserves_dry_run_metadata_for_reads() {
        let event = manual_event();
        let (_dir, api) = storage_api_with_registry(&["msg.history.read"], &[], true);
        seed_message_journal(&api);

        let response = api
            .msg_by_user(
                &event,
                MsgByUserRequest {
                    chat_id: -100123,
                    user_id: 99887766,
                    since: "2026-04-21T11:59:05Z".to_owned(),
                    limit: 2,
                },
            )
            .expect("msg.by_user succeeds");

        assert!(response.dry_run);
        assert_eq!(response.value.messages.len(), 2);
    }

    #[test]
    fn job_schedule_after_dry_run_validates_without_mutation() {
        let event = manual_event();
        let (_dir, api) = storage_api_with_registry(&["job.schedule"], &[], true);

        let response = api
            .job_schedule_after(
                &event,
                JobScheduleAfterRequest {
                    delay: "7d".to_owned(),
                    executor_unit: "moderation.mute_release".to_owned(),
                    payload: json!({"kind":"host_op","op":"tg.send_ui"}),
                    dedupe_key: Some("mute:99887766".to_owned()),
                    max_retries: Some(2),
                    audit_action_id: Some("act_1".to_owned()),
                },
            )
            .expect("dry-run schedule succeeds");

        assert!(response.dry_run);
        assert_eq!(response.value.job.status, "scheduled");
        assert!(
            api.storage(HostApiOperation::JobScheduleAfter)
                .expect("storage")
                .get_job(&response.value.job.job_id)
                .expect("job lookup succeeds")
                .is_none()
        );
    }

    #[test]
    fn job_schedule_after_rejects_too_distant_delay() {
        let event = manual_event();
        let (_dir, api) = storage_api_with_registry(&["job.schedule"], &[], false);

        let error = api
            .job_schedule_after(
                &event,
                JobScheduleAfterRequest {
                    delay: "53w".to_owned(),
                    executor_unit: "moderation.mute_release".to_owned(),
                    payload: json!({"kind":"host_op"}),
                    dedupe_key: None,
                    max_retries: None,
                    audit_action_id: None,
                },
            )
            .expect_err("delay beyond 365 days must fail");

        assert_eq!(error.kind, HostApiErrorKind::Validation);
        assert_eq!(
            error.detail,
            HostApiErrorDetail::JobTooFarInFuture {
                delay: "53w".to_owned(),
                max_days: 365,
            }
        );
    }

    #[test]
    fn job_schedule_after_persists_job_on_happy_path() {
        let event = manual_event();
        let (_dir, api) = storage_api_with_registry(&["job.schedule"], &[], false);

        let response = api
            .job_schedule_after(
                &event,
                JobScheduleAfterRequest {
                    delay: "2h".to_owned(),
                    executor_unit: "moderation.mute_release".to_owned(),
                    payload: json!({"kind":"host_op","op":"tg.send_ui"}),
                    dedupe_key: Some("mute:99887766".to_owned()),
                    max_retries: Some(2),
                    audit_action_id: Some("act_1".to_owned()),
                },
            )
            .expect("job schedule succeeds");

        assert!(!response.dry_run);
        assert_eq!(response.value.job.executor_unit, "moderation.mute_release");
        assert!(
            api.storage(HostApiOperation::JobScheduleAfter)
                .expect("storage")
                .get_job(&response.value.job.job_id)
                .expect("lookup succeeds")
                .is_some()
        );
    }

    #[test]
    fn job_schedule_after_denies_when_capability_is_missing() {
        let event = manual_event();
        let (_dir, api) = storage_api_with_registry(&["audit.read"], &[], false);

        let error = api
            .job_schedule_after(
                &event,
                JobScheduleAfterRequest {
                    delay: "2h".to_owned(),
                    executor_unit: "moderation.mute_release".to_owned(),
                    payload: json!({"kind":"host_op"}),
                    dedupe_key: None,
                    max_retries: None,
                    audit_action_id: None,
                },
            )
            .expect_err("missing capability must fail");

        assert_eq!(error.kind, HostApiErrorKind::Denied);
        assert_eq!(
            error.detail,
            HostApiErrorDetail::CapabilityDenied {
                capability: "job.schedule".to_owned(),
                unit_id: "moderation.test".to_owned(),
            }
        );
    }

    #[test]
    fn audit_find_returns_matching_entries() {
        let event = manual_event();
        let (_dir, api) = storage_api_with_registry(&["audit.read"], &[], false);
        seed_audit_entries(&api);

        let response = api
            .audit_find(
                &event,
                AuditFindRequest {
                    filters: AuditLogFilter {
                        trigger_message_id: Some(81231),
                        ..AuditLogFilter::default()
                    },
                    limit: 10,
                },
            )
            .expect("audit.find succeeds");

        assert_eq!(response.operation, HostApiOperation::AuditFind);
        assert_eq!(response.value.entries.len(), 2);
        assert_eq!(response.value.entries[0].action_id, "act_2");
    }

    #[test]
    fn audit_find_requires_at_least_one_filter() {
        let event = manual_event();
        let (_dir, api) = storage_api_with_registry(&["audit.read"], &[], false);

        let error = api
            .audit_find(
                &event,
                AuditFindRequest {
                    filters: AuditLogFilter::default(),
                    limit: 10,
                },
            )
            .expect_err("audit.find without filters must fail");

        assert_eq!(error.kind, HostApiErrorKind::Validation);
        assert_eq!(error.detail, HostApiErrorDetail::MissingAuditFilter);
    }

    #[test]
    fn audit_find_denies_when_capability_is_missing() {
        let event = manual_event();
        let (_dir, api) = storage_api_with_registry(&["job.schedule"], &[], false);

        let error = api
            .audit_find(
                &event,
                AuditFindRequest {
                    filters: AuditLogFilter {
                        trace_id: Some("trace-1".to_owned()),
                        ..AuditLogFilter::default()
                    },
                    limit: 10,
                },
            )
            .expect_err("missing capability must fail");

        assert_eq!(error.kind, HostApiErrorKind::Denied);
        assert_eq!(error.operation, HostApiOperation::AuditFind);
        assert_eq!(
            error.detail,
            HostApiErrorDetail::CapabilityDenied {
                capability: "audit.read".to_owned(),
                unit_id: "moderation.test".to_owned(),
            }
        );
    }

    #[test]
    fn audit_find_preserves_dry_run_metadata_for_reads() {
        let event = manual_event();
        let (_dir, api) = storage_api_with_registry(&["audit.read"], &[], true);
        seed_audit_entries(&api);

        let response = api
            .audit_find(
                &event,
                AuditFindRequest {
                    filters: AuditLogFilter {
                        trigger_message_id: Some(81231),
                        ..AuditLogFilter::default()
                    },
                    limit: 10,
                },
            )
            .expect("audit.find succeeds");

        assert!(response.dry_run);
        assert_eq!(response.value.entries.len(), 2);
    }

    #[test]
    fn audit_compensate_appends_compensation_entry() {
        let event = manual_event();
        let (_dir, api) =
            storage_api_with_registry(&["audit.compensate", "audit.read"], &[], false);
        seed_audit_entries(&api);

        let response = api
            .audit_compensate(
                &event,
                AuditCompensateRequest {
                    action_id: "act_1".to_owned(),
                },
            )
            .expect("audit.compensate succeeds");

        assert!(response.value.compensated);
        let new_action_id = response
            .value
            .new_action_id
            .clone()
            .expect("new action id returned");
        let inserted = api
            .storage(HostApiOperation::AuditCompensate)
            .expect("storage")
            .get_audit_entry(&new_action_id)
            .expect("lookup succeeds")
            .expect("compensation entry exists");
        assert_eq!(inserted.op, "audit.compensate");
        assert_eq!(inserted.target_id.as_deref(), Some("act_1"));
    }

    #[test]
    fn audit_compensate_dry_run_does_not_append_entry() {
        let event = manual_event();
        let (_dir, api) = storage_api_with_registry(&["audit.compensate", "audit.read"], &[], true);
        seed_audit_entries(&api);

        let response = api
            .audit_compensate(
                &event,
                AuditCompensateRequest {
                    action_id: "act_1".to_owned(),
                },
            )
            .expect("dry-run compensate succeeds");

        assert!(response.dry_run);
        let new_action_id = response
            .value
            .new_action_id
            .clone()
            .expect("predicted action id returned");
        assert!(
            api.storage(HostApiOperation::AuditCompensate)
                .expect("storage")
                .get_audit_entry(&new_action_id)
                .expect("lookup succeeds")
                .is_none()
        );
    }

    #[test]
    fn audit_compensate_rejects_already_compensated_action() {
        let event = manual_event();
        let (_dir, api) =
            storage_api_with_registry(&["audit.compensate", "audit.read"], &[], false);
        seed_audit_entries(&api);

        let first = api
            .audit_compensate(
                &event,
                AuditCompensateRequest {
                    action_id: "act_1".to_owned(),
                },
            )
            .expect("first compensation succeeds");
        assert!(first.value.compensated);

        let error = api
            .audit_compensate(
                &event,
                AuditCompensateRequest {
                    action_id: "act_1".to_owned(),
                },
            )
            .expect_err("second compensation must fail");

        assert_eq!(error.kind, HostApiErrorKind::Validation);
        assert_eq!(
            error.detail,
            HostApiErrorDetail::InvalidField {
                field: "action_id".to_owned(),
                message: "audit action `act_1` is already compensated".to_owned(),
            }
        );

        let compensations = api
            .storage(HostApiOperation::AuditCompensate)
            .expect("storage")
            .find_audit_by_idempotency_key("compensate:act_1")
            .expect("lookup succeeds");
        assert_eq!(compensations.len(), 1);
    }

    #[test]
    fn audit_compensate_rejects_non_reversible_action() {
        let event = manual_event();
        let (_dir, api) =
            storage_api_with_registry(&["audit.compensate", "audit.read"], &[], false);
        seed_audit_entries(&api);

        let error = api
            .audit_compensate(
                &event,
                AuditCompensateRequest {
                    action_id: "act_2".to_owned(),
                },
            )
            .expect_err("non-reversible action must fail");

        assert_eq!(error.kind, HostApiErrorKind::Validation);
        assert_eq!(
            error.detail,
            HostApiErrorDetail::InvalidField {
                field: "action_id".to_owned(),
                message: "audit action `act_2` is not reversible".to_owned(),
            }
        );
    }

    #[test]
    fn audit_compensate_rejects_invalid_compensation_recipe() {
        let event = manual_event();
        let (_dir, api) =
            storage_api_with_registry(&["audit.compensate", "audit.read"], &[], false);
        seed_audit_entries(&api);
        api.storage(HostApiOperation::AuditCompensate)
            .expect("storage")
            .append_audit_entry(&AuditLogEntry {
                action_id: "act_invalid_recipe".to_owned(),
                trace_id: Some("trace-invalid".to_owned()),
                request_id: None,
                unit_name: "moderation.test".to_owned(),
                execution_mode: "manual".to_owned(),
                op: "mute".to_owned(),
                actor_user_id: Some(42),
                chat_id: Some(-100123),
                target_kind: Some("user".to_owned()),
                target_id: Some("99887766".to_owned()),
                trigger_message_id: Some(81231),
                idempotency_key: Some("idem-invalid".to_owned()),
                reversible: true,
                compensation_json: Some("{not-json}".to_owned()),
                args_json: "{\"duration\":\"7d\"}".to_owned(),
                result_json: Some("{\"ok\":true}".to_owned()),
                created_at: "2026-04-21T12:02:00Z".to_owned(),
            })
            .expect("invalid recipe audit entry");

        let error = api
            .audit_compensate(
                &event,
                AuditCompensateRequest {
                    action_id: "act_invalid_recipe".to_owned(),
                },
            )
            .expect_err("invalid recipe must fail");

        assert_eq!(error.kind, HostApiErrorKind::Validation);
        assert!(matches!(
            error.detail,
            HostApiErrorDetail::InvalidField { ref field, ref message }
                if field == "compensation_json"
                    && message.contains("invalid compensation recipe")
        ));

        let compensations = api
            .storage(HostApiOperation::AuditCompensate)
            .expect("storage")
            .find_audit_by_idempotency_key("compensate:act_invalid_recipe")
            .expect("lookup succeeds");
        assert!(compensations.is_empty());
    }

    #[test]
    fn capability_denial_uses_structured_error_surface() {
        let event = manual_event();
        let (_dir, api) = storage_api_with_registry(&[], &["audit.compensate"], false);
        seed_audit_entries(&api);

        let error = api
            .audit_compensate(
                &event,
                AuditCompensateRequest {
                    action_id: "act_1".to_owned(),
                },
            )
            .expect_err("denied capability must fail");

        assert_eq!(error.kind, HostApiErrorKind::Denied);
        assert_eq!(
            error.detail,
            HostApiErrorDetail::CapabilityDenied {
                capability: "audit.compensate".to_owned(),
                unit_id: "moderation.test".to_owned(),
            }
        );
    }

    #[test]
    fn audit_compensate_returns_structured_unknown_action_error() {
        let event = manual_event();
        let (_dir, api) = storage_api_with_registry(&["audit.compensate"], &[], false);

        let error = api
            .audit_compensate(
                &event,
                AuditCompensateRequest {
                    action_id: "missing".to_owned(),
                },
            )
            .expect_err("unknown action must fail");

        assert_eq!(error.kind, HostApiErrorKind::Validation);
        assert_eq!(error.operation, HostApiOperation::AuditCompensate);
        assert_eq!(
            error.detail,
            HostApiErrorDetail::UnknownAuditAction {
                action_id: "missing".to_owned(),
            }
        );
    }

    #[test]
    fn unit_status_returns_summary_and_specific_entry() {
        let event = manual_event();
        let api = unit_registry_api();

        let response = api
            .unit_status(
                &event,
                UnitStatusRequest {
                    unit_id: Some("moderation.warn".to_owned()),
                },
            )
            .expect("unit status succeeds");

        assert_eq!(response.operation, HostApiOperation::UnitStatus);
        assert_eq!(response.value.summary.total_units, 2);
        assert_eq!(response.value.summary.active_units, 1);
        assert_eq!(response.value.summary.disabled_units, 1);
        assert_eq!(
            response.value.unit,
            Some(UnitStatusEntry {
                unit_id: "moderation.warn".to_owned(),
                status: UnitStatus::Active,
                enabled: Some(true),
                diagnostics: Vec::new(),
            })
        );
    }

    #[test]
    fn unit_status_returns_structured_not_found_error() {
        let event = manual_event();
        let api = unit_registry_api();

        let error = api
            .unit_status(
                &event,
                UnitStatusRequest {
                    unit_id: Some("missing.unit".to_owned()),
                },
            )
            .expect_err("unknown unit must fail");

        assert_eq!(error.kind, HostApiErrorKind::Validation);
        assert_eq!(
            error.detail,
            HostApiErrorDetail::UnknownUnit {
                unit_id: "missing.unit".to_owned(),
            }
        );
    }

    #[test]
    fn unit_status_preserves_dry_run_metadata() {
        let active = UnitManifest::new(
            UnitDefinition::new("moderation.warn"),
            TriggerSpec::command(["warn"]),
            ServiceSpec::new("cargo run"),
        );
        let report = UnitRegistry::load_manifests(vec![active]);
        let api = HostApi::new(true).with_unit_registry(report.registry);
        let event = manual_event();

        let response = api
            .unit_status(&event, UnitStatusRequest { unit_id: None })
            .expect("unit status succeeds");

        assert!(response.dry_run);
        assert_eq!(response.value.summary.total_units, 1);
    }

    #[test]
    fn call_surface_routes_db_and_unit_requests() {
        let event = manual_event();
        let api = unit_registry_api();

        let response = api
            .call(
                &event,
                HostApiRequest::UnitStatus(UnitStatusRequest { unit_id: None }),
            )
            .expect("typed call succeeds");

        match response.value {
            HostApiValue::UnitStatus(value) => assert_eq!(value.summary.total_units, 2),
            other => panic!("unexpected host api value: {other:?}"),
        }
    }

    #[test]
    fn dry_run_is_preserved_in_ctx_responses() {
        let event = manual_event();
        let api = HostApi::new(true);

        let response = api
            .ctx_parse_duration(
                &event,
                CtxParseDurationRequest {
                    input: "1h".to_owned(),
                },
            )
            .expect("ctx op still succeeds in dry run");

        assert!(response.dry_run);
        assert_eq!(response.operation, HostApiOperation::CtxParseDuration);
    }

    #[test]
    fn invalid_event_maps_to_validation_error() {
        let mut event = EventContext::new(
            "evt_invalid",
            UpdateType::Message,
            ExecutionMode::Realtime,
            SystemContext::synthetic(SystemOrigin::Manual),
        );
        event.message = None;

        let api = HostApi::new(false);
        let error = api
            .ctx_current(&event)
            .expect_err("invalid event must fail");

        assert_eq!(error.kind, HostApiErrorKind::Validation);
        assert_eq!(error.operation, HostApiOperation::CtxCurrent);
        assert!(
            matches!(
                error,
                HostApiError {
                    detail: HostApiErrorDetail::InvalidEventContext { .. },
                    ..
                }
            ),
            "unexpected error shape: {error:?}"
        );
    }
