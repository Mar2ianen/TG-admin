use crate::audit::AuditService;
use crate::config::AppConfig;
use crate::host_api::HostApi;
use crate::host_api::MlServerTransport;
use crate::ingress::IngressPipeline;
use crate::moderation::ModerationEngine;
use crate::router::{ExecutionRouter, RouterIndex};
use crate::scheduler::Scheduler;
use crate::shutdown::{ShutdownController, ShutdownReason};
use crate::storage::Storage;
use crate::tg::{TelegramGateway, TeloxideCoreTransport};
use crate::unit::{UnitRegistry, UnitRegistryStatus};
use anyhow::{Context, Result};
use std::fs;
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

        let registry_handle = std::rc::Rc::new(self.registry.clone());
        let host_api = HostApi::new(false)
            .with_storage(host_api_storage)
            .with_unit_registry_handle(registry_handle.clone())
            .with_ml_server_transport(self.services.ml_server_transport.clone());
        let moderation = ModerationEngine::new(moderation_storage, self.services.telegram.clone())
            .with_unit_registry_handle(registry_handle)
            .with_admin_user_ids(config.telegram.admin_user_ids.iter().copied())
            .without_processed_update_guard();
        let router = Rc::new(
            ExecutionRouter::new()
                .with_registry(self.registry.clone())
                .with_index(RouterIndex::from_registry(&self.registry))
                .with_moderation(moderation),
        );
        let ingress = self.services.polling_bot().map(|bot| {
            IngressPipeline::new(bot, ingress_storage, router.clone())
                .with_admin_user_ids(config.telegram.admin_user_ids.iter().copied())
        });

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
            scheduler: Scheduler::new(config.scheduler.tick_interval_ms),
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
        assert!(runtime.router().is_some());
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
