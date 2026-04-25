//! Typed service handles over the `life-kernel-proto` generated client
//! stubs. Each handle exposes ergonomic methods that take/return
//! `aios-protocol` canonical types where feasible; the Kernel handle
//! stays at the proto layer since its wire types carry complex spec
//! structures that would be noisy to unwrap on the client side.

pub mod approvals;
pub mod events;
pub mod kernel;
pub mod policy;
pub mod session;

pub use approvals::Approvals;
pub use events::Events;
pub use kernel::Kernel;
pub use policy::Policy;
pub use session::Session;
