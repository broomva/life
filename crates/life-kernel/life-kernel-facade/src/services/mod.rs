//! Generic tonic service trait adapters — each generic over its
//! port trait so `lifed` can plug in proxy impls (arcand, lagod) or
//! in-process impls (PolicyGate from life-kernel-gate).

pub mod approvals;
pub mod events;
pub mod policy;
pub mod session;
pub mod v0_2;
