use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationResult {
    pub previous_version: u32,
    pub current_version: u32,
    pub applied_versions: Vec<u32>,
}

impl MigrationResult {
    pub fn changed(&self) -> bool {
        !self.applied_versions.is_empty()
    }
}

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("failed to create storage directory `{path}`")]
    CreateDirectory {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("sqlite error")]
    Sqlite(#[from] rusqlite::Error),
    #[error("unsupported schema version {found}; supported up to {supported}")]
    UnsupportedSchemaVersion { found: u32, supported: u32 },
    #[error("invalid processed update status `{status}`")]
    InvalidProcessedUpdateStatus { status: String },
    #[error("expected persisted row in `{0}` after write")]
    MissingRow(&'static str),
}
