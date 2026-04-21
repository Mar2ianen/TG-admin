use crate::config::AppConfig;
use anyhow::Result;
use tracing::level_filters::LevelFilter;
use tracing::warn;
use tracing_subscriber::EnvFilter;

pub fn init_logging(config: &AppConfig) -> Result<()> {
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

#[cfg(test)]
mod tests {
    use super::parse_level;

    #[test]
    fn invalid_log_level_falls_back_to_info() {
        let directive = parse_level("definitely-not-a-level");
        assert_eq!(directive.to_string(), "info");
    }
}
