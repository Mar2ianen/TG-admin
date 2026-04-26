use crate::audit::AuditService;
use crate::config::AppConfig;
use crate::host_api::HostApi;
use crate::host_api::MlServerTransport;
use crate::ingress::IngressPipeline;
use crate::moderation::ModerationEngine;
use crate::router::ExecutionRouter;
use crate::scheduler::Scheduler;
use crate::script::ScriptRunner;
use crate::shutdown::{ShutdownController, ShutdownReason};
use crate::storage::JobRecord;
use crate::storage::Storage;
use crate::storage::StorageConnection;
use crate::tg::{
    ParseMode, TelegramExecutionOptions, TelegramGateway, TelegramRequest,
    TelegramSendMessageRequest, TeloxideCoreTransport,
};
use crate::unit::{UnitRegistry, UnitRegistryStatus};
use anyhow::{Context, Result};
use std::fs;
use std::path::Path;
use std::rc::Rc;
use tokio::time;

// Runtime execution model: single-threaded by design.
//
// `StorageConnection` wraps `rusqlite::Connection` which is `!Send`. As a result,
// the entire execution graph — `StorageConnection` → `ModerationEngine` →
// `ExecutionRouter` → `RuntimeExecution` — is also `!Send`. Reference-counting
// uses `Rc<>` instead of `Arc<>` to reflect this: there is no cross-thread sharing.
//
// The main async runtime uses `flavor = "current_thread"` (see `main.rs`), which
// drives all futures on a single OS thread. This is intentional and sufficient for
// a Telegram polling bot: the throughput bottleneck is the Telegram API rate limit,
// not CPU parallelism.

#[derive(Debug)]
pub struct Runtime {
    registry: UnitRegistry,
    services: RuntimeServices,
    execution: RuntimeExecution,
}

impl Runtime {
    pub fn from_config(config: &AppConfig) -> Result<Self> {
        let registry = load_registry_from_units_dir(config)?;
        Ok(Self {
            registry,
            services: RuntimeServices::from_config(config)?,
            execution: RuntimeExecution::default(),
        })
    }

    pub async fn startup(&mut self, config: &AppConfig) -> Result<RuntimeBootstrapInfo> {
        let storage_bootstrap = self
            .services
            .storage
            .bootstrap()
            .context("failed to bootstrap storage during runtime startup")?;

        let schema_version = storage_bootstrap.migration().current_version;
        let moderation_storage = storage_bootstrap.into_connection();
        let host_api_storage = self
            .services
            .storage
            .init()
            .context("failed to open host api storage during runtime startup")?;
        let ingress_storage = self
            .services
            .storage
            .init()
            .context("failed to open ingress storage during runtime startup")?;
        let script_storage = self
            .services
            .storage
            .init()
            .context("failed to open script storage during runtime startup")?;

        self.execution = self.compose_execution(
            config,
            moderation_storage,
            host_api_storage,
            ingress_storage,
            script_storage,
        );

        Ok(RuntimeBootstrapInfo { schema_version })
    }

    fn compose_execution(
        &self,
        config: &AppConfig,
        moderation_storage: StorageConnection,
        host_api_storage: StorageConnection,
        ingress_storage: StorageConnection,
        script_storage: StorageConnection,
    ) -> RuntimeExecution {
        let registry_handle = Rc::new(self.registry.clone());
        let host_api = HostApi::new(false)
            .with_storage(host_api_storage)
            .with_unit_registry_handle(registry_handle.clone())
            .with_ml_server_transport(self.services.ml_server_transport.clone());
        let moderation = ModerationEngine::new(moderation_storage, self.services.telegram.clone())
            .with_unit_registry_handle(registry_handle.clone())
            .with_admin_user_ids(config.telegram.admin_user_ids.iter().copied())
            .without_processed_update_guard();
        let script_host_api = HostApi::new(false)
            .with_storage(script_storage)
            .with_unit_registry_handle(registry_handle.clone())
            .with_ml_server_transport(self.services.ml_server_transport.clone());
        let script_runner = ScriptRunner::new(config.paths.scripts_dir.clone());
        let router = Rc::new(
            ExecutionRouter::new()
                .with_registry_handle(registry_handle.clone())
                .with_moderation(moderation)
                .with_script_runner(script_runner, script_host_api),
        );
        let ingress = self.services.polling_bot().map(|bot| {
            IngressPipeline::new(bot, ingress_storage, router.clone())
                .with_admin_user_ids(config.telegram.admin_user_ids.iter().copied())
        });

        RuntimeExecution {
            host_api: Some(host_api),
            router: Some(router),
            ingress,
        }
    }

