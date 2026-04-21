#[derive(Debug, Clone)]
pub struct Scheduler {
    tick_interval_ms: u64,
}

impl Scheduler {
    pub fn new(tick_interval_ms: u64) -> Self {
        Self { tick_interval_ms }
    }

    pub fn tick_interval_ms(&self) -> u64 {
        self.tick_interval_ms
    }
}
