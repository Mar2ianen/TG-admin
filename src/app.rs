use crate::config::AppConfig;
use crate::event::EventContext;
use crate::runtime::Runtime;
use crate::shutdown::{ShutdownController, ShutdownReason};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use tokio::time::{Duration, timeout};
use tracing::{info, warn};

pub struct Application {
    config: AppConfig,
    state: ApplicationState,
    runtime: Runtime,
    shutdown: ShutdownController,
}

impl Application {
    pub fn from_config(config: AppConfig) -> Result<Self> {
        Self::from_config_with_shutdown(config, ShutdownController::os_signals())
    }

    fn from_config_with_shutdown(config: AppConfig, shutdown: ShutdownController) -> Result<Self> {
        let startup_event = EventContext::system_event();
        let runtime = Runtime::from_config(&config)?;

        Ok(Self {
            config,
            state: ApplicationState::new(startup_event),
            runtime,
            shutdown,
        })
    }

    pub async fn run(&mut self) -> Result<()> {
        self.startup().await?;
        let reason = self.runtime.run_until_shutdown(self.shutdown).await?;
        self.graceful_shutdown(reason).await?;
        Ok(())
    }

    async fn startup(&mut self) -> Result<()> {
        self.state.mark_starting();
        let startup = self
            .runtime
            .startup(&self.config)
            .await
            .context("failed to bootstrap runtime during startup")?;

        let summary = self.runtime.summary();
        info!(
            event_id = %self.state.startup_event.event_id,
            update_type = ?self.state.startup_event.update_type,
            execution_mode = ?self.state.startup_event.execution_mode,
            database_path = %summary.database_path.display(),
            storage_schema_version = startup.schema_version,
            units_loaded = summary.registry.total_units,
            polling = summary.polling,
            telegram_transport = summary.transport_name,
            router_ready = summary.router_ready,
            scheduler_tick_ms = summary.scheduler_tick_ms,
            audit_enabled = summary.audit_enabled,
            host_api_dry_run = summary.host_api_dry_run,
            shutdown_grace_period_ms = self.config.runtime.shutdown_grace_period_ms,
            "runtime skeleton initialized"
        );

        self.state.mark_running();
        info!(lifecycle = ?self.state.lifecycle, "startup path complete");

        Ok(())
    }

    async fn graceful_shutdown(&mut self, reason: ShutdownReason) -> Result<()> {
        self.state.mark_shutting_down();

        info!(
            lifecycle = ?self.state.lifecycle,
            reason = ?reason,
            grace_period_ms = self.config.runtime.shutdown_grace_period_ms,
            "shutdown signal received"
        );

        let grace_period = Duration::from_millis(self.config.runtime.shutdown_grace_period_ms);
        let shutdown = timeout(grace_period, self.runtime.shutdown());

        match shutdown.await {
            Ok(Ok(())) => {
                self.state.mark_stopped();
                info!(lifecycle = ?self.state.lifecycle, reason = ?reason, "shutdown path complete");
            }
            Ok(Err(err)) => return Err(err),
            Err(_) => {
                self.state.mark_stopped();
                warn!(
                    lifecycle = ?self.state.lifecycle,
                    reason = ?reason,
                    grace_period_ms = self.config.runtime.shutdown_grace_period_ms,
                    "shutdown grace period exceeded"
                );
            }
        }

        Ok(())
    }
}

#[derive(Debug)]
struct ApplicationState {
    lifecycle: LifecycleState,
    startup_event: EventContext,
    started_at: Option<DateTime<Utc>>,
    stopped_at: Option<DateTime<Utc>>,
}

impl ApplicationState {
    fn new(startup_event: EventContext) -> Self {
        Self {
            lifecycle: LifecycleState::Created,
            startup_event,
            started_at: None,
            stopped_at: None,
        }
    }

    fn mark_starting(&mut self) {
        self.lifecycle = LifecycleState::Starting;
    }

    fn mark_running(&mut self) {
        self.lifecycle = LifecycleState::Running;
        self.started_at = Some(Utc::now());
    }

    fn mark_shutting_down(&mut self) {
        self.lifecycle = LifecycleState::ShuttingDown;
    }