    pub fn summary(&self) -> RuntimeSummary<'_> {
        let index_stats = self
            .execution
            .router
            .as_ref()
            .map(|router| router.index_stats())
            .unwrap_or_default();
        RuntimeSummary {
            database_path: self.services.storage.database_path(),
            registry: self.registry.status_summary(),
            polling: self.services.telegram.polling(),
            scheduler_tick_ms: self.services.scheduler.tick_interval_ms(),
            audit_enabled: self.services.audit.enabled(),
            host_api_dry_run: self
                .execution
                .host_api
                .as_ref()
                .map(HostApi::dry_run)
                .unwrap_or(false),
            transport_name: self.services.telegram.transport_name(),
            router_ready: self.execution.router.is_some(),
            indexed_trait_routes: index_stats.trait_routes,
            indexed_command_routes: index_stats.command_routes,
        }
    }

    pub async fn shutdown(&mut self) -> Result<()> {
        self.execution = RuntimeExecution::default();
        tokio::task::yield_now().await;
        Ok(())
    }

    pub async fn run_until_shutdown(&self, shutdown: ShutdownController) -> Result<ShutdownReason> {
        tokio::select! {
            result = self.run_ingress_or_wait(shutdown) => result,
            _ = self.run_scheduler_loop() => {
                anyhow::bail!("scheduler loop terminated unexpectedly")
            }
        }
    }

    async fn run_ingress_or_wait(&self, shutdown: ShutdownController) -> Result<ShutdownReason> {
        match self.execution.ingress.as_ref() {
            Some(ingress) => ingress.run_until_shutdown(shutdown).await,
            None => shutdown.wait().await,
        }
    }

    async fn run_scheduler_loop(&self) {
        // If no router is ready (e.g. before startup), park until cancelled.
        if self.execution.router.is_none() {
            std::future::pending::<()>().await;
            return;
        }

        let tick = std::time::Duration::from_millis(self.services.scheduler.tick_interval_ms());
        let mut interval = time::interval(tick);
        interval.set_missed_tick_behavior(time::MissedTickBehavior::Skip);

        loop {
            interval.tick().await;
            if let Err(err) = self.tick_scheduler().await {
                tracing::warn!(error = %err, "scheduler tick error");
            }
        }
    }

    async fn tick_scheduler(&self) -> Result<()> {
        let storage = self
            .services
            .storage
            .init()
            .context("scheduler: failed to open storage connection")?;

        let now = chrono::Utc::now().to_rfc3339();
        let limit = self.services.scheduler.max_concurrent_jobs();
        let due_jobs = storage
            .poll_due_jobs(&now, limit)
            .context("scheduler: failed to poll due jobs")?;

        for job in &due_jobs {
            let claimed_at = chrono::Utc::now().to_rfc3339();
            if let Err(err) =
                storage.update_job_status(&job.job_id, "processing", None, &claimed_at)
            {
                tracing::warn!(job_id = %job.job_id, error = %err, "scheduler: failed to claim job");
                continue;
            }

            let result = self.execute_scheduled_job(job).await;
            let done_at = chrono::Utc::now().to_rfc3339();
            match result {
                Ok(()) => {
                    tracing::debug!(
                        job_id = %job.job_id,
                        executor = %job.executor_unit,
                        "scheduler: job completed"
                    );
                    if let Err(err) =
                        storage.update_job_status(&job.job_id, "completed", None, &done_at)
                    {
                        tracing::warn!(
                            job_id = %job.job_id,
                            error = %err,
                            "scheduler: failed to mark job completed"
                        );
                    }
                }
                Err(err) => {
                    tracing::warn!(
                        job_id = %job.job_id,
                        executor = %job.executor_unit,
                        error = %err,
                        "scheduler: job execution failed"
                    );
                    let error_str = err.to_string();
                    if let Err(mark_err) =
                        storage.update_job_status(&job.job_id, "failed", Some(&error_str), &done_at)
                    {
                        tracing::warn!(
                            job_id = %job.job_id,
                            error = %mark_err,
                            "scheduler: failed to mark job as failed"
                        );
                    }
                }
            }
        }

        Ok(())
    }

    async fn execute_scheduled_job(&self, job: &JobRecord) -> Result<()> {
        match job.executor_unit.as_str() {
            "moderation.pipe.message" => self.execute_pipe_message_job(job).await,
            other => {
                tracing::warn!(
                    executor_unit = other,
                    job_id = %job.job_id,
                    "scheduler: unrecognized executor unit, skipping"
                );
                Ok(())
            }
        }
    }

    async fn execute_pipe_message_job(&self, job: &JobRecord) -> Result<()> {
        let payload: serde_json::Value = serde_json::from_str(&job.payload_json)
            .context("scheduler: invalid pipe message payload JSON")?;

        let chat_id = payload["chat_id"]
            .as_i64()
            .context("scheduler: pipe message job missing chat_id")?;

        let text = payload["text"]
            .as_str()
            .context("scheduler: pipe message job missing text")?
            .to_owned();

        let request = TelegramRequest::SendMessage(TelegramSendMessageRequest {
            chat_id,
            text,
            reply_to_message_id: None,
            silent: false,
            parse_mode: ParseMode::PlainText,
            markup: None,
        });

        self.services
            .telegram
            .execute_checked(request, TelegramExecutionOptions { dry_run: false })
            .await
            .map(|_| ())
            .map_err(|err| {
                anyhow::anyhow!("scheduler: telegram error executing pipe message: {err}")
            })
    }

    pub fn host_api(&self) -> Option<&HostApi> {
        self.execution.host_api.as_ref()
    }

    pub fn router(&self) -> Option<&ExecutionRouter> {
        self.execution.router.as_deref()
    }

    pub fn refresh_router_index(&mut self) {
        if let Some(router) = self.execution.router.as_ref() {
            router.sync_registry(self.registry.clone());
        }
    }
}

