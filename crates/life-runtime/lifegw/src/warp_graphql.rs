//! Warp-compatible GraphQL surface (Spec C₃ M7-W).
//!
//! This module exposes a verbatim mirror of Warp's upstream GraphQL
//! schema, with resolvers that translate to internal `lifed` gRPC.
//! Phase α scope: the adapter compiles, schema introspection is
//! byte-identical to upstream, all operations resolve (T1+T2 with real
//! data, T3 with safe defaults), JWT auth gates every call.
//!
//! See `core/life/docs/superpowers/specs/2026-05-10-lumen-warp-on-life-design.md`
//! for the design contract.

pub mod adapter;
pub mod auth;
pub mod context;
pub mod dataloader;
pub mod error;
pub mod resolvers;
pub mod route;
pub mod schema_mirror;
pub mod subscriptions;

pub use adapter::WarpSchema;
pub use error::WarpGraphqlError;