    fn mark_stopped(&mut self) {
        self.lifecycle = LifecycleState::Stopped;
        self.stopped_at = Some(Utc::now());
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum LifecycleState {
    Created,
    Starting,
    Running,
    ShuttingDown,
    Stopped,
}

#[cfg(test)]
mod tests {
    use super::{Application, LifecycleState};
    use crate::config::AppConfig;
    use crate::runtime::Runtime;
    use crate::shutdown::{ShutdownController, ShutdownReason};
    use std::fs;
    use tempfile::TempDir;

    fn app_test_config() -> (TempDir, AppConfig) {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut config = AppConfig::default();
        config.paths.database_path = dir.path().join("runtime.sqlite3");
        (dir, config)
    }

    #[tokio::test]
    async fn startup_and_shutdown_transition_application_to_stopped() {
        let (_dir, config) = app_test_config();
        let mut app = Application::from_config_with_shutdown(
            config,
            ShutdownController::immediate(),
        )
        .expect("application builds");
        assert_eq!(app.state.lifecycle, LifecycleState::Created);

        app.startup().await.expect("startup succeeds");
        assert_eq!(app.state.lifecycle, LifecycleState::Running);

        app.graceful_shutdown(ShutdownReason::Immediate)
            .await
            .expect("shutdown succeeds");
        assert_eq!(app.state.lifecycle, LifecycleState::Stopped);
        assert!(app.state.started_at.is_some());
        assert!(app.state.stopped_at.is_some());
    }

    #[test]
    fn runtime_summary_reflects_empty_registry() {
        let (_dir, config) = app_test_config();
        let app = Application::from_config(config).expect("application builds");
        let summary = app.runtime.summary();

        assert_eq!(summary.registry.total_units, 0);
        assert!(summary.polling);
        assert!(!summary.router_ready);
        assert_eq!(summary.indexed_command_routes, 0);
    }

    #[test]
    fn audit_service_is_independent_from_metrics_flag() {
        let (_dir, mut config) = app_test_config();
        config.observability.metrics_enabled = false;

        let runtime = Runtime::from_config(&config).expect("runtime builds");
        let summary = runtime.summary();

        assert!(summary.audit_enabled);
    }

    #[test]
    fn manual_mode_does_not_enable_dry_run() {
        let (_dir, mut config) = app_test_config();
        config.runtime.manual_mode_enabled = true;

        let runtime = Runtime::from_config(&config).expect("runtime builds");
        let summary = runtime.summary();

        assert!(!summary.host_api_dry_run);
    }

    #[test]
    fn bot_token_switches_runtime_transport_to_teloxide_core() {
        let (_dir, mut config) = app_test_config();
        config.telegram.bot_token = Some("123456:TEST_TOKEN".to_owned());

        let runtime = Runtime::from_config(&config).expect("runtime builds");
        let summary = runtime.summary();

        assert_eq!(summary.transport_name, "teloxide-core");
    }

    #[tokio::test]
    async fn application_run_with_immediate_shutdown_reaches_stopped_state() {
        let (_dir, config) = app_test_config();
        let mut app = Application::from_config_with_shutdown(
            config,
            ShutdownController::immediate(),
        )
        .expect("application builds");

        app.run().await.expect("application run succeeds");

        assert_eq!(app.state.lifecycle, LifecycleState::Stopped);
        assert!(app.state.started_at.is_some());
        assert!(app.state.stopped_at.is_some());
    }

    #[tokio::test]
    async fn startup_fails_when_database_path_is_unopenable() {
        let base = std::env::temp_dir().join(format!(
            "telegram-moderation-os-app-startup-invalid-db-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let database_path = base.join("runtime.sqlite3");
        fs::create_dir_all(&database_path).expect("db path directory exists");

        let mut config = AppConfig::default();
        config.paths.database_path = database_path;

        let mut app = Application::from_config_with_shutdown(
            config,
            ShutdownController::immediate(),
        )
        .expect("application builds");

        let error = app.startup().await.expect_err("startup must fail");
        assert!(error.to_string().contains("failed to bootstrap runtime"));
    }

    #[tokio::test]
    async fn startup_wires_router_and_host_api_into_runtime() {
        let (_dir, config) = app_test_config();
        let mut app = Application::from_config_with_shutdown(
            config,
            ShutdownController::immediate(),
        )
        .expect("application builds");

        app.startup().await.expect("startup succeeds");

        let summary = app.runtime.summary();
        assert!(summary.router_ready);
        assert_eq!(summary.transport_name, "noop");
        assert_eq!(summary.indexed_command_routes, 6);
        assert!(app.runtime.host_api().is_some());
        assert!(app.runtime.router().is_some());
    }
}
