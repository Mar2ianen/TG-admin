use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub telegram: TelegramConfig,
    pub paths: PathsConfig,
    pub storage: StorageConfig,
    pub runtime: RuntimeConfig,
    pub limits: LimitsConfig,
    pub fetch_policy: FetchPolicyConfig,
    pub scheduler: SchedulerConfig,
    pub observability: ObservabilityConfig,
    pub features: FeatureFlags,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            telegram: TelegramConfig::default(),
            paths: PathsConfig::default(),
            storage: StorageConfig::default(),
            runtime: RuntimeConfig::default(),
            limits: LimitsConfig::default(),
            fetch_policy: FetchPolicyConfig::default(),
            scheduler: SchedulerConfig::default(),
            observability: ObservabilityConfig::default(),
            features: FeatureFlags::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
pub struct StorageConfig {
    pub sqlite_journal_mode: String,
    pub sqlite_synchronous: String,
    pub sqlite_busy_timeout_ms: u64,
    pub max_write_batch_size: usize,
    pub write_flush_interval_ms: u64,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            sqlite_journal_mode: "WAL".to_owned(),
            sqlite_synchronous: "NORMAL".to_owned(),
            sqlite_busy_timeout_ms: 3_000,
            max_write_batch_size: 256,
            write_flush_interval_ms: 5_000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
