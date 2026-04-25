mod config;
mod connection;
mod error;
mod helpers;
mod schema;
mod tests;
mod types;

pub use config::*;
pub use connection::*;
pub use error::*;
pub use helpers::*;
pub use schema::*;
pub use types::*;

use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Storage {
    database_path: PathBuf,
    config: StorageConfig,
}

impl Storage {
    pub fn new(database_path: PathBuf) -> Self {
        Self {
            database_path,
            config: StorageConfig::default(),
        }
    }

    pub fn with_config(database_path: PathBuf, config: StorageConfig) -> Self {
        Self {
            database_path,
            config,
        }
    }

    pub fn database_path(&self) -> &Path {
        &self.database_path
    }

    pub fn config(&self) -> &StorageConfig {
        &self.config
    }

    pub fn open(&self) -> Result<StorageConnection, StorageError> {
        StorageConnection::open(self.database_path.clone(), self.config.clone())
    }

    pub fn init(&self) -> Result<StorageConnection, StorageError> {
        let mut connection = self.open()?;
        connection.init_schema()?;
        Ok(connection)
    }

    pub fn bootstrap(&self) -> Result<StorageBootstrap, StorageError> {
        let mut connection = self.open()?;
        let migration = connection.init_schema()?;

        Ok(StorageBootstrap {
            connection,
            migration,
        })
    }
}

#[derive(Debug)]
pub struct StorageBootstrap {
    connection: StorageConnection,
    migration: MigrationResult,
}

impl StorageBootstrap {
    pub fn connection(&self) -> &StorageConnection {
        &self.connection
    }

    pub fn connection_mut(&mut self) -> &mut StorageConnection {
        &mut self.connection
    }

    pub fn into_connection(self) -> StorageConnection {
        self.connection
    }

    pub fn migration(&self) -> &MigrationResult {
        &self.migration
    }
}