#[derive(Debug)]
struct RuntimeServices {
    storage: Storage,
    audit: AuditService,
    scheduler: Scheduler,
    telegram: TelegramGateway,
    polling_bot: Option<teloxide_core::Bot>,
    ml_server_transport: MlServerTransport,
}

impl RuntimeServices {
    fn from_config(config: &AppConfig) -> Result<Self> {
        Ok(Self {
            storage: Storage::with_config(
                config.paths.database_path.clone(),
                config.runtime_storage_config()?,
            ),
            audit: AuditService::new(true),
            scheduler: Scheduler::new(
                config.scheduler.tick_interval_ms,
                config.scheduler.max_concurrent_jobs,
            ),
            telegram: match config.telegram.bot_token.as_deref() {
                Some(token) => TelegramGateway::new(config.telegram.polling)
                    .with_transport(TeloxideCoreTransport::new(token.to_owned())),
                None => TelegramGateway::new(config.telegram.polling),
            },
            ml_server_transport: MlServerTransport::new(config.ml_server.base_url.clone())
                .context("failed to configure ml server transport")?,
            polling_bot: match (
                config.telegram.polling,
                config.telegram.bot_token.as_deref(),
            ) {
                (true, Some(token)) => Some(teloxide_core::Bot::new(token.to_owned())),
                _ => None,
            },
        })
    }

    fn polling_bot(&self) -> Option<teloxide_core::Bot> {
        self.polling_bot.clone()
    }
}

#[derive(Debug, Default)]
struct RuntimeExecution {
    host_api: Option<HostApi>,
    router: Option<Rc<ExecutionRouter>>,
    ingress: Option<IngressPipeline>,
}

