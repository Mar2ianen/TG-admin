use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Storage {
    database_path: PathBuf,
}

impl Storage {
    pub fn new(database_path: PathBuf) -> Self {
        Self { database_path }
    }

    pub fn database_path(&self) -> &PathBuf {
        &self.database_path
    }
}
