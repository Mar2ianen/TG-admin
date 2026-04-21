mod audit;
mod config;
mod event;
mod host_api;
mod scheduler;
mod storage;
mod tg;
mod unit;

use crate::audit::AuditService;
use crate::config::AppConfig;
use crate::event::EventContext;
use crate::host_api::HostApi;
use crate::scheduler::Scheduler;
use crate::storage::Storage;
use crate::tg::TelegramGateway;
use crate::unit::UnitRegistry;
use anyhow::Result;
use tracing::level_filters::LevelFilter;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    let config = AppConfig::load()?;
    bootstrap_logging(&config)?;

    let startup_event = EventContext::system_event();
    let runtime = Runtime::from_config(config);

    info!(
        event_id = %startup_event.event_id,
        update_type = ?startup_event.update_type,
        execution_mode = ?startup_event.execution_mode,
        database_path = %runtime.storage.database_path().display(),
        units_loaded = runtime.units.status_summary().total_units,
        polling = runtime.telegram.polling(),
        scheduler_tick_ms = runtime.scheduler.tick_interval_ms(),
        audit_enabled = runtime.audit.enabled(),
        host_api_dry_run = runtime.host_api.dry_run(),
        "runtime skeleton initialized"
    );

    info!("startup path complete");
    info!("shutdown path complete");

    Ok(())
}

fn bootstrap_logging(config: &AppConfig) -> Result<()> {
    let env_filter = EnvFilter::builder()
        .with_default_directive(parse_level(&config.observability.log_level))
        .from_env_lossy();

    let builder = tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_target(true);

    if config.observability.json_logs {
        builder
            .json()
            .try_init()
            .map_err(|err| anyhow::anyhow!(err.to_string()))?;
    } else {
        builder
            .compact()
            .try_init()
            .map_err(|err| anyhow::anyhow!(err.to_string()))?;
    }

    Ok(())
}

fn parse_level(level: &str) -> tracing_subscriber::filter::Directive {
    level
        .parse::<LevelFilter>()
        .unwrap_or_else(|_| {
            warn!(
                requested_level = level,
                "invalid log level, falling back to info"
            );
            LevelFilter::INFO
        })
        .into()
}

struct Runtime {
    storage: Storage,
    units: UnitRegistry,
    audit: AuditService,
    scheduler: Scheduler,
    telegram: TelegramGateway,
    host_api: HostApi,
}

impl Runtime {
    fn from_config(config: AppConfig) -> Self {
        Self {
            storage: Storage::new(config.paths.database_path.clone()),
            units: UnitRegistry::new(),
            audit: AuditService::new(config.observability.metrics_enabled),
            scheduler: Scheduler::new(config.scheduler.tick_interval_ms),
            telegram: TelegramGateway::new(config.telegram.polling),
            host_api: HostApi::new(config.runtime.manual_mode_enabled),
        }
    }
}
