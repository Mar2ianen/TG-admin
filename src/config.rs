use crate::storage::{JournalMode, SynchronousMode, TempStoreMode};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct AppConfig {
    pub telegram: TelegramConfig,
    pub paths: PathsConfig,
    pub storage: ConfigStorage,
    pub runtime: RuntimeConfig,
    pub ml_server: MlServerConfig,
    pub limits: LimitsConfig,
    pub fetch_policy: FetchPolicyConfig,
    pub scheduler: SchedulerConfig,
    pub observability: ObservabilityConfig,
    pub features: FeatureFlags,
}

impl AppConfig {
    pub fn load() -> Result<Self> {
        match env::var_os("TMO_CONFIG") {
            Some(path) => Self::load_required_from_path(Path::new(&path)),
            None => Self::load_from_path(Path::new("config.toml")),
        }
    }

    pub fn load_from_path(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }

        let raw = fs::read_to_string(path)
            .with_context(|| format!("failed to read config from {}", path.display()))?;

        toml::from_str(&raw)
            .with_context(|| format!("failed to parse config from {}", path.display()))
    }

    pub fn load_required_from_path(path: &Path) -> Result<Self> {
        let raw = fs::read_to_string(path)
            .with_context(|| format!("failed to read config from {}", path.display()))?;

        toml::from_str(&raw)
            .with_context(|| format!("failed to parse config from {}", path.display()))
    }

    pub fn runtime_storage_config(&self) -> Result<crate::storage::StorageConfig> {
        let journal_mode = parse_journal_mode(&self.storage.sqlite_journal_mode)?;
        let synchronous = parse_synchronous_mode(&self.storage.sqlite_synchronous)?;

        Ok(crate::storage::StorageConfig {
            busy_timeout: std::time::Duration::from_millis(self.storage.sqlite_busy_timeout_ms),
            journal_mode,
            synchronous,
            temp_store: TempStoreMode::Memory,
            foreign_keys: true,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MlServerConfig {
    pub base_url: String,
}

impl Default for MlServerConfig {
    fn default() -> Self {
        Self {
            base_url: "http://localhost:11434".to_owned(),
        }
    }
}

fn parse_journal_mode(raw: &str) -> Result<JournalMode> {
    match raw.trim().to_ascii_uppercase().as_str() {
        "DELETE" => Ok(JournalMode::Delete),
        "WAL" => Ok(JournalMode::Wal),
        other => anyhow::bail!("unsupported sqlite_journal_mode `{other}`"),
    }
}

fn parse_synchronous_mode(raw: &str) -> Result<SynchronousMode> {
    match raw.trim().to_ascii_uppercase().as_str() {
        "OFF" => Ok(SynchronousMode::Off),
        "NORMAL" => Ok(SynchronousMode::Normal),
        "FULL" => Ok(SynchronousMode::Full),
        other => anyhow::bail!("unsupported sqlite_synchronous `{other}`"),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TelegramConfig {
    pub bot_token: Option<String>,
    pub polling: bool,
    pub admin_user_ids: Vec<i64>,
    pub primary_chat_ids: Vec<i64>,
    pub allowed_webhook_hosts: Vec<String>,
}

impl Default for TelegramConfig {
    fn default() -> Self {
        Self {
            bot_token: None,
            polling: true,
            admin_user_ids: Vec::new(),
            primary_chat_ids: Vec::new(),
            allowed_webhook_hosts: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PathsConfig {
    pub data_dir: PathBuf,
    pub database_path: PathBuf,
    pub units_dir: PathBuf,
    pub scripts_dir: PathBuf,
    pub templates_dir: PathBuf,
    pub log_dir: PathBuf,
}

impl Default for PathsConfig {
    fn default() -> Self {
        let data_dir = PathBuf::from("data");
        Self {
            database_path: data_dir.join("runtime.sqlite3"),
            units_dir: PathBuf::from("units"),
            scripts_dir: PathBuf::from("scripts"),
            templates_dir: PathBuf::from("templates"),
            log_dir: data_dir.join("logs"),
            data_dir,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ConfigStorage {
    pub sqlite_journal_mode: String,
    pub sqlite_synchronous: String,
    pub sqlite_busy_timeout_ms: u64,
}

impl Default for ConfigStorage {
    fn default() -> Self {
        Self {
            sqlite_journal_mode: "WAL".to_owned(),
            sqlite_synchronous: "NORMAL".to_owned(),
            sqlite_busy_timeout_ms: 5000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RuntimeConfig {
    pub tokio_worker_threads: Option<usize>,
    pub shutdown_grace_period_ms: u64,
    pub reload_enabled: bool,
    pub manual_mode_enabled: bool,
    pub degraded_mode_enabled: bool,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            tokio_worker_threads: None,
            shutdown_grace_period_ms: 10_000,
            reload_enabled: true,
            manual_mode_enabled: false,
            degraded_mode_enabled: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LimitsConfig {
    pub max_message_text_bytes: usize,
    pub max_caption_bytes: usize,
    pub max_callback_data_bytes: usize,
    pub max_username_bytes: usize,
    pub max_units_per_event: usize,
    pub max_pipeline_depth: usize,
    pub max_batch_ops: usize,
    pub max_queue_depth_ingest: usize,
    pub max_queue_depth_dispatch: usize,
}

impl Default for LimitsConfig {
    fn default() -> Self {
        Self {
            max_message_text_bytes: 16_384,
            max_caption_bytes: 4_096,
            max_callback_data_bytes: 256,
            max_username_bytes: 128,
            max_units_per_event: 16,
            max_pipeline_depth: 4,
            max_batch_ops: 16,
            max_queue_depth_ingest: 2_048,
            max_queue_depth_dispatch: 1_024,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FetchPolicyConfig {
    pub enabled: bool,
    pub deny_private_ip_ranges: bool,
    pub deny_localhost: bool,
    pub max_concurrent_fetches: usize,
    pub connect_timeout_ms: u64,
    pub request_timeout_ms: u64,
    pub max_response_body_bytes: usize,
    pub max_decompressed_body_bytes: usize,
    pub max_redirects: usize,
    pub allowed_domains: Vec<String>,
    pub blocked_domains: Vec<String>,
}

impl Default for FetchPolicyConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            deny_private_ip_ranges: true,
            deny_localhost: true,
            max_concurrent_fetches: 32,
            connect_timeout_ms: 1_500,
            request_timeout_ms: 5_000,
            max_response_body_bytes: 1_048_576,
            max_decompressed_body_bytes: 4_194_304,
            max_redirects: 3,
            allowed_domains: Vec::new(),
            blocked_domains: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SchedulerConfig {
    pub tick_interval_ms: u64,
    pub max_concurrent_jobs: usize,
    pub max_scheduler_lag_ms: u64,
    pub retry_backoff_base_ms: u64,
    pub retry_backoff_max_ms: u64,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            tick_interval_ms: 500,
            max_concurrent_jobs: 32,
            max_scheduler_lag_ms: 10_000,
            retry_backoff_base_ms: 1_000,
            retry_backoff_max_ms: 60_000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ObservabilityConfig {
    pub log_level: String,
    pub json_logs: bool,
    pub metrics_enabled: bool,
    pub trace_sampling: String,
}

impl Default for ObservabilityConfig {
    fn default() -> Self {
        Self {
            log_level: "info".to_owned(),
            json_logs: true,
            metrics_enabled: true,
            trace_sampling: "low".to_owned(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FeatureFlags {
    pub hot_reload: bool,
    pub semantic: bool,
    pub bloom_prefilter: bool,
}

impl Default for FeatureFlags {
    fn default() -> Self {
        Self {
            hot_reload: true,
            semantic: true,
            bloom_prefilter: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::AppConfig;
    use std::fs;

    #[test]
    fn missing_config_path_uses_defaults() {
        let base = std::env::temp_dir().join(format!(
            "telegram-moderation-os-missing-{}",
            std::process::id()
        ));
        let path = base.join("config.toml");
        let config = AppConfig::load_from_path(&path).expect("default config");

        assert!(config.telegram.polling);
        assert_eq!(config.storage.sqlite_journal_mode, "WAL");
    }

    #[test]
    fn config_file_overrides_defaults() {
        let base = std::env::temp_dir().join(format!(
            "telegram-moderation-os-config-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        fs::create_dir_all(&base).expect("temp config dir");
        let path = base.join("config.toml");
        let body = r#"
[telegram]
bot_token = "token"
polling = false
admin_user_ids = [1, 2]
primary_chat_ids = [-100]
allowed_webhook_hosts = ["example.com"]

[paths]
data_dir = "state"
database_path = "state/runtime.sqlite3"
units_dir = "units"
scripts_dir = "scripts"
templates_dir = "templates"
log_dir = "logs"

[storage]
sqlite_journal_mode = "WAL"
sqlite_synchronous = "NORMAL"
sqlite_busy_timeout_ms = 1000

[runtime]
tokio_worker_threads = 2
shutdown_grace_period_ms = 500
reload_enabled = false
manual_mode_enabled = true
degraded_mode_enabled = false

[ml_server]
base_url = "http://127.0.0.1:11434"

[limits]
max_message_text_bytes = 100
max_caption_bytes = 101
max_callback_data_bytes = 102
max_username_bytes = 103
max_units_per_event = 3
max_pipeline_depth = 4
max_batch_ops = 5
max_queue_depth_ingest = 6
max_queue_depth_dispatch = 7

[fetch_policy]
enabled = false
deny_private_ip_ranges = true
deny_localhost = true
max_concurrent_fetches = 8
connect_timeout_ms = 9
request_timeout_ms = 10
max_response_body_bytes = 11
max_decompressed_body_bytes = 12
max_redirects = 13
allowed_domains = ["allowed.example"]
blocked_domains = ["blocked.example"]

[scheduler]
tick_interval_ms = 14
max_concurrent_jobs = 15
max_scheduler_lag_ms = 16
retry_backoff_base_ms = 17
retry_backoff_max_ms = 18

[observability]
log_level = "debug"
json_logs = false
metrics_enabled = false
trace_sampling = "off"

[features]
hot_reload = false
semantic = false
bloom_prefilter = false
"#;

        fs::write(&path, body).expect("config file");
        let config = AppConfig::load_from_path(&path).expect("parsed config");

        assert_eq!(config.telegram.bot_token.as_deref(), Some("token"));
        assert_eq!(config.runtime.tokio_worker_threads, Some(2));
        assert_eq!(config.ml_server.base_url, "http://127.0.0.1:11434");
        assert_eq!(config.observability.log_level, "debug");
        assert!(!config.features.hot_reload);

        let _ = fs::remove_file(&path);
        let _ = fs::remove_dir(&base);
    }

    #[test]
    fn partial_config_file_uses_section_defaults() {
        let base = std::env::temp_dir().join(format!(
            "telegram-moderation-os-partial-config-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        fs::create_dir_all(&base).expect("temp config dir");
        let path = base.join("config.toml");
        let body = r#"
[observability]
log_level = "warn"
"#;

        fs::write(&path, body).expect("config file");
        let config = AppConfig::load_from_path(&path).expect("parsed config");

        assert_eq!(config.observability.log_level, "warn");
        assert!(config.telegram.polling);
        assert_eq!(config.storage.sqlite_journal_mode, "WAL");
        assert_eq!(config.ml_server.base_url, "http://localhost:11434");
        assert!(config.observability.json_logs);

        let _ = fs::remove_file(&path);
        let _ = fs::remove_dir(&base);
    }

    #[test]
    fn explicit_config_path_missing_returns_error() {
        let base = std::env::temp_dir().join(format!(
            "telegram-moderation-os-missing-explicit-config-{}",
            std::process::id()
        ));
        let path = base.join("missing-config.toml");

        let error = AppConfig::load_required_from_path(&path).expect_err("missing file must fail");
        assert!(error.to_string().contains("failed to read config"));
    }

    #[test]
    fn runtime_storage_config_rejects_invalid_modes() {
        let mut config = AppConfig::default();
        config.storage.sqlite_journal_mode = "bogus".to_owned();

        let error = config
            .runtime_storage_config()
            .expect_err("invalid journal mode must fail");
        assert!(error.to_string().contains("sqlite_journal_mode"));

        config.storage.sqlite_journal_mode = "WAL".to_owned();
        config.storage.sqlite_synchronous = "bogus".to_owned();

        let error = config
            .runtime_storage_config()
            .expect_err("invalid synchronous mode must fail");
        assert!(error.to_string().contains("sqlite_synchronous"));
    }
}
