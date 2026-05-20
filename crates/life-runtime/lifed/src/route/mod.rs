//! lifed routes — name-keyed resolvers that bridge public-plane RPCs
//! onto substrate primitives without taking a hard dependency on the
//! substrate's runtime crates.
//!
//! Each submodule owns one primitive's resolution-and-execution glue:
//!
//! | Submodule | Primitive | Spec |
//! |---|---|---|
//! | [`ergon`] | Ergon agent harness | `docs/superpowers/specs/2026-05-05-ergon-v0.1.md` §12.8 |
//!
//! Routes are deliberately substrate-agnostic at this layer — they
//! define a minimal trait (e.g. [`ergon::ErgonRegistry`]) that a host
//! wires up at bootstrap. The lifed dependency-rule script
//! (`scripts/verify_dependencies_lifed.sh`, Spec C₂ §11) forbids lifed
//! from pulling `arcan-core`, `aios-runtime`, or any other substrate
//! runtime crate, so the production wiring of any route's trait lives
//! outside this crate (typically inside arcand, in a layer that already
//! holds the runtime substrate handles).

pub mod ergon;
