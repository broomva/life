//! Batched lifed gRPC clients (DataLoader pattern).
//!
//! Real implementation lands in Task 8. This file ships a stub so
//! sibling modules compile in CI during the M7-W.1 sub-phase.

#[derive(Default)]
pub struct LifedDataloaders {
    // Real fields added in Task 8.
}

impl LifedDataloaders {
    /// Test-only no-op dataloader. Real construction goes through
    /// `LifedDataloaders::new(channel)` (Task 8).
    #[cfg(test)]
    pub fn test_stub() -> Self {
        Self::default()
    }
}
