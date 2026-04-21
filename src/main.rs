mod app;
mod audit;
mod config;
mod event;
mod host_api;
mod observability;
mod parser;
mod scheduler;
mod shutdown;
mod storage;
mod tg;
mod unit;

use crate::app::Application;
use crate::config::AppConfig;
use crate::observability::init_logging;
use anyhow::Result;
use tracing::info;

#[tokio::main]
async fn main() -> Result<()> {
    let config = AppConfig::load()?;
    init_logging(&config)?;
    info!("bootstrapping application");

    let mut application = Application::from_config(config);
    application.run().await
}
