mod app;
mod audit;
mod config;
mod event;
mod host_api;
mod scheduler;
mod storage;
mod tg;
mod unit;

use crate::app::Application;
use crate::config::AppConfig;
use anyhow::Result;
use tracing::level_filters::LevelFilter;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    let config = AppConfig::load()?;
    bootstrap_logging(&config)?;
    info!("bootstrapping application");

    Application::from_config(config).run().await
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
