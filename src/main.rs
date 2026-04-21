use anyhow::Result;
use telegram_moderation_os::observability::init_logging;
use telegram_moderation_os::{AppConfig, Application};
use tracing::info;

#[tokio::main]
async fn main() -> Result<()> {
    let config = AppConfig::load()?;
    init_logging(&config)?;
    info!("bootstrapping application");

    let mut application = Application::from_config(config);
    application.run().await
}
