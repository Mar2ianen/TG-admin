/// Configuration holder for the audit subsystem.
///
/// Actual audit log entries are written directly to SQLite via
/// [`crate::storage::StorageConnection::append_audit_entry`] inside
/// [`crate::moderation::ModerationEngine`]. This struct is retained as a
/// future hook for pluggable audit backends and as a summary flag for
/// [`crate::runtime::RuntimeSummary::audit_enabled`].
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
