/// Scheduler configuration.
///
/// The scheduler polls the `jobs` table for due entries on every tick and
/// delegates execution to [`crate::runtime::Runtime`]. Configuration lives
/// here; the actual async tick loop runs inside [`crate::runtime::Runtime::run_scheduler_loop`].
#[derive(Debug, Clone)]
pub struct Scheduler {
    tick_interval_ms: u64,
    max_concurrent_jobs: usize,
}

impl Scheduler {
    pub fn new(tick_interval_ms: u64, max_concurrent_jobs: usize) -> Self {
        Self {
            tick_interval_ms,
            max_concurrent_jobs,
        }
    }

    pub fn tick_interval_ms(&self) -> u64 {
        self.tick_interval_ms
    }

    pub fn max_concurrent_jobs(&self) -> usize {
        self.max_concurrent_jobs
    }
}
