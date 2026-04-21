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
use crate::host_api::HostApi;
use crate::scheduler::Scheduler;
use crate::storage::Storage;
use crate::tg::TelegramGateway;
use crate::unit::UnitRegistry;

fn main() {
    let _config = AppConfig::default();
    let _storage = Storage::new();
    let _units = UnitRegistry::new();
    let _audit = AuditService::new();
    let _scheduler = Scheduler::new();
    let _tg = TelegramGateway::new();
    let _host_api = HostApi::new();
}
