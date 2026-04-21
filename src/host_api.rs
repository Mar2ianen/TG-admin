#[derive(Debug, Clone)]
pub struct HostApi {
    dry_run: bool,
}

impl HostApi {
    pub fn new(dry_run: bool) -> Self {
        Self { dry_run }
    }

    pub fn dry_run(&self) -> bool {
        self.dry_run
    }
}
