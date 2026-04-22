use crate::audit::AuditService;
use crate::config::AppConfig;
use crate::host_api::HostApi;
use crate::ingress::IngressPipeline;
use crate::moderation::ModerationEngine;
use crate::router::{ExecutionRouter, RouterIndex};
use crate::scheduler::Scheduler;
use crate::shutdown::{ShutdownController, ShutdownReason};
use crate::storage::Storage;
use crate::tg::{TelegramGateway, TeloxideCoreTransport};
use crate::unit::{UnitRegistry, UnitRegistryStatus};
use anyhow::{Context, Result};
use std::path::Path;
use std::rc::Rc;

#[derive(Debug)]
pub struct Runtime {
    registry: UnitRegistry,
    services: RuntimeServices,
    execution: RuntimeExecution,
}

impl Runtime {
    pub fn from_config(config: &AppConfig) -> Result<Self> {
        Ok(Self {
            registry: UnitRegistry::default(),
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

        let registry_handle = std::rc::Rc::new(self.registry.clone());
        let host_api = HostApi::new(false)
            .with_storage(host_api_storage)
            .with_unit_registry_handle(registry_handle.clone());
        let moderation = ModerationEngine::new(moderation_storage, self.services.telegram.clone())
            .with_unit_registry_handle(registry_handle)
            .with_admin_user_ids(config.telegram.admin_user_ids.iter().copied())
            .without_processed_update_guard();
        let router = Rc::new(
            ExecutionRouter::new()
                .with_index(RouterIndex::from_registry(&self.registry))
                .with_moderation(moderation),
        );
        let ingress = self
            .services
            .polling_bot()
            .map(|bot| IngressPipeline::new(bot, ingress_storage, router.clone()));

        self.execution = RuntimeExecution {
            host_api: Some(host_api),
            router: Some(router),
            ingress,
        };

        Ok(RuntimeBootstrapInfo { schema_version })
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
        match self.execution.ingress.as_ref() {
            Some(ingress) => ingress.run_until_shutdown(shutdown).await,
            None => shutdown.wait().await,
        }
    }

    pub fn host_api(&self) -> Option<&HostApi> {
        self.execution.host_api.as_ref()
    }

    pub fn router(&self) -> Option<&ExecutionRouter> {
        self.execution.router.as_deref()
    }
}

#[derive(Debug)]
struct RuntimeServices {
    storage: Storage,
    audit: AuditService,
    scheduler: Scheduler,
    telegram: TelegramGateway,
    polling_bot: Option<teloxide_core::Bot>,
}

impl RuntimeServices {
    fn from_config(config: &AppConfig) -> Result<Self> {
        Ok(Self {
            storage: Storage::with_config(
                config.paths.database_path.clone(),
                config.runtime_storage_config()?,
            ),
            audit: AuditService::new(true),
            scheduler: Scheduler::new(config.scheduler.tick_interval_ms),
            telegram: match config.telegram.bot_token.as_deref() {
                Some(token) => TelegramGateway::new(config.telegram.polling)
                    .with_transport(TeloxideCoreTransport::new(token.to_owned())),
                None => TelegramGateway::new(config.telegram.polling),
            },
            polling_bot: match (config.telegram.polling, config.telegram.bot_token.as_deref()) {
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
