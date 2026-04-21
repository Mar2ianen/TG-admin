#[derive(Debug, Clone)]
pub struct AuditService {
    enabled: bool,
}

impl AuditService {
    pub fn new(enabled: bool) -> Self {
        Self { enabled }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }
}