#[derive(Debug, Clone, Copy)]
pub struct RuntimeBootstrapInfo {
    pub schema_version: u32,
}

pub struct RuntimeSummary<'a> {
    pub database_path: &'a Path,
    pub registry: UnitRegistryStatus,
    pub polling: bool,
    pub scheduler_tick_ms: u64,
    pub audit_enabled: bool,
    pub host_api_dry_run: bool,
    pub transport_name: &'static str,
    pub router_ready: bool,
    pub indexed_trait_routes: usize,
    pub indexed_command_routes: usize,
}

fn load_registry_from_units_dir(config: &AppConfig) -> Result<UnitRegistry> {
    let units_dir = &config.paths.units_dir;
    if !units_dir.exists() {
        return Ok(UnitRegistry::default());
    }

    if !units_dir.is_dir() {
        anyhow::bail!("units_dir {} is not a directory", units_dir.display());
    }

    let mut manifest_paths = fs::read_dir(units_dir)
        .with_context(|| format!("failed to read units_dir {}", units_dir.display()))?
        .collect::<std::io::Result<Vec<_>>>()
        .with_context(|| format!("failed to enumerate units_dir {}", units_dir.display()))?
        .into_iter()
        .filter_map(|entry| match entry.file_type() {
            Ok(file_type) if file_type.is_file() => Some(entry.path()),
            Ok(_) => None,
            Err(_) => Some(entry.path()),
        })
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "toml")
        })
        .collect::<Vec<_>>();
    manifest_paths.sort();

    if manifest_paths.is_empty() {
        return Ok(UnitRegistry::default());
    }

    let report = UnitRegistry::load_paths(&manifest_paths);
    if report.is_fully_valid() || config.runtime.degraded_mode_enabled {
        Ok(report.registry)
    } else {
        anyhow::bail!(
            "failed to load unit manifests from {}: {} invalid manifest(s)",
            units_dir.display(),
            report.registry.status_summary().failed_units
        );
    }
}

#[cfg(test)]
mod tests {
    use super::Runtime;
    use crate::config::AppConfig;
    use crate::event::{
        ChatContext, EventContext, EventNormalizer, ManualInvocationInput, SenderContext,
        UnitContext,
    };
    use crate::router::ExecutionLane;
    use crate::unit::{ServiceSpec, TriggerSpec, UnitDefinition, UnitManifest, UnitRegistry};
    use chrono::{TimeZone, Utc};
    use tempfile::TempDir;

    fn runtime_test_config() -> (TempDir, AppConfig) {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut config = AppConfig::default();
        config.paths.database_path = dir.path().join("runtime.sqlite3");
        config.paths.units_dir = dir.path().join("units");
        (dir, config)
    }

    fn write_unit_manifest(dir: &TempDir, file_name: &str, body: &str) {
        let units_dir = dir.path().join("units");
        std::fs::create_dir_all(&units_dir).expect("units dir");
        std::fs::write(units_dir.join(file_name), body).expect("manifest written");
    }

    fn registry_from_manifests(manifests: Vec<UnitManifest>) -> UnitRegistry {
        UnitRegistry::load_manifests(manifests).registry
    }

    fn command_manifest(name: &str, command: &str) -> UnitManifest {
        UnitManifest::new(
            UnitDefinition::new(name),
            TriggerSpec::command([command]),
            ServiceSpec::new(format!("scripts/{name}.rhai")),
        )
    }

    fn manual_event(command_text: &str) -> EventContext {
        let mut input = ManualInvocationInput::new(
            UnitContext::new("runtime.test").with_trigger("manual"),
            command_text,
        );
        input.received_at = Utc
            .with_ymd_and_hms(2026, 4, 22, 12, 0, 0)
            .single()
            .expect("valid timestamp");
        input.chat = Some(ChatContext {
            id: -100123,
            chat_type: "supergroup".to_owned(),
            title: Some("Moderation HQ".to_owned()),
            username: Some("mod_hq".to_owned()),
            thread_id: Some(11),
        });
        input.sender = Some(SenderContext {
            id: 42,
            username: Some("admin".to_owned()),
            display_name: Some("Admin".to_owned()),
            is_bot: false,
            is_admin: true,
            role: Some("owner".to_owned()),
        });

        EventNormalizer::new()
            .normalize_manual(input)
            .expect("manual event normalizes")
    }

