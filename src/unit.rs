#![allow(dead_code)]

#[derive(Debug, Clone, Default)]
pub struct UnitRegistry {
    entries: Vec<UnitDescriptor>,
}

impl UnitRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn status_summary(&self) -> UnitRegistryStatus {
        UnitRegistryStatus {
            total_units: self.entries.len(),
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[derive(Debug, Clone)]
pub struct UnitDescriptor {
    pub id: String,
    pub status: UnitStatus,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum UnitStatus {
    Active,
    Failed,
    Disabled,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct UnitRegistryStatus {
    pub total_units: usize,
}
