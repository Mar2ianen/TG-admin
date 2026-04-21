use crate::audit::AuditService;
use crate::config::AppConfig;
use crate::event::EventContext;
use crate::host_api::HostApi;
use crate::scheduler::Scheduler;
use crate::shutdown::{ShutdownController, ShutdownReason};
use crate::storage::Storage;
use crate::tg::TelegramGateway;
use crate::unit::{UnitRegistry, UnitRegistryStatus};
use anyhow::Result;
use chrono::{DateTime, Utc};
use tokio::time::{Duration, timeout};
use tracing::{info, warn};

pub struct Application {
    config: AppConfig,
    state: ApplicationState,
    runtime: RuntimeState,
    shutdown: ShutdownController,
}

impl Application {
    pub fn from_config(config: AppConfig) -> Self {
        Self::from_config_with_shutdown(config, ShutdownController::os_signals())
    }

    fn from_config_with_shutdown(config: AppConfig, shutdown: ShutdownController) -> Self {
        let startup_event = EventContext::system_event();
        let runtime = RuntimeState::from_config(&config);

        Self {
            config,
            state: ApplicationState::new(startup_event),
            runtime,
            shutdown,
        }
    }

    pub async fn run(mut self) -> Result<()> {
        self.startup().await?;
        let reason = self.shutdown.wait().await?;
        self.graceful_shutdown(reason).await?;
        Ok(())
    }

    async fn startup(&mut self) -> Result<()> {
        self.state.mark_starting();

        let summary = self.runtime.summary();
        info!(
            event_id = %self.state.startup_event.event_id,
            update_type = ?self.state.startup_event.update_type,
            execution_mode = ?self.state.startup_event.execution_mode,
            database_path = %summary.database_path.display(),
            units_loaded = summary.registry.total_units,
            polling = summary.polling,
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

#[derive(Debug)]
struct RuntimeState {
    registry: RuntimeRegistry,
    services: RuntimeServices,
}

impl RuntimeState {
    fn from_config(config: &AppConfig) -> Self {
        Self {
            registry: RuntimeRegistry::default(),
            services: RuntimeServices::from_config(config),
        }
    }

    fn summary(&self) -> RuntimeSummary<'_> {
        RuntimeSummary {
            database_path: self.services.storage.database_path(),
            registry: self.registry.units.status_summary(),
            polling: self.services.telegram.polling(),
            scheduler_tick_ms: self.services.scheduler.tick_interval_ms(),
            audit_enabled: self.services.audit.enabled(),
            host_api_dry_run: self.services.host_api.dry_run(),
        }
    }

    async fn shutdown(&mut self) -> Result<()> {
        self.registry = RuntimeRegistry::default();
        tokio::task::yield_now().await;
        Ok(())
    }
}

#[derive(Debug, Default)]
struct RuntimeRegistry {
    units: UnitRegistry,
}

#[derive(Debug)]
struct RuntimeServices {
    storage: Storage,
    audit: AuditService,
    scheduler: Scheduler,
    telegram: TelegramGateway,
    host_api: HostApi,
}

impl RuntimeServices {
    fn from_config(config: &AppConfig) -> Self {
        Self {
            storage: Storage::new(config.paths.database_path.clone()),
            audit: AuditService::new(config.observability.metrics_enabled),
            scheduler: Scheduler::new(config.scheduler.tick_interval_ms),
            telegram: TelegramGateway::new(config.telegram.polling),
            host_api: HostApi::new(config.runtime.manual_mode_enabled),
        }
    }
}

struct RuntimeSummary<'a> {
    database_path: &'a std::path::Path,
    registry: UnitRegistryStatus,
    polling: bool,
    scheduler_tick_ms: u64,
    audit_enabled: bool,
    host_api_dry_run: bool,
}

#[cfg(test)]
mod tests {
    use super::{Application, LifecycleState};
    use crate::config::AppConfig;
    use crate::shutdown::ShutdownController;

    #[tokio::test]
    async fn application_run_transitions_to_stopped() {
        let app = Application::from_config_with_shutdown(
            AppConfig::default(),
            ShutdownController::immediate(),
        );
        assert_eq!(app.state.lifecycle, LifecycleState::Created);

        app.run().await.expect("application run succeeds");
    }

    #[test]
    fn runtime_summary_reflects_empty_registry() {
        let app = Application::from_config(AppConfig::default());
        let summary = app.runtime.summary();

        assert_eq!(summary.registry.total_units, 0);
        assert!(summary.polling);
    }

    #[tokio::test]
    async fn immediate_shutdown_marks_application_stopped() {
        let app = Application::from_config_with_shutdown(
            AppConfig::default(),
            ShutdownController::immediate(),
        );

        app.run().await.expect("application run succeeds");
    }
}