    #[test]
    fn from_config_keeps_empty_registry_when_units_dir_is_missing() {
        let (_dir, config) = runtime_test_config();

        let runtime = Runtime::from_config(&config).expect("runtime builds");
        let summary = runtime.summary();

        assert_eq!(summary.registry.total_units, 0);
        assert_eq!(summary.registry.failed_units, 0);
    }

    #[test]
    fn from_config_loads_registry_from_units_dir() {
        let (dir, config) = runtime_test_config();
        write_unit_manifest(
            &dir,
            "moderation.warn.unit.toml",
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

        let runtime = Runtime::from_config(&config).expect("runtime builds");
        let summary = runtime.summary();

        assert_eq!(summary.registry.total_units, 1);
        assert_eq!(summary.registry.active_units, 1);
    }

    #[tokio::test]
    async fn startup_indexes_routes_from_loaded_units() {
        let (dir, config) = runtime_test_config();
        write_unit_manifest(
            &dir,
            "command.stats.unit.toml",
            r#"
[Unit]
Name = "command.stats.unit"

[Trigger]
Type = "command"
Commands = ["stats"]

[Service]
ExecStart = "scripts/command/stats.rhai"
"#,
        );

        let mut runtime = Runtime::from_config(&config).expect("runtime builds");
        runtime.startup(&config).await.expect("startup succeeds");
        let summary = runtime.summary();

        assert!(summary.router_ready);
        assert_eq!(summary.indexed_command_routes, 7);
        assert_eq!(summary.transport_name, "noop");
        assert!(runtime.host_api().is_some());
        assert!(runtime.router().is_some());
    }

    #[tokio::test]
    async fn startup_and_refresh_use_the_live_registry_state() {
        let (_dir, config) = runtime_test_config();
        let mut runtime = Runtime::from_config(&config).expect("runtime builds");

        runtime.registry =
            registry_from_manifests(vec![command_manifest("command.stats.unit", "stats")]);
        runtime.startup(&config).await.expect("startup succeeds");

        let stats_plan = runtime
            .router()
            .expect("router is available")
            .plan(&manual_event("/stats"));
        assert!(stats_plan.lanes.contains(&ExecutionLane::UnitDispatch));
        assert_eq!(runtime.summary().indexed_command_routes, 7);

        runtime.registry =
            registry_from_manifests(vec![command_manifest("command.audit.unit", "audit")]);
        runtime.refresh_router_index();

        let audit_plan = runtime
            .router()
            .expect("router is available")
            .plan(&manual_event("/audit"));
        let stale_plan = runtime
            .router()
            .expect("router is available")
            .plan(&manual_event("/stats"));

        assert!(audit_plan.lanes.contains(&ExecutionLane::UnitDispatch));
        assert!(!stale_plan.lanes.contains(&ExecutionLane::UnitDispatch));
        assert_eq!(runtime.summary().indexed_command_routes, 7);
    }

    #[test]
    fn degraded_mode_keeps_failed_manifest_entries_in_registry_summary() {
        let (dir, config) = runtime_test_config();
        write_unit_manifest(
            &dir,
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

        let runtime = Runtime::from_config(&config).expect("runtime builds in degraded mode");
        let summary = runtime.summary();

        assert_eq!(summary.registry.total_units, 1);
        assert_eq!(summary.registry.failed_units, 1);
    }

    #[test]
    fn strict_mode_fails_when_units_dir_contains_invalid_manifest() {
        let (dir, mut config) = runtime_test_config();
        config.runtime.degraded_mode_enabled = false;
        write_unit_manifest(
            &dir,
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

        let error = Runtime::from_config(&config).expect_err("runtime must fail");
        assert!(
            error
                .to_string()
                .contains("failed to load unit manifests from")
        );
    }
}
