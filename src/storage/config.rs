use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorageConfig {
    pub busy_timeout: Duration,
    pub journal_mode: JournalMode,
    pub synchronous: SynchronousMode,
    pub temp_store: TempStoreMode,
    pub foreign_keys: bool,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            busy_timeout: Duration::from_secs(5),
            journal_mode: JournalMode::Wal,
            synchronous: SynchronousMode::Normal,
            temp_store: TempStoreMode::Memory,
            foreign_keys: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JournalMode {
    Delete,
    Wal,
}

impl JournalMode {
    pub fn as_sql(self) -> &'static str {
        match self {
            Self::Delete => "DELETE",
            Self::Wal => "WAL",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SynchronousMode {
    Off,
    Normal,
    Full,
}

impl SynchronousMode {
    pub fn as_sql(self) -> &'static str {
        match self {
            Self::Off => "OFF",
            Self::Normal => "NORMAL",
            Self::Full => "FULL",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TempStoreMode {
    Default,
    File,
    Memory,
}

impl TempStoreMode {
    pub fn as_sql(self) -> &'static str {
        match self {
            Self::Default => "DEFAULT",
            Self::File => "FILE",
            Self::Memory => "MEMORY",
        }
    }
}
