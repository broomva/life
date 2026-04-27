//! Saga driver + sub-phase C in-memory registry.
//!
//! - `driver`   — runs `Vec<Box<dyn SagaStep>>` with reverse compensation.
//! - `steps`    — the four `CreateSession` saga steps (Spec C₂ §4.2).
//! - `registry` — tracks inflight + recently-completed sagas so the
//!   admin-plane `Saga.Show` / `ListInflight` RPCs have a reader.

pub mod driver;
pub mod registry;
pub mod steps;

pub use driver::{
    InMemorySagaJournal, LagoSagaJournal, SagaCtx, SagaDriver, SagaError, SagaEvent, SagaEventType,
    SagaJournal, SagaStep,
};
pub use registry::{SagaRecord, SagaRegistry, SagaStatus};
