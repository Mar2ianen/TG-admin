use crate::audit::AuditService;
use crate::config::AppConfig;
use crate::host_api::HostApi;
use crate::host_api::MlServerTransport;
use crate::ingress::IngressPipeline;
use crate::moderation::ModerationEngine;
use crate::reputation::ReputationClient;
use crate::router::ExecutionRouter;
use crate::scheduler::Scheduler;
use crate::script::ScriptRunner;
use crate::shutdown::{ShutdownController, ShutdownReason};
use crate::storage::JobRecord;
use crate::storage::Storage;
use crate::storage::StorageConnection;
use crate::tg::init::{ChatInitializer, fetch_bot_id};
use crate::tg::{
    ParseMode, TelegramExecutionOptions, TelegramGateway, TelegramRequest,
    TelegramSendMessageRequest, TeloxideCoreTransport,
};
use crate::unit::{UnitRegistry, UnitRegistryStatus};
use anyhow::{Context, Result};
use chrono::{Datelike, Duration as ChronoDuration, Timelike, Utc};
use std::fs;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
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
        self.recover_stale_scheduler_jobs(&moderation_storage)
            .context("failed to recover stale scheduler jobs during runtime startup")?;

        // Прогрев кэша репутации
        if let Some(reputation) = self.services.reputation_client.as_ref() {
            if let Err(err) = reputation.warm_cache(1000).await {
                tracing::warn!(error = %err, "failed to warm reputation cache");
            } else {
                tracing::info!("reputation cache warmed successfully");
            }
        }

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
        let bot_id = self
            .bootstrap_telegram_readiness(config)
            .await
            .context("failed to verify Telegram readiness during runtime startup")?;

        self.execution = self.compose_execution(
            config,
            moderation_storage,
            host_api_storage,
            ingress_storage,
            script_storage,
            bot_id,
        );

        Ok(RuntimeBootstrapInfo { schema_version })
    }

    async fn bootstrap_telegram_readiness(&self, config: &AppConfig) -> Result<i64> {
        let Some(bot) = self.services.polling_bot.as_ref() else {
            return Ok(0);
        };

        let bot_id = fetch_bot_id(bot)
            .await
            .context("failed to resolve Telegram bot identity with getMe")?;

        if !config.telegram.primary_chat_ids.is_empty() {
            let initializer =
                ChatInitializer::new(self.services.telegram.transport(), &self.services.storage);
            initializer
                .initialize_primary_chats(config.telegram.primary_chat_ids.iter().copied(), bot_id)
                .await
                .context("failed to initialize configured primary Telegram chats")?;
        }

        Ok(bot_id)
    }

    fn recover_stale_scheduler_jobs(&self, storage: &StorageConnection) -> Result<usize> {
        let recovery_now = Utc::now();
        let stale_before = recovery_now
            - ChronoDuration::milliseconds(
                self.services.config.scheduler.max_scheduler_lag_ms as i64,
            );

        storage
            .recover_stale_processing_jobs(&stale_before.to_rfc3339(), &recovery_now.to_rfc3339())
            .context("scheduler: failed to recover stale processing jobs")
    }

    fn compose_execution(
        &self,
        config: &AppConfig,
        moderation_storage: StorageConnection,
        host_api_storage: StorageConnection,
        ingress_storage: StorageConnection,
        script_storage: StorageConnection,
        bot_id: i64,
    ) -> RuntimeExecution {
        let registry_handle = Rc::new(self.registry.clone());
        let host_api = HostApi::new(false)
            .with_storage(host_api_storage)
            .with_unit_registry_handle(registry_handle.clone())
            .with_ml_server_transport(self.services.ml_server_transport.clone())
            .with_templates_dir(config.paths.templates_dir.clone());

        let mut moderation =
            ModerationEngine::new(moderation_storage, self.services.telegram.clone())
                .with_unit_registry_handle(registry_handle.clone())
                .with_admin_user_ids(config.telegram.admin_user_ids.iter().copied())
                .without_processed_update_guard();

        if let Some(reputation) = self.services.reputation_client.clone() {
            moderation = moderation.with_reputation_client(reputation);
        }

        let script_host_api = HostApi::new(false)
            .with_storage(script_storage)
            .with_unit_registry_handle(registry_handle.clone())
            .with_ml_server_transport(self.services.ml_server_transport.clone())
            .with_templates_dir(config.paths.templates_dir.clone());
        let script_runner = ScriptRunner::new(config.paths.scripts_dir.clone());
        let router = Rc::new(
            ExecutionRouter::new(bot_id, config.moderation.delete_unknown)
                .with_registry_handle(registry_handle.clone())
                .with_moderation(moderation)
                .with_script_runner(script_runner, script_host_api)
                .with_gateway(Arc::new(self.services.telegram.clone()))
                .with_storage(self.services.storage.clone()),
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
            if let Err(err) = self.tick_counters().await {
                tracing::warn!(error = %err, "counters tick error");
            }
        }
    }

    async fn tick_scheduler(&self) -> Result<()> {
        let storage = self
            .services
            .storage
            .init()
            .context("scheduler: failed to open storage connection")?;

        let recovered = self
            .recover_stale_scheduler_jobs(&storage)
            .context("scheduler: failed to recover stale processing jobs")?;
        if recovered > 0 {
            tracing::info!(recovered, "scheduler: recovered stale processing jobs");
        }

        let now = Utc::now().to_rfc3339();
        let limit = self.services.scheduler.max_concurrent_jobs();
        let due_jobs = storage
            .poll_due_jobs(&now, limit)
            .context("scheduler: failed to poll due jobs")?;

        for job in &due_jobs {
            let claimed_at = Utc::now().to_rfc3339();
            let claimed = storage
                .claim_job(&job.job_id, &claimed_at)
                .context("scheduler: failed to claim job")?;
            if !claimed {
                tracing::warn!(
                    job_id = %job.job_id,
                    "scheduler: skipped job claim because status changed"
                );
                continue;
            }

            let result = self.execute_scheduled_job(job).await;
            let done_at = Utc::now().to_rfc3339();
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

    async fn tick_counters(&self) -> Result<()> {
        let mut storage = self
            .services
            .storage
            .init()
            .context("counters: failed to open storage connection")?;

        let now = Utc::now();
        let reset_hour = self.services.config.runtime.counters.reset_hour;

        if now.hour() == reset_hour {
            let date_str = now.format("%Y-%m-%d").to_string();
            let last_snapshot = storage
                .get_kv("system", "counters", "last_daily_snapshot")?
                .map(|e| e.value_json)
                .unwrap_or_default();

            if last_snapshot != date_str {
                tracing::info!(date = %date_str, "performing daily counter snapshots");

                // Daily snapshot
                storage.create_counter_snapshots("day", &date_str)?;

                // Weekly snapshot (on Mondays)
                if now.weekday() == chrono::Weekday::Mon {
                    storage.create_counter_snapshots("week", &date_str)?;
                }

                // Monthly snapshot (on 1st of month)
                if now.day() == 1 {
                    storage.create_counter_snapshots("month", &date_str)?;
                }

                // Yearly snapshot (on Jan 1st)
                if now.month() == 1 && now.day() == 1 {
                    storage.create_counter_snapshots("year", &date_str)?;
                }

                storage.set_kv(&crate::storage::KvEntry {
                    scope_kind: "system".to_owned(),
                    scope_id: "counters".to_owned(),
                    key: "last_daily_snapshot".to_owned(),
                    value_json: date_str,
                    updated_at: now.to_rfc3339(),
                })?;
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
    reputation_client: Option<Rc<ReputationClient>>,
    config: Rc<AppConfig>,
}

impl RuntimeServices {
    fn from_config(config: &AppConfig) -> Result<Self> {
        let reputation_client = if config.reputation.enabled {
            Some(Rc::new(ReputationClient::new(
                config.reputation.base_url.clone(),
                "bot".to_owned(), // TODO: Get real bot ID/name
            )))
        } else {
            None
        };
        let storage = Storage::with_config(
            config.paths.database_path.clone(),
            config.runtime_storage_config()?,
        );
        let telegram = match config.telegram.bot_token.as_deref() {
            Some(token) => TelegramGateway::new(config.telegram.polling)
                .with_idempotency_storage(storage.clone())
                .with_transport(TeloxideCoreTransport::new(token.to_owned())),
            None => TelegramGateway::new(config.telegram.polling)
                .with_idempotency_storage(storage.clone()),
        };

        Ok(Self {
            storage,
            audit: AuditService::new(true),
            scheduler: Scheduler::new(
                config.scheduler.tick_interval_ms,
                config.scheduler.max_concurrent_jobs,
            ),
            telegram,
            ml_server_transport: MlServerTransport::new(config.ml_server.base_url.clone())
                .context("failed to configure ml server transport")?,
            polling_bot: match (
                config.telegram.polling,
                config.telegram.bot_token.as_deref(),
            ) {
                (true, Some(token)) => Some(teloxide_core::Bot::new(token.to_owned())),
                _ => None,
            },
            reputation_client,
            config: Rc::new(config.clone()),
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

    let paths = fs::read_dir(units_dir)
        .with_context(|| format!("failed to read units dir `{}`", units_dir.display()))?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "toml"));

    let report = UnitRegistry::load_paths(paths);
    if !report.is_fully_valid() && !config.runtime.degraded_mode_enabled {
        anyhow::bail!("units registry contains invalid manifests");
    }

    Ok(report.registry)
}
