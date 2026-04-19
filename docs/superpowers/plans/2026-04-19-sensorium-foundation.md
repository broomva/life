# Sensorium Foundation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Land the Sensorium foundation in `core/life/`: add `ExternalToL0` boundary marker to Pneuma, scaffold `sensorium-core` with all type definitions, and implement `sensorium-fabric` as an in-process `Pneuma<ExternalToL0>` substrate. Produces a working, testable perception fabric with a MockPublisher integration test.

**Architecture:** Sensorium-fabric is a typed in-process pub/sub with QoS, backed by `tokio::sync::broadcast` channels. One fabric holds all subscriptions; signals match against `KeyExpr` patterns and route to matching subscribers. The Pneuma impl is a thin wrapper over the fabric's core operations. Heavy concerns (schema registry, persistence, Arcan bridge, concrete publishers) are follow-up plans.

**Tech Stack:** Rust 2024 Edition (MSRV 1.85), tokio, async-trait, serde, chrono, thiserror, uuid. Follows existing `core/life/` conventions.

**Prerequisites:**
- Pneuma trait must exist at `crates/aios/aios-protocol/src/pneuma.rs` (tracked in `docs/specs/pneuma-trait-surface.md`). Verify with:
  ```bash
  test -f crates/aios/aios-protocol/src/pneuma.rs && grep -q "pub trait Pneuma" crates/aios/aios-protocol/src/pneuma.rs && echo OK
  ```
  If this fails, land the Pneuma trait per its spec first. Do not proceed.

**Scope — what this plan does NOT cover** (each has its own follow-up plan):
- `arcan-sensorium` bridge (Phase 2)
- `sensorium-lago` tiered persistence (Phase 2)
- `sensorium-qos` full negotiation matrix (subset included here; companion spec TBD)
- `sensorium-schema` tag catalog (Phase 5)
- Concrete publishers (Phases 3–7: opsis, screen, modbus, ble)
- `sensoriumd` daemon (Phase 8)

---

## File Structure

```
crates/aios/aios-protocol/src/pneuma.rs    # MODIFY — add ExternalToL0 + reserved markers

crates/sensorium/                          # NEW — entire tree
├── sensorium-core/
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs                         # Module re-exports + crate docs
│       ├── key_expr.rs                    # KeyExpr with wildcard matching
│       ├── source.rs                      # SourceId, SourceDescriptor, SourceState, LifecycleState
│       ├── quality.rs                     # SignalQuality, QualityStatus, TimestampQuality
│       ├── signal_kind.rs                 # SignalKind enum, Payload, DataEncoding
│       ├── signal.rs                      # SensorySignal
│       ├── perception.rs                  # PerceptionState, SourceStatus, QualitySummary
│       ├── directive.rs                   # AttentionDirective, RatePolicy, QosRequirement
│       ├── publisher.rs                   # Publisher trait, PublisherError, SourceId
│       ├── transformer.rs                 # Transformer trait, ComputeCost, TransformerError
│       ├── lifecycle.rs                   # BirthCertificate, TopicDeclaration, QosProfile (minimal)
│       └── error.rs                       # SensoriumError taxonomy
│
└── sensorium-fabric/
    ├── Cargo.toml
    └── src/
        ├── lib.rs
        ├── registry.rs                    # Subscription registry, KeyExpr matching
        ├── fabric.rs                      # SensoriumFabric + emit/aggregate/receive
        ├── pneuma_impl.rs                 # impl Pneuma<ExternalToL0> for SensoriumFabric
        └── tests/
            └── integration.rs             # End-to-end test with MockPublisher

Cargo.toml (workspace)                     # MODIFY — add two crates to members
```

---

## Task 1: Add ExternalToL0 boundary + reserved markers

**Files:**
- Modify: `crates/aios/aios-protocol/src/pneuma.rs`
- Test: same file (inline `#[cfg(test)]` module)

- [ ] **Step 1: Write failing tests for the new markers**

Append to `crates/aios/aios-protocol/src/pneuma.rs` test module:

```rust
#[test]
fn external_to_l0_boundary_identity() {
    assert_eq!(ExternalToL0::axis_name(), "vertical");
    assert_eq!(ExternalToL0::boundary_name(), "external → L0");
}

#[test]
fn reserved_markers_exist_but_are_not_boundaries() {
    // These types must exist (compilation check); they intentionally
    // do not impl Boundary until their owning crates exist.
    let _: PhantomData<ExternalToL1>;
    let _: PhantomData<ExternalToL2>;
    let _: PhantomData<ExternalToL3>;
    let _: PhantomData<PerceptToField>;
    let _: PhantomData<ExternalToD1>;
    let _: PhantomData<FieldToPercept>;
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p aios-protocol pneuma::`
Expected: FAIL — `cannot find type 'ExternalToL0'` and related errors.

- [ ] **Step 3: Add the marker types**

Append to `crates/aios/aios-protocol/src/pneuma.rs` (above the test module):

```rust
/// The boundary between the external world and the agent's L0 plant.
/// Vertical axis, below L0. Crossed by all sensory input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExternalToL0;

impl Boundary for ExternalToL0 {
    fn axis_name() -> &'static str {
        "vertical"
    }
    fn boundary_name() -> &'static str {
        "external → L0"
    }
}

/// Reserved: direct external feeds into L1 homeostatic state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExternalToL1;

/// Reserved: external feeds into L2 meta-control.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExternalToL2;

/// Reserved: external feeds into L3 governance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExternalToL3;

/// Reserved: shared perception within a formation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PerceptToField;

/// Reserved: depth-1 Sensorium — hive-level perception.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExternalToD1;

/// Reserved: environmental field effects feeding back into individual perception.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FieldToPercept;
```

Ensure `use std::marker::PhantomData;` is in scope for tests.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p aios-protocol pneuma::`
Expected: PASS. Also run `cargo clippy -p aios-protocol -- -D warnings` and `cargo fmt`.

- [ ] **Step 5: Commit**

```bash
git add crates/aios/aios-protocol/src/pneuma.rs
git commit -m "feat(aios-protocol): add ExternalToL0 boundary + reserve L1/L2/L3/field markers

Sensorium implements Pneuma<ExternalToL0>. Reserved markers document
intent for future external boundaries (homeostatic/eval/governance feeds,
multi-agent perception) without committing to impls yet.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: Scaffold sensorium-core crate

**Files:**
- Create: `crates/sensorium/sensorium-core/Cargo.toml`
- Create: `crates/sensorium/sensorium-core/src/lib.rs`
- Modify: `Cargo.toml` (workspace members)

- [ ] **Step 1: Create the Cargo.toml**

Create `crates/sensorium/sensorium-core/Cargo.toml`:

```toml
[package]
name = "sensorium-core"
version.workspace = true
edition.workspace = true
license.workspace = true
authors.workspace = true
repository.workspace = true
description = "Sensorium core types — SensorySignal, PerceptionState, AttentionDirective, Publisher/Transformer traits for the Life Agent OS perception substrate."

[dependencies]
aios-protocol.workspace = true
async-trait.workspace = true
chrono.workspace = true
serde.workspace = true
serde_json.workspace = true
thiserror.workspace = true
uuid = { workspace = true, features = ["v4", "serde"] }

[dev-dependencies]
tokio = { workspace = true, features = ["rt", "macros"] }

[lints]
workspace = true
```

- [ ] **Step 2: Create minimal lib.rs**

Create `crates/sensorium/sensorium-core/src/lib.rs`:

```rust
//! Sensorium core — types and traits for the perception substrate of the
//! Life Agent OS.
//!
//! This crate defines the Pneuma<ExternalToL0> associated types
//! (`SensorySignal`, `PerceptionState`, `AttentionDirective`) plus the
//! internal contracts for publishers and transformers that feed the
//! fabric.
//!
//! See `core/life/docs/specs/sensorium-architecture.md` for the full
//! architectural context.

#![forbid(unsafe_code)]

// Module stubs — each gets implemented in a dedicated task.
pub mod error;
```

- [ ] **Step 3: Add error module with minimal content**

Create `crates/sensorium/sensorium-core/src/error.rs`:

```rust
//! Error taxonomy for Sensorium.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum SensoriumError {
    #[error("invalid key expression: {0}")]
    InvalidKeyExpr(String),

    #[error("schema mismatch for key {key}: {reason}")]
    SchemaMismatch { key: String, reason: String },

    #[error("subscription exhausted (backpressure)")]
    Backpressure,

    #[error("source not connected: {0}")]
    NotConnected(String),

    #[error("transport error: {0}")]
    Transport(String),
}
```

- [ ] **Step 4: Register in workspace**

Modify `Cargo.toml` (workspace root) — find the `members = [` list and insert (maintaining alphabetical grouping by pillar):

```toml
    # Sensorium — perception substrate
    "crates/sensorium/sensorium-core",
    "crates/sensorium/sensorium-fabric",
```

Verify with: `cargo check -p sensorium-core`
Expected: clean compile.

- [ ] **Step 5: Commit**

```bash
git add crates/sensorium/sensorium-core Cargo.toml
git commit -m "feat(sensorium-core): scaffold crate with error taxonomy

First of the Sensorium crate family. Empty module scaffold will be filled
in by subsequent tasks (KeyExpr, SourceDescriptor, SignalQuality, etc).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: Implement KeyExpr

**Files:**
- Create: `crates/sensorium/sensorium-core/src/key_expr.rs`
- Modify: `crates/sensorium/sensorium-core/src/lib.rs`

- [ ] **Step 1: Write failing tests**

Create `crates/sensorium/sensorium-core/src/key_expr.rs`:

```rust
//! Hierarchical topic path with wildcard support. Inspired by Zenoh key
//! expressions. Segments separated by `/`. Wildcards: `*` matches one
//! segment, `**` matches zero or more segments.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::SensoriumError;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct KeyExpr(String);

impl KeyExpr {
    pub fn new(raw: impl Into<String>) -> Result<Self, SensoriumError> {
        let s = raw.into();
        if s.is_empty() {
            return Err(SensoriumError::InvalidKeyExpr("empty".into()));
        }
        if s.contains("//") {
            return Err(SensoriumError::InvalidKeyExpr(format!(
                "consecutive slashes: {s}"
            )));
        }
        if s.starts_with('/') || s.ends_with('/') {
            return Err(SensoriumError::InvalidKeyExpr(format!(
                "leading/trailing slash: {s}"
            )));
        }
        Ok(Self(s))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns true when `self` is a pattern that matches `concrete`.
    pub fn matches(&self, concrete: &KeyExpr) -> bool {
        matches_segments(self.0.split('/').collect(), concrete.0.split('/').collect())
    }
}

fn matches_segments(pattern: Vec<&str>, concrete: Vec<&str>) -> bool {
    let (mut pi, mut ci) = (0, 0);
    while pi < pattern.len() {
        match pattern[pi] {
            "**" => {
                if pi == pattern.len() - 1 {
                    return true;
                }
                for start in ci..=concrete.len() {
                    if matches_segments(pattern[pi + 1..].to_vec(), concrete[start..].to_vec()) {
                        return true;
                    }
                }
                return false;
            }
            "*" => {
                if ci >= concrete.len() {
                    return false;
                }
                pi += 1;
                ci += 1;
            }
            lit => {
                if ci >= concrete.len() || concrete[ci] != lit {
                    return false;
                }
                pi += 1;
                ci += 1;
            }
        }
    }
    ci == concrete.len()
}

impl fmt::Display for KeyExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(s: &str) -> KeyExpr {
        KeyExpr::new(s).expect("valid key")
    }

    #[test]
    fn rejects_empty_and_malformed() {
        assert!(KeyExpr::new("").is_err());
        assert!(KeyExpr::new("/leading").is_err());
        assert!(KeyExpr::new("trailing/").is_err());
        assert!(KeyExpr::new("double//slash").is_err());
    }

    #[test]
    fn literal_match() {
        assert!(k("plant/t1/rpm").matches(&k("plant/t1/rpm")));
        assert!(!k("plant/t1/rpm").matches(&k("plant/t2/rpm")));
    }

    #[test]
    fn single_wildcard_matches_one_segment() {
        assert!(k("plant/*/rpm").matches(&k("plant/t1/rpm")));
        assert!(k("plant/*/rpm").matches(&k("plant/t99/rpm")));
        assert!(!k("plant/*/rpm").matches(&k("plant/t1/bay/rpm")));
    }

    #[test]
    fn double_wildcard_matches_any_tail() {
        assert!(k("plant/**").matches(&k("plant/t1/rpm")));
        assert!(k("plant/**").matches(&k("plant/t1/bay/rpm")));
        assert!(!k("plant/**").matches(&k("other/t1/rpm")));
    }

    #[test]
    fn double_wildcard_matches_empty_tail() {
        assert!(k("plant/**").matches(&k("plant/x")));
    }

    #[test]
    fn double_wildcard_mid_pattern() {
        assert!(k("plant/**/rpm").matches(&k("plant/t1/rpm")));
        assert!(k("plant/**/rpm").matches(&k("plant/t1/bay/rpm")));
        assert!(!k("plant/**/rpm").matches(&k("plant/t1/other")));
    }
}
```

- [ ] **Step 2: Expose module in lib.rs**

Modify `crates/sensorium/sensorium-core/src/lib.rs`:

```rust
// Add to existing module list:
pub mod key_expr;

pub use key_expr::KeyExpr;
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p sensorium-core key_expr`
Expected: 6 tests pass.

- [ ] **Step 4: Lint and format**

Run: `cargo clippy -p sensorium-core -- -D warnings && cargo fmt`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add crates/sensorium/sensorium-core/src/key_expr.rs crates/sensorium/sensorium-core/src/lib.rs
git commit -m "feat(sensorium-core): KeyExpr with wildcard matching

Hierarchical topic path (Zenoh-inspired). '*' matches one segment,
'**' matches zero or more segments. 6 test cases cover literal,
single-wildcard, and double-wildcard cases at arbitrary positions.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: Source identity & lifecycle types

**Files:**
- Create: `crates/sensorium/sensorium-core/src/source.rs`

- [ ] **Step 1: Write failing tests** (include inline with the source file)

Create `crates/sensorium/sensorium-core/src/source.rs`:

```rust
//! Source identity, descriptor, and lifecycle state (Sparkplug-inspired).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SourceId(pub Uuid);

impl SourceId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for SourceId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SourceDescriptor {
    pub source_id: SourceId,
    pub protocol: Protocol,
    pub address: String,
    pub capabilities: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Protocol {
    Screen,
    Audio,
    Ble,
    Modbus,
    OpcUa,
    Mqtt,
    CanBus,
    Serial,
    Http,
    WebSocket,
    Sse,
    OpsisSse,
    Custom(String),
}

/// Sparkplug-inspired lifecycle. Attached to every SensorySignal so L0
/// can distinguish "silent" from "dead".
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SourceState {
    Online,
    Stale { last_seen: DateTime<Utc> },
    Dead { since: DateTime<Utc> },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LifecycleState {
    Birth,
    Data,
    Death { reason: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_id_uniqueness() {
        let a = SourceId::new();
        let b = SourceId::new();
        assert_ne!(a, b);
    }

    #[test]
    fn descriptor_round_trips_json() {
        let d = SourceDescriptor {
            source_id: SourceId::new(),
            protocol: Protocol::Modbus,
            address: "modbus://10.0.1.50:502/unit/1".into(),
            capabilities: vec!["poll".into(), "write".into()],
        };
        let s = serde_json::to_string(&d).unwrap();
        let d2: SourceDescriptor = serde_json::from_str(&s).unwrap();
        assert_eq!(d.source_id, d2.source_id);
        assert_eq!(d.address, d2.address);
    }

    #[test]
    fn source_state_transitions_are_representable() {
        let online = SourceState::Online;
        let stale = SourceState::Stale { last_seen: Utc::now() };
        let dead = SourceState::Dead { since: Utc::now() };
        assert_ne!(online, stale);
        assert_ne!(stale, dead);
    }
}
```

- [ ] **Step 2: Expose in lib.rs**

Add to `crates/sensorium/sensorium-core/src/lib.rs`:

```rust
pub mod source;

pub use source::{LifecycleState, Protocol, SourceDescriptor, SourceId, SourceState};
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p sensorium-core source`
Expected: 3 tests pass.

- [ ] **Step 4: Lint and format**

Run: `cargo clippy -p sensorium-core -- -D warnings && cargo fmt`

- [ ] **Step 5: Commit**

```bash
git add crates/sensorium/sensorium-core/src/source.rs crates/sensorium/sensorium-core/src/lib.rs
git commit -m "feat(sensorium-core): source identity, descriptor, and lifecycle state

SourceId (UUID v4), SourceDescriptor with Protocol enum covering the
publisher matrix, Sparkplug-inspired SourceState (Online/Stale/Dead)
and LifecycleState for birth/data/death messages.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: Signal quality with OPC-UA-inspired semantics

**Files:**
- Create: `crates/sensorium/sensorium-core/src/quality.rs`

- [ ] **Step 1: Write failing tests in the module**

Create `crates/sensorium/sensorium-core/src/quality.rs`:

```rust
//! Signal quality — OPC-UA-inspired semantics applied universally.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum QualityStatus {
    Good,
    GoodLocalOverride,
    Uncertain,
    UncertainSensorNotAccurate,
    Bad,
    BadSensorFailure,
    BadCommunicationFailure,
    BadConfigurationError,
}

impl QualityStatus {
    pub fn is_good(&self) -> bool {
        matches!(self, Self::Good | Self::GoodLocalOverride)
    }

    pub fn is_bad(&self) -> bool {
        matches!(
            self,
            Self::Bad
                | Self::BadSensorFailure
                | Self::BadCommunicationFailure
                | Self::BadConfigurationError
        )
    }

    pub fn is_uncertain(&self) -> bool {
        matches!(self, Self::Uncertain | Self::UncertainSensorNotAccurate)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TimestampQuality {
    Source,
    Interpolated,
    Estimated,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SignalQuality {
    pub status: QualityStatus,
    pub timestamp_quality: TimestampQuality,
    pub confidence: f32,
}

impl SignalQuality {
    pub const GOOD: Self = Self {
        status: QualityStatus::Good,
        timestamp_quality: TimestampQuality::Source,
        confidence: 1.0,
    };

    pub fn bad_communication() -> Self {
        Self {
            status: QualityStatus::BadCommunicationFailure,
            timestamp_quality: TimestampQuality::Estimated,
            confidence: 0.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_predicates_partition_space() {
        for s in [
            QualityStatus::Good,
            QualityStatus::GoodLocalOverride,
            QualityStatus::Uncertain,
            QualityStatus::UncertainSensorNotAccurate,
            QualityStatus::Bad,
            QualityStatus::BadSensorFailure,
            QualityStatus::BadCommunicationFailure,
            QualityStatus::BadConfigurationError,
        ] {
            let bucket_count =
                s.is_good() as u8 + s.is_bad() as u8 + s.is_uncertain() as u8;
            assert_eq!(bucket_count, 1, "{s:?} must belong to exactly one bucket");
        }
    }

    #[test]
    fn good_constant_round_trips() {
        let s = serde_json::to_string(&SignalQuality::GOOD).unwrap();
        let q: SignalQuality = serde_json::from_str(&s).unwrap();
        assert!(q.status.is_good());
    }
}
```

- [ ] **Step 2: Expose in lib.rs**

Add to `lib.rs`:

```rust
pub mod quality;

pub use quality::{QualityStatus, SignalQuality, TimestampQuality};
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p sensorium-core quality`
Expected: 2 tests pass.

- [ ] **Step 4: Lint and format**

Run: `cargo clippy -p sensorium-core -- -D warnings && cargo fmt`

- [ ] **Step 5: Commit**

```bash
git add crates/sensorium/sensorium-core/src/quality.rs crates/sensorium/sensorium-core/src/lib.rs
git commit -m "feat(sensorium-core): SignalQuality with OPC-UA-inspired statuses

QualityStatus enum covers Good/Uncertain/Bad families with matching
predicates. SignalQuality pairs status with timestamp quality and a
confidence score. Predicate test guarantees the status space is
partitioned into exactly three buckets.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 6: SignalKind taxonomy + Payload

**Files:**
- Create: `crates/sensorium/sensorium-core/src/signal_kind.rs`

- [ ] **Step 1: Write the module with inline tests**

Create `crates/sensorium/sensorium-core/src/signal_kind.rs`:

```rust
//! SignalKind taxonomy — domains of reality a signal can come from.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SignalKind {
    // Continuous media
    VideoFrame,
    AudioChunk,
    // Structured text
    Transcription,
    OcrExtraction,
    TextStream,
    // Industrial I/O
    RegisterRead,
    DiscreteInput,
    AnalogMeasurement,
    AlarmEvent,
    // Environmental
    Geospatial,
    Inertial,
    Atmospheric,
    // Digital feeds
    NetworkMessage,
    FileSystemEvent,
    ApiResponse,
    // Agent/system
    WorldStateDelta,
    AgentMessage,
    // Forward-compatible
    Custom(String),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Payload {
    Json(serde_json::Value),
    Text(String),
    Bytes(Vec<u8>),
    F64(f64),
    I64(i64),
    Bool(bool),
    Null,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DataEncoding {
    Raw,
    Utf8,
    Json,
    MsgPack,
    Jpeg,
    Png,
    Hevc,
    OpusAudio,
    Pcm16,
    ModbusPdu,
    OpcUaDataValue,
    Custom(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_signal_kind_round_trips() {
        let k = SignalKind::Custom("sensorium.domain.special".into());
        let s = serde_json::to_string(&k).unwrap();
        let k2: SignalKind = serde_json::from_str(&s).unwrap();
        assert_eq!(k, k2);
    }

    #[test]
    fn payload_variants_serialize_with_tag() {
        // Verifies tagged enum representation (important for forward compat).
        let p = Payload::Json(serde_json::json!({ "rpm": 1850 }));
        let s = serde_json::to_string(&p).unwrap();
        assert!(s.contains("Json"), "expected tag 'Json' in {s}");
    }
}
```

- [ ] **Step 2: Expose in lib.rs**

```rust
pub mod signal_kind;

pub use signal_kind::{DataEncoding, Payload, SignalKind};
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p sensorium-core signal_kind`
Expected: 2 tests pass.

- [ ] **Step 4: Lint and format**

Run: `cargo clippy -p sensorium-core -- -D warnings && cargo fmt`

- [ ] **Step 5: Commit**

```bash
git add crates/sensorium/sensorium-core/src/signal_kind.rs crates/sensorium/sensorium-core/src/lib.rs
git commit -m "feat(sensorium-core): SignalKind taxonomy, Payload, DataEncoding

SignalKind spans media, structured text, industrial I/O, environmental,
digital feeds, and agent/system domains, with Custom(String) for forward
compatibility. Payload is a typed tagged union. DataEncoding names raw
byte formats so perceivers can dispatch on them.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 7: SensorySignal

**Files:**
- Create: `crates/sensorium/sensorium-core/src/signal.rs`

- [ ] **Step 1: Write the module with tests**

Create `crates/sensorium/sensorium-core/src/signal.rs`:

```rust
//! SensorySignal — the payload crossing the external → L0 boundary.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{
    key_expr::KeyExpr,
    quality::SignalQuality,
    signal_kind::{Payload, SignalKind},
    source::SourceState,
};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SensorySignal {
    pub key: KeyExpr,
    pub timestamp: DateTime<Utc>,
    pub kind: SignalKind,
    pub payload: Payload,
    pub quality: SignalQuality,
    pub source_state: SourceState,
    #[serde(default)]
    pub metadata: serde_json::Value,
}

impl SensorySignal {
    /// Constructor for a "good" signal with Online source state. Tests
    /// and simple publishers use this; industrial publishers should build
    /// explicitly to set quality/state precisely.
    pub fn good(key: KeyExpr, kind: SignalKind, payload: Payload) -> Self {
        Self {
            key,
            timestamp: Utc::now(),
            kind,
            payload,
            quality: SignalQuality::GOOD,
            source_state: SourceState::Online,
            metadata: serde_json::Value::Null,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn good_constructor_sets_sensible_defaults() {
        let s = SensorySignal::good(
            KeyExpr::new("plant/t1/rpm").unwrap(),
            SignalKind::AnalogMeasurement,
            Payload::F64(1850.0),
        );
        assert!(s.quality.status.is_good());
        assert_eq!(s.source_state, SourceState::Online);
    }

    #[test]
    fn signal_round_trips_json() {
        let original = SensorySignal::good(
            KeyExpr::new("plant/t1/rpm").unwrap(),
            SignalKind::AnalogMeasurement,
            Payload::F64(1850.0),
        );
        let json = serde_json::to_string(&original).unwrap();
        let decoded: SensorySignal = serde_json::from_str(&json).unwrap();
        assert_eq!(original.key, decoded.key);
        assert_eq!(original.kind, decoded.kind);
    }
}
```

- [ ] **Step 2: Expose in lib.rs**

```rust
pub mod signal;

pub use signal::SensorySignal;
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p sensorium-core signal`
Expected: 2 tests pass.

- [ ] **Step 4: Lint and format**

Run: `cargo clippy -p sensorium-core -- -D warnings && cargo fmt`

- [ ] **Step 5: Commit**

```bash
git add crates/sensorium/sensorium-core/src/signal.rs crates/sensorium/sensorium-core/src/lib.rs
git commit -m "feat(sensorium-core): SensorySignal — the universal perception payload

Every signal carries key + timestamp + kind + payload + quality +
source_state + metadata. Quality and source_state are first-class so
consumers can filter by data reliability and lifecycle without branching
by source type.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 8: PerceptionState aggregate

**Files:**
- Create: `crates/sensorium/sensorium-core/src/perception.rs`

- [ ] **Step 1: Write the module with tests**

Create `crates/sensorium/sensorium-core/src/perception.rs`:

```rust
//! PerceptionState — what L0 observes across all active sources.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{
    key_expr::KeyExpr,
    signal::SensorySignal,
    source::{LifecycleState, SourceId},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PerceptionState {
    pub active_sources: HashMap<SourceId, SourceStatus>,
    pub latest_by_key: HashMap<String, SensorySignal>,
    pub quality_summary: QualitySummary,
    pub sparkplug_state: HashMap<SourceId, LifecycleState>,
    pub captured_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SourceStatus {
    pub source_id: SourceId,
    pub topics_advertised: usize,
    pub signals_emitted: u64,
    pub last_signal_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct QualitySummary {
    pub good: u64,
    pub uncertain: u64,
    pub bad: u64,
}

impl PerceptionState {
    pub fn empty() -> Self {
        Self {
            active_sources: HashMap::new(),
            latest_by_key: HashMap::new(),
            quality_summary: QualitySummary::default(),
            sparkplug_state: HashMap::new(),
            captured_at: Utc::now(),
        }
    }

    pub fn latest(&self, key: &KeyExpr) -> Option<&SensorySignal> {
        self.latest_by_key.get(key.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{signal::SensorySignal, signal_kind::{Payload, SignalKind}};

    #[test]
    fn empty_state_is_empty() {
        let s = PerceptionState::empty();
        assert!(s.active_sources.is_empty());
        assert!(s.latest_by_key.is_empty());
    }

    #[test]
    fn latest_lookup_by_key() {
        let mut s = PerceptionState::empty();
        let key = KeyExpr::new("plant/t1/rpm").unwrap();
        let signal =
            SensorySignal::good(key.clone(), SignalKind::AnalogMeasurement, Payload::F64(1.0));
        s.latest_by_key.insert(key.as_str().to_string(), signal);
        assert!(s.latest(&key).is_some());
        assert!(s.latest(&KeyExpr::new("plant/t2/rpm").unwrap()).is_none());
    }
}
```

- [ ] **Step 2: Expose in lib.rs**

```rust
pub mod perception;

pub use perception::{PerceptionState, QualitySummary, SourceStatus};
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p sensorium-core perception`
Expected: 2 tests pass.

- [ ] **Step 4: Lint and format**

Run: `cargo clippy -p sensorium-core -- -D warnings && cargo fmt`

- [ ] **Step 5: Commit**

```bash
git add crates/sensorium/sensorium-core/src/perception.rs crates/sensorium/sensorium-core/src/lib.rs
git commit -m "feat(sensorium-core): PerceptionState aggregate

The read-side snapshot of all active sources, latest signals per key,
quality histogram, and Sparkplug lifecycle state — what L0 observes
through Pneuma::aggregate().

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 9: AttentionDirective + RatePolicy + QosRequirement

**Files:**
- Create: `crates/sensorium/sensorium-core/src/directive.rs`

- [ ] **Step 1: Write the module**

Create `crates/sensorium/sensorium-core/src/directive.rs`:

```rust
//! AttentionDirective — L0's control input back into the sensory substrate.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::{key_expr::KeyExpr, quality::QualityStatus, source::SourceId};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RatePolicy {
    Unlimited,
    MaxPerSecond(u32),
    MinIntervalMs(u64),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QosRequirement {
    pub min_quality: QualityStatus,
    pub max_staleness: Option<Duration>,
}

impl Default for QosRequirement {
    fn default() -> Self {
        Self {
            min_quality: QualityStatus::Uncertain,
            max_staleness: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum AttentionDirective {
    Subscribe {
        key_pattern: KeyExpr,
        qos: QosRequirement,
    },
    Unsubscribe {
        key_pattern: KeyExpr,
    },
    AdjustRate {
        key_pattern: KeyExpr,
        policy: RatePolicy,
    },
    AdjustQuality {
        key_pattern: KeyExpr,
        min_quality: QualityStatus,
    },
    EnableSource {
        source_id: SourceId,
    },
    DisableSource {
        source_id: SourceId,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directives_round_trip_json() {
        let d = AttentionDirective::Subscribe {
            key_pattern: KeyExpr::new("plant/**").unwrap(),
            qos: QosRequirement::default(),
        };
        let s = serde_json::to_string(&d).unwrap();
        let d2: AttentionDirective = serde_json::from_str(&s).unwrap();
        assert_eq!(d, d2);
    }
}
```

- [ ] **Step 2: Expose in lib.rs**

```rust
pub mod directive;

pub use directive::{AttentionDirective, QosRequirement, RatePolicy};
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p sensorium-core directive`
Expected: 1 test passes.

- [ ] **Step 4: Lint and format**

Run: `cargo clippy -p sensorium-core -- -D warnings && cargo fmt`

- [ ] **Step 5: Commit**

```bash
git add crates/sensorium/sensorium-core/src/directive.rs crates/sensorium/sensorium-core/src/lib.rs
git commit -m "feat(sensorium-core): AttentionDirective + RatePolicy + QosRequirement

L0's control inputs to the fabric — subscribe, unsubscribe, rate
adjustment, quality floor, enable/disable source. Realizes Pneuma::
Directive for the ExternalToL0 boundary.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 10: Publisher trait + BirthCertificate + QosProfile

**Files:**
- Create: `crates/sensorium/sensorium-core/src/publisher.rs`
- Create: `crates/sensorium/sensorium-core/src/lifecycle.rs`

- [ ] **Step 1: Write lifecycle.rs** (contains the types publisher references)

Create `crates/sensorium/sensorium-core/src/lifecycle.rs`:

```rust
//! Birth/death certificates and minimal QoS profiles.
//! Full QoS negotiation matrix lives in a future sensorium-qos crate.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::{key_expr::KeyExpr, signal_kind::DataEncoding, source::SourceDescriptor};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Reliability {
    BestEffort,
    Reliable,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Durability {
    Volatile,
    TransientLocal { depth: u32 },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum History {
    KeepLast(u32),
    KeepAll,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QosProfile {
    pub reliability: Reliability,
    pub durability: Durability,
    pub history: History,
    pub deadline: Option<Duration>,
}

impl QosProfile {
    pub fn sensor_stream() -> Self {
        Self {
            reliability: Reliability::BestEffort,
            durability: Durability::Volatile,
            history: History::KeepLast(5),
            deadline: None,
        }
    }

    pub fn critical_alarm() -> Self {
        Self {
            reliability: Reliability::Reliable,
            durability: Durability::TransientLocal { depth: 100 },
            history: History::KeepAll,
            deadline: Some(Duration::from_secs(1)),
        }
    }

    pub fn media_frame() -> Self {
        Self {
            reliability: Reliability::BestEffort,
            durability: Durability::Volatile,
            history: History::KeepLast(1),
            deadline: None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TopicDeclaration {
    pub key: KeyExpr,
    pub encoding: DataEncoding,
    pub description: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BirthCertificate {
    pub source: SourceDescriptor,
    pub topics: Vec<TopicDeclaration>,
    pub qos: QosProfile,
    pub capabilities: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sensor_stream_profile_is_best_effort() {
        let p = QosProfile::sensor_stream();
        assert_eq!(p.reliability, Reliability::BestEffort);
        assert_eq!(p.history, History::KeepLast(5));
    }

    #[test]
    fn critical_alarm_profile_is_reliable() {
        let p = QosProfile::critical_alarm();
        assert_eq!(p.reliability, Reliability::Reliable);
        assert_eq!(p.history, History::KeepAll);
    }
}
```

- [ ] **Step 2: Write publisher.rs**

Create `crates/sensorium/sensorium-core/src/publisher.rs`:

```rust
//! Publisher trait — the contract every concrete publisher crate implements.

use async_trait::async_trait;
use thiserror::Error;

use crate::{lifecycle::BirthCertificate, signal::SensorySignal, source::SourceDescriptor};

#[derive(Debug, Error)]
pub enum PublisherError {
    #[error("connect failed: {0}")]
    Connect(String),
    #[error("protocol error: {0}")]
    Protocol(String),
    #[error("unavailable: {0}")]
    Unavailable(String),
}

/// Sink exposed to publishers for emitting signals into the fabric.
#[async_trait]
pub trait SignalSink: Send + Sync {
    async fn emit(&self, signal: SensorySignal) -> Result<(), PublisherError>;
}

/// Cancellation signal for long-running publisher `run` loops.
#[derive(Clone)]
pub struct CancelToken {
    inner: std::sync::Arc<tokio::sync::Notify>,
}

impl CancelToken {
    pub fn new() -> Self {
        Self {
            inner: std::sync::Arc::new(tokio::sync::Notify::new()),
        }
    }

    pub fn cancel(&self) {
        self.inner.notify_waiters();
    }

    pub async fn cancelled(&self) {
        self.inner.notified().await;
    }
}

impl Default for CancelToken {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
pub trait Publisher: Send + Sync {
    fn descriptor(&self) -> &SourceDescriptor;

    async fn birth(&self) -> Result<BirthCertificate, PublisherError>;

    async fn run(
        &self,
        sink: std::sync::Arc<dyn SignalSink>,
        shutdown: CancelToken,
    ) -> Result<(), PublisherError>;

    async fn death(&self) -> Result<(), PublisherError>;
}
```

- [ ] **Step 3: Expose both modules in lib.rs**

```rust
pub mod lifecycle;
pub mod publisher;

pub use lifecycle::{
    BirthCertificate, Durability, History, QosProfile, Reliability, TopicDeclaration,
};
pub use publisher::{CancelToken, Publisher, PublisherError, SignalSink};
```

- [ ] **Step 4: Run tests and verify compile**

Run: `cargo test -p sensorium-core lifecycle`
Expected: 2 tests pass.

Run: `cargo check -p sensorium-core`
Expected: clean compile.

- [ ] **Step 5: Lint, format, commit**

```bash
cargo clippy -p sensorium-core -- -D warnings
cargo fmt
git add crates/sensorium/sensorium-core/src/publisher.rs crates/sensorium/sensorium-core/src/lifecycle.rs crates/sensorium/sensorium-core/src/lib.rs
git commit -m "feat(sensorium-core): Publisher trait, BirthCertificate, QosProfile

Publisher contract (birth → run → death) plus Sparkplug-inspired
BirthCertificate carrying topic declarations and QoS. Minimal
QosProfile with three predefined profiles (sensor_stream,
critical_alarm, media_frame); full negotiation lives in future
sensorium-qos crate.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 11: Transformer trait

**Files:**
- Create: `crates/sensorium/sensorium-core/src/transformer.rs`

- [ ] **Step 1: Write the module**

Create `crates/sensorium/sensorium-core/src/transformer.rs`:

```rust
//! Transformer trait — topic → topic functions composed as DAGs.

use async_trait::async_trait;
use thiserror::Error;

use crate::{key_expr::KeyExpr, signal::SensorySignal};

#[derive(Debug, Error)]
pub enum TransformerError {
    #[error("transformation failed: {0}")]
    Failed(String),
    #[error("resource unavailable: {0}")]
    Unavailable(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ComputeCost {
    Trivial,
    Light,
    Medium,
    Heavy,
    GpuRequired,
}

#[async_trait]
pub trait Transformer: Send + Sync {
    fn inputs(&self) -> Vec<KeyExpr>;
    fn outputs(&self) -> Vec<KeyExpr>;
    fn cost(&self) -> ComputeCost;

    async fn transform(
        &self,
        input: &SensorySignal,
    ) -> Result<Vec<SensorySignal>, TransformerError>;
}
```

- [ ] **Step 2: Expose in lib.rs**

```rust
pub mod transformer;

pub use transformer::{ComputeCost, Transformer, TransformerError};
```

- [ ] **Step 3: Verify compile**

Run: `cargo check -p sensorium-core`
Expected: clean compile.

- [ ] **Step 4: Lint and format**

```bash
cargo clippy -p sensorium-core -- -D warnings
cargo fmt
```

- [ ] **Step 5: Commit**

```bash
git add crates/sensorium/sensorium-core/src/transformer.rs crates/sensorium/sensorium-core/src/lib.rs
git commit -m "feat(sensorium-core): Transformer trait + ComputeCost

Topic→topic functions forming DAGs. ComputeCost drives Autonomic
budget-aware gating: under economy pressure, Heavy/GpuRequired
transformers can be suspended while Trivial/Light keep running.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 12: Scaffold sensorium-fabric crate

**Files:**
- Create: `crates/sensorium/sensorium-fabric/Cargo.toml`
- Create: `crates/sensorium/sensorium-fabric/src/lib.rs`

- [ ] **Step 1: Create Cargo.toml**

Create `crates/sensorium/sensorium-fabric/Cargo.toml`:

```toml
[package]
name = "sensorium-fabric"
version.workspace = true
edition.workspace = true
license.workspace = true
authors.workspace = true
repository.workspace = true
description = "Sensorium fabric — in-process Pneuma<ExternalToL0> substrate with typed pub/sub and QoS."

[dependencies]
aios-protocol.workspace = true
sensorium-core = { path = "../sensorium-core" }
async-trait.workspace = true
chrono.workspace = true
tokio = { workspace = true, features = ["rt-multi-thread", "sync", "macros"] }
parking_lot = "0.12"
thiserror.workspace = true

[dev-dependencies]
tokio = { workspace = true, features = ["rt-multi-thread", "sync", "macros", "time", "test-util"] }
serde_json.workspace = true

[lints]
workspace = true
```

Verify `parking_lot = "0.12"` is either in the workspace root's `[workspace.dependencies]` or can be inlined. If the workspace pins it, use `parking_lot.workspace = true` instead. Check with:

```bash
grep -A1 "^\[workspace.dependencies\]" Cargo.toml | head -30
grep "parking_lot" Cargo.toml
```

If present in workspace deps, replace the inline `parking_lot = "0.12"` with `parking_lot.workspace = true`.

- [ ] **Step 2: Create lib.rs skeleton**

Create `crates/sensorium/sensorium-fabric/src/lib.rs`:

```rust
//! Sensorium fabric — in-process Pneuma<ExternalToL0> substrate.
//!
//! Holds active subscriptions, routes signals via KeyExpr matching, and
//! maintains a read-side PerceptionState snapshot.
//!
//! Future: swap in Zenoh backend for cross-host transport. The Pneuma
//! surface does not change.

#![forbid(unsafe_code)]

pub mod registry;
pub mod fabric;
pub mod pneuma_impl;

pub use fabric::{SensoriumFabric, FabricConfig};
```

- [ ] **Step 3: Verify scaffolding compiles**

The module references don't exist yet. Create empty placeholder files to keep the crate compiling during the sequence:

`crates/sensorium/sensorium-fabric/src/registry.rs`:
```rust
//! Subscription registry — implemented in Task 13.
```

`crates/sensorium/sensorium-fabric/src/fabric.rs`:
```rust
//! SensoriumFabric — implemented in Task 14–15.

use sensorium_core::SensorySignal;

#[derive(Clone, Debug, Default)]
pub struct FabricConfig {
    pub default_channel_capacity: usize,
}

#[derive(Debug)]
pub struct SensoriumFabric {
    _placeholder: std::marker::PhantomData<SensorySignal>,
}
```

`crates/sensorium/sensorium-fabric/src/pneuma_impl.rs`:
```rust
//! Pneuma<ExternalToL0> impl — completed in Task 16.
```

Run: `cargo check -p sensorium-fabric`
Expected: clean compile.

- [ ] **Step 4: Lint and format**

```bash
cargo clippy -p sensorium-fabric -- -D warnings
cargo fmt
```

- [ ] **Step 5: Commit**

```bash
git add crates/sensorium/sensorium-fabric Cargo.toml
git commit -m "feat(sensorium-fabric): scaffold crate skeleton

Empty modules for registry/fabric/pneuma_impl with placeholders to keep
the workspace compiling as subsequent tasks fill them in.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 13: Subscription registry

**Files:**
- Replace: `crates/sensorium/sensorium-fabric/src/registry.rs`

- [ ] **Step 1: Write the failing tests first**

Replace `crates/sensorium/sensorium-fabric/src/registry.rs`:

```rust
//! Subscription registry — maps pattern KeyExprs to broadcast channels.

use std::sync::Arc;

use parking_lot::RwLock;
use sensorium_core::{KeyExpr, QosRequirement, SensorySignal};
use tokio::sync::broadcast;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SubscriptionId(pub Uuid);

impl SubscriptionId {
    fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

#[derive(Debug)]
struct Subscription {
    id: SubscriptionId,
    pattern: KeyExpr,
    _qos: QosRequirement,
    sender: broadcast::Sender<SensorySignal>,
}

#[derive(Debug, Default)]
pub struct Registry {
    inner: RwLock<Vec<Subscription>>,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn subscribe(
        &self,
        pattern: KeyExpr,
        qos: QosRequirement,
        capacity: usize,
    ) -> (SubscriptionId, broadcast::Receiver<SensorySignal>) {
        let (tx, rx) = broadcast::channel(capacity);
        let id = SubscriptionId::new();
        self.inner.write().push(Subscription {
            id,
            pattern,
            _qos: qos,
            sender: tx,
        });
        (id, rx)
    }

    pub fn unsubscribe(&self, id: SubscriptionId) -> bool {
        let mut subs = self.inner.write();
        if let Some(pos) = subs.iter().position(|s| s.id == id) {
            subs.remove(pos);
            true
        } else {
            false
        }
    }

    /// Fan-out a signal to all matching subscriptions.
    /// Returns the number of receivers that got the signal.
    pub fn route(&self, signal: &SensorySignal) -> usize {
        let subs = self.inner.read();
        let mut count = 0;
        for sub in subs.iter() {
            if sub.pattern.matches(&signal.key) {
                // best_effort: send may fail if receiver dropped — that's fine.
                if sub.sender.send(signal.clone()).is_ok() {
                    count += 1;
                }
            }
        }
        count
    }

    pub fn active_count(&self) -> usize {
        self.inner.read().len()
    }
}

pub type SharedRegistry = Arc<Registry>;

#[cfg(test)]
mod tests {
    use super::*;
    use sensorium_core::{Payload, SignalKind};

    fn signal(key: &str) -> SensorySignal {
        SensorySignal::good(
            KeyExpr::new(key).unwrap(),
            SignalKind::AnalogMeasurement,
            Payload::F64(1.0),
        )
    }

    #[test]
    fn subscribe_then_unsubscribe() {
        let r = Registry::new();
        let (id, _rx) = r.subscribe(
            KeyExpr::new("plant/**").unwrap(),
            QosRequirement::default(),
            16,
        );
        assert_eq!(r.active_count(), 1);
        assert!(r.unsubscribe(id));
        assert_eq!(r.active_count(), 0);
    }

    #[tokio::test]
    async fn route_delivers_to_matching_subscribers_only() {
        let r = Registry::new();
        let (_id_a, mut rx_a) = r.subscribe(
            KeyExpr::new("plant/**").unwrap(),
            QosRequirement::default(),
            16,
        );
        let (_id_b, mut rx_b) = r.subscribe(
            KeyExpr::new("desktop/**").unwrap(),
            QosRequirement::default(),
            16,
        );

        let delivered = r.route(&signal("plant/t1/rpm"));
        assert_eq!(delivered, 1);

        let got = rx_a.recv().await.unwrap();
        assert_eq!(got.key.as_str(), "plant/t1/rpm");

        // desktop subscriber should not receive the plant signal.
        let timeout = tokio::time::timeout(std::time::Duration::from_millis(50), rx_b.recv()).await;
        assert!(timeout.is_err(), "desktop subscriber should time out");
    }

    #[tokio::test]
    async fn multiple_matching_subscribers_all_receive() {
        let r = Registry::new();
        let (_, mut rx1) = r.subscribe(
            KeyExpr::new("plant/*/rpm").unwrap(),
            QosRequirement::default(),
            16,
        );
        let (_, mut rx2) = r.subscribe(
            KeyExpr::new("plant/t1/**").unwrap(),
            QosRequirement::default(),
            16,
        );

        let delivered = r.route(&signal("plant/t1/rpm"));
        assert_eq!(delivered, 2);

        assert!(rx1.recv().await.is_ok());
        assert!(rx2.recv().await.is_ok());
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p sensorium-fabric registry`
Expected: 3 tests pass.

- [ ] **Step 3: Lint and format**

```bash
cargo clippy -p sensorium-fabric -- -D warnings
cargo fmt
```

- [ ] **Step 4: Commit**

```bash
git add crates/sensorium/sensorium-fabric/src/registry.rs
git commit -m "feat(sensorium-fabric): subscription registry with KeyExpr routing

Active subscriptions held under RwLock; fan-out via route() uses KeyExpr
pattern matching. Tokio broadcast channels provide multi-subscriber
delivery with bounded capacity for backpressure. Three tests cover
subscribe/unsubscribe, selective delivery, and multi-matcher fan-out.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 14: SensoriumFabric core (emit, aggregate, receive)

**Files:**
- Replace: `crates/sensorium/sensorium-fabric/src/fabric.rs`

- [ ] **Step 1: Write fabric.rs**

Replace `crates/sensorium/sensorium-fabric/src/fabric.rs`:

```rust
//! SensoriumFabric — in-process perception substrate.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use parking_lot::{Mutex, RwLock};
use sensorium_core::{
    AttentionDirective, KeyExpr, PerceptionState, PublisherError, QualitySummary, QosRequirement,
    SensorySignal, SignalSink, SourceId, SourceStatus,
};
use tokio::sync::broadcast;

use crate::registry::{Registry, SubscriptionId};

#[derive(Clone, Debug)]
pub struct FabricConfig {
    pub default_channel_capacity: usize,
}

impl Default for FabricConfig {
    fn default() -> Self {
        Self { default_channel_capacity: 1024 }
    }
}

#[derive(Debug)]
pub struct SensoriumFabric {
    cfg: FabricConfig,
    registry: Arc<Registry>,
    state: RwLock<PerceptionStateInner>,
    pending_directives: Mutex<Vec<AttentionDirective>>,
}

#[derive(Debug, Default)]
struct PerceptionStateInner {
    sources: std::collections::HashMap<SourceId, SourceStatus>,
    latest: std::collections::HashMap<String, SensorySignal>,
    quality: QualitySummary,
    sparkplug: std::collections::HashMap<SourceId, sensorium_core::LifecycleState>,
}

impl SensoriumFabric {
    pub fn new(cfg: FabricConfig) -> Self {
        Self {
            cfg,
            registry: Arc::new(Registry::new()),
            state: RwLock::new(PerceptionStateInner::default()),
            pending_directives: Mutex::new(Vec::new()),
        }
    }

    pub fn registry(&self) -> Arc<Registry> {
        self.registry.clone()
    }

    pub fn subscribe(
        &self,
        pattern: KeyExpr,
        qos: QosRequirement,
    ) -> (SubscriptionId, broadcast::Receiver<SensorySignal>) {
        self.registry
            .subscribe(pattern, qos, self.cfg.default_channel_capacity)
    }

    pub fn emit_signal(&self, signal: SensorySignal) -> usize {
        // Update aggregate state first, then route.
        {
            let mut state = self.state.write();
            state.latest.insert(signal.key.as_str().to_string(), signal.clone());
            if signal.quality.status.is_good() {
                state.quality.good += 1;
            } else if signal.quality.status.is_bad() {
                state.quality.bad += 1;
            } else {
                state.quality.uncertain += 1;
            }
        }
        self.registry.route(&signal)
    }

    pub fn snapshot(&self) -> PerceptionState {
        let state = self.state.read();
        PerceptionState {
            active_sources: state.sources.clone(),
            latest_by_key: state.latest.clone(),
            quality_summary: state.quality.clone(),
            sparkplug_state: state.sparkplug.clone(),
            captured_at: Utc::now(),
        }
    }

    pub fn push_directive(&self, d: AttentionDirective) {
        self.pending_directives.lock().push(d);
    }

    pub fn pop_directive(&self) -> Option<AttentionDirective> {
        let mut q = self.pending_directives.lock();
        if q.is_empty() {
            None
        } else {
            Some(q.remove(0))
        }
    }
}

pub struct FabricSink {
    fabric: Arc<SensoriumFabric>,
}

impl FabricSink {
    pub fn new(fabric: Arc<SensoriumFabric>) -> Self {
        Self { fabric }
    }
}

#[async_trait]
impl SignalSink for FabricSink {
    async fn emit(&self, signal: SensorySignal) -> Result<(), PublisherError> {
        self.fabric.emit_signal(signal);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sensorium_core::{Payload, SignalKind};

    fn signal(key: &str, val: f64) -> SensorySignal {
        SensorySignal::good(
            KeyExpr::new(key).unwrap(),
            SignalKind::AnalogMeasurement,
            Payload::F64(val),
        )
    }

    #[tokio::test]
    async fn emit_updates_snapshot() {
        let f = SensoriumFabric::new(FabricConfig::default());
        f.emit_signal(signal("plant/t1/rpm", 1850.0));
        f.emit_signal(signal("plant/t2/rpm", 1700.0));

        let snap = f.snapshot();
        assert_eq!(snap.latest_by_key.len(), 2);
        assert_eq!(snap.quality_summary.good, 2);
    }

    #[tokio::test]
    async fn directive_queue_is_fifo() {
        let f = SensoriumFabric::new(FabricConfig::default());
        let d1 = AttentionDirective::Unsubscribe {
            key_pattern: KeyExpr::new("a/b").unwrap(),
        };
        let d2 = AttentionDirective::Unsubscribe {
            key_pattern: KeyExpr::new("c/d").unwrap(),
        };
        f.push_directive(d1.clone());
        f.push_directive(d2.clone());

        assert_eq!(f.pop_directive(), Some(d1));
        assert_eq!(f.pop_directive(), Some(d2));
        assert_eq!(f.pop_directive(), None);
    }

    #[tokio::test]
    async fn emit_and_subscribe_deliver_signal() {
        let f = Arc::new(SensoriumFabric::new(FabricConfig::default()));
        let (_id, mut rx) = f.subscribe(
            KeyExpr::new("plant/**").unwrap(),
            QosRequirement::default(),
        );

        let sent = tokio::spawn({
            let f = f.clone();
            async move {
                f.emit_signal(signal("plant/t1/rpm", 1850.0));
            }
        });
        sent.await.unwrap();

        let got = rx.recv().await.unwrap();
        assert_eq!(got.key.as_str(), "plant/t1/rpm");
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p sensorium-fabric fabric`
Expected: 3 tests pass.

- [ ] **Step 3: Lint and format**

```bash
cargo clippy -p sensorium-fabric -- -D warnings
cargo fmt
```

- [ ] **Step 4: Commit**

```bash
git add crates/sensorium/sensorium-fabric/src/fabric.rs
git commit -m "feat(sensorium-fabric): SensoriumFabric core — emit, snapshot, directives

Fabric holds subscription registry + PerceptionState inner + pending
directive queue. emit_signal() updates aggregate and routes in one call.
snapshot() returns a consistent clone of the state. Directives are
FIFO-queued for L0 cognition to drain via pop_directive().

Also adds FabricSink — the Arc<dyn SignalSink> publishers use to emit.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 15: Pneuma<ExternalToL0> impl for SensoriumFabric

**Files:**
- Replace: `crates/sensorium/sensorium-fabric/src/pneuma_impl.rs`

- [ ] **Step 1: Write the Pneuma impl**

Replace `crates/sensorium/sensorium-fabric/src/pneuma_impl.rs`:

```rust
//! Pneuma<ExternalToL0> impl — the canonical integration point.
//!
//! Wraps SensoriumFabric's inherent methods in the Pneuma trait surface
//! so callers depending only on `aios-protocol` can use Sensorium without
//! importing sensorium-fabric types directly.

use aios_protocol::pneuma::{
    ExternalToL0, Pneuma, PneumaError, ResourceCeiling, SubstrateKind, SubstrateProfile,
    WarpFactors,
};
use sensorium_core::{AttentionDirective, PerceptionState, SensorySignal};

use crate::fabric::SensoriumFabric;

impl Pneuma for SensoriumFabric {
    type B = ExternalToL0;
    type Signal = SensorySignal;
    type Aggregate = PerceptionState;
    type Directive = AttentionDirective;

    fn emit(&self, signal: SensorySignal) -> Result<(), PneumaError> {
        let delivered = self.emit_signal(signal);
        // For in-process fabric, emit_signal is infallible unless a
        // downstream queue is saturated and we want to surface that
        // (deferred — current impl drops on saturation per tokio::broadcast
        // semantics).
        let _ = delivered;
        Ok(())
    }

    fn aggregate(&self) -> PerceptionState {
        self.snapshot()
    }

    fn receive(&self) -> Option<AttentionDirective> {
        self.pop_directive()
    }

    fn substrate(&self) -> SubstrateProfile {
        // In-process fabric on classical silicon. When publishers register,
        // this aggregates into Hybrid; for now (no publishers yet) return
        // a baseline profile.
        SubstrateProfile {
            kind: SubstrateKind::ClassicalSilicon,
            warp_factors: WarpFactors::classical_baseline(),
            ceiling: ResourceCeiling::Thermodynamic { max_watts: 100.0 },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fabric::FabricConfig;
    use sensorium_core::{KeyExpr, Payload, SignalKind};

    #[test]
    fn pneuma_emit_and_aggregate_round_trip() {
        let f = SensoriumFabric::new(FabricConfig::default());
        let sig = SensorySignal::good(
            KeyExpr::new("plant/t1/rpm").unwrap(),
            SignalKind::AnalogMeasurement,
            Payload::F64(1850.0),
        );
        <SensoriumFabric as Pneuma>::emit(&f, sig).unwrap();

        let agg = <SensoriumFabric as Pneuma>::aggregate(&f);
        assert_eq!(agg.latest_by_key.len(), 1);
    }

    #[test]
    fn pneuma_receive_drains_directive_queue() {
        let f = SensoriumFabric::new(FabricConfig::default());
        assert!(<SensoriumFabric as Pneuma>::receive(&f).is_none());
        f.push_directive(AttentionDirective::Unsubscribe {
            key_pattern: KeyExpr::new("a/b").unwrap(),
        });
        assert!(<SensoriumFabric as Pneuma>::receive(&f).is_some());
        assert!(<SensoriumFabric as Pneuma>::receive(&f).is_none());
    }

    #[test]
    fn substrate_profile_is_classical() {
        let f = SensoriumFabric::new(FabricConfig::default());
        let profile = <SensoriumFabric as Pneuma>::substrate(&f);
        assert!(matches!(profile.kind, SubstrateKind::ClassicalSilicon));
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p sensorium-fabric pneuma_impl`
Expected: 3 tests pass.

- [ ] **Step 3: Lint and format**

```bash
cargo clippy -p sensorium-fabric -- -D warnings
cargo fmt
```

- [ ] **Step 4: Commit**

```bash
git add crates/sensorium/sensorium-fabric/src/pneuma_impl.rs
git commit -m "feat(sensorium-fabric): Pneuma<ExternalToL0> impl

SensoriumFabric implements the Pneuma trait for the ExternalToL0
boundary. Associated types: Signal=SensorySignal, Aggregate=
PerceptionState, Directive=AttentionDirective. emit()/aggregate()/
receive() wrap the fabric's inherent methods; substrate() returns
a classical-silicon profile for the in-process fabric.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 16: End-to-end integration test with MockPublisher

**Files:**
- Create: `crates/sensorium/sensorium-fabric/tests/integration.rs`

- [ ] **Step 1: Write the test module**

Create `crates/sensorium/sensorium-fabric/tests/integration.rs`:

```rust
//! End-to-end: a mock publisher running against the Pneuma surface.

use std::sync::Arc;

use aios_protocol::pneuma::Pneuma;
use async_trait::async_trait;
use sensorium_core::{
    BirthCertificate, CancelToken, KeyExpr, Payload, Protocol, Publisher, PublisherError,
    QosProfile, SensorySignal, SignalKind, SignalSink, SourceDescriptor, SourceId,
    TopicDeclaration,
};
use sensorium_fabric::{FabricConfig, SensoriumFabric};

/// Trivial publisher that emits `count` signals then exits.
struct MockPublisher {
    descriptor: SourceDescriptor,
    count: usize,
}

impl MockPublisher {
    fn new(count: usize) -> Self {
        Self {
            descriptor: SourceDescriptor {
                source_id: SourceId::new(),
                protocol: Protocol::Custom("mock".into()),
                address: "mock://test".into(),
                capabilities: vec![],
            },
            count,
        }
    }
}

#[async_trait]
impl Publisher for MockPublisher {
    fn descriptor(&self) -> &SourceDescriptor {
        &self.descriptor
    }

    async fn birth(&self) -> Result<BirthCertificate, PublisherError> {
        Ok(BirthCertificate {
            source: self.descriptor.clone(),
            topics: vec![TopicDeclaration {
                key: KeyExpr::new("mock/heartbeat").unwrap(),
                encoding: sensorium_core::DataEncoding::Raw,
                description: Some("test signal".into()),
            }],
            qos: QosProfile::sensor_stream(),
            capabilities: vec![],
        })
    }

    async fn run(
        &self,
        sink: Arc<dyn SignalSink>,
        _shutdown: CancelToken,
    ) -> Result<(), PublisherError> {
        for i in 0..self.count {
            let signal = SensorySignal::good(
                KeyExpr::new("mock/heartbeat").unwrap(),
                SignalKind::Custom("mock.heartbeat".into()),
                Payload::I64(i as i64),
            );
            sink.emit(signal).await?;
        }
        Ok(())
    }

    async fn death(&self) -> Result<(), PublisherError> {
        Ok(())
    }
}

#[tokio::test]
async fn publisher_emits_through_fabric_pneuma() {
    use sensorium_fabric::fabric::FabricSink;

    let fabric = Arc::new(SensoriumFabric::new(FabricConfig::default()));
    let pub_ = MockPublisher::new(5);

    let cert = pub_.birth().await.unwrap();
    assert_eq!(cert.topics.len(), 1);

    let sink: Arc<dyn SignalSink> = Arc::new(FabricSink::new(fabric.clone()));
    pub_.run(sink, CancelToken::new()).await.unwrap();

    // Read Pneuma aggregate — the L0 view of perception.
    let agg = <SensoriumFabric as Pneuma>::aggregate(&fabric);
    assert_eq!(agg.latest_by_key.len(), 1, "one key, latest wins");
    let latest = agg.latest_by_key.get("mock/heartbeat").unwrap();
    assert!(matches!(latest.payload, Payload::I64(4)), "last value");
    assert_eq!(agg.quality_summary.good, 5);
}

#[tokio::test]
async fn subscriber_receives_each_emitted_signal() {
    use sensorium_fabric::fabric::FabricSink;

    let fabric = Arc::new(SensoriumFabric::new(FabricConfig::default()));
    let (_id, mut rx) = fabric.subscribe(
        KeyExpr::new("mock/**").unwrap(),
        sensorium_core::QosRequirement::default(),
    );

    let pub_ = MockPublisher::new(3);
    let sink: Arc<dyn SignalSink> = Arc::new(FabricSink::new(fabric.clone()));

    // Spawn so subscription is established first.
    let handle = tokio::spawn(async move {
        pub_.run(sink, CancelToken::new()).await.unwrap();
    });
    handle.await.unwrap();

    let mut received = vec![];
    while let Ok(s) = tokio::time::timeout(std::time::Duration::from_millis(50), rx.recv()).await
    {
        received.push(s.unwrap());
    }
    assert_eq!(received.len(), 3);
}
```

- [ ] **Step 2: Make the `fabric` module public for tests**

The test references `sensorium_fabric::fabric::FabricSink`. Verify `src/lib.rs` exports the fabric module:

```rust
pub mod fabric;   // already public from Task 12
```

If `FabricSink` isn't re-exported at the crate root, add to lib.rs:

```rust
pub use fabric::FabricSink;
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p sensorium-fabric --test integration`
Expected: 2 tests pass.

- [ ] **Step 4: Run full workspace validation**

```bash
cargo fmt
cargo clippy --workspace -- -D warnings
cargo test --workspace
```

Expected: all pre-existing + 3 aios-protocol + 6 key_expr + 3 source + 2 quality + 2 signal_kind + 2 signal + 2 perception + 1 directive + 2 lifecycle + 3 registry + 3 fabric + 3 pneuma_impl + 2 integration = **1077 existing + 34 new tests passing**.

If pre-existing tests fail, something outside Sensorium regressed — investigate before proceeding.

- [ ] **Step 5: Commit**

```bash
git add crates/sensorium/sensorium-fabric/tests/integration.rs crates/sensorium/sensorium-fabric/src/lib.rs
git commit -m "feat(sensorium-fabric): end-to-end integration test with MockPublisher

Test publisher emits 5 signals through FabricSink. Verifies both the
Pneuma surface (aggregate returns latest-wins perception state) and the
subscriber channel (each emitted signal delivered to matching subscribers).

This closes the Sensorium foundation — a working Pneuma<ExternalToL0>
substrate verified end-to-end with a mock source.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 17: Document what landed in life-monorepo

**Files:**
- Modify: `core/life/docs/STATUS.md` (or equivalent status doc)
- Modify: `core/life/docs/ARCHITECTURE.md`

- [ ] **Step 1: Update ARCHITECTURE.md**

Locate the `crates/life/docs/ARCHITECTURE.md` file and find the "Planned" table (Chronos / Aegis / Mnemo etc.). Add Sensorium as a new row with status "FOUNDATION LANDED" — or equivalent active indicator your project uses. Cross-reference `docs/specs/sensorium-architecture.md` and this plan.

Replace the relevant section. Exact text to add (adjust table formatting to match surrounding style):

```markdown
| Perception | Sensory substrate | Sensorium | FOUNDATION LANDED (2026-04-19) |
```

And in any pillar-diagram section, add Sensorium as an active pillar below Vigil.

- [ ] **Step 2: Update STATUS.md**

Add under current phase status:

```markdown
### Sensorium foundation (2026-04-19)

Pneuma<ExternalToL0> substrate landed. Two new crates:

- `sensorium-core` — SensorySignal, PerceptionState, AttentionDirective,
  Publisher and Transformer traits, QosProfile, BirthCertificate.
- `sensorium-fabric` — in-process pub/sub with KeyExpr matching,
  SensoriumFabric implementing Pneuma<ExternalToL0>.

End-to-end validated with MockPublisher. Arcan bridge, concrete
publishers (opsis, screen, modbus, ble), and daemon tracked in
follow-up plans. See `docs/specs/sensorium-architecture.md`.
```

- [ ] **Step 3: Verify and commit**

```bash
git add core/life/docs/STATUS.md core/life/docs/ARCHITECTURE.md
git commit -m "docs(sensorium): mark foundation landed in ARCHITECTURE + STATUS

Sensorium is now an active Life pillar. ARCHITECTURE.md adds the row;
STATUS.md records the two-crate landing with a pointer to the
architecture spec.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Self-Review Summary

**Spec coverage check:**

| Spec section | Covered by |
|---|---|
| ExternalToL0 boundary marker | Task 1 |
| Reserved markers (L1/L2/L3/field) | Task 1 |
| KeyExpr | Task 3 |
| Source identity + Sparkplug state | Task 4 |
| SignalQuality (OPC-UA) | Task 5 |
| SignalKind taxonomy | Task 6 |
| SensorySignal | Task 7 |
| PerceptionState | Task 8 |
| AttentionDirective | Task 9 |
| Publisher trait | Task 10 |
| Transformer trait | Task 11 |
| QosProfile (minimal) | Task 10 |
| BirthCertificate / TopicDeclaration | Task 10 |
| Typed pub/sub fabric | Tasks 13–14 |
| Pneuma<ExternalToL0> impl | Task 15 |
| End-to-end verification | Task 16 |

**Deferred to follow-up plans** (per spec Phase 2+):
- Arcan bridge (`arcan-sensorium`) — follow-up plan
- Tiered persistence (`sensorium-lago`) — follow-up plan
- Full QoS negotiation matrix (`sensorium-qos`) — companion spec + plan
- Tag catalog / ISA-18.2 (`sensorium-schema`) — companion spec + plan
- Concrete publishers (opsis, screen, modbus, ble, ...) — one plan per V1–V4 validation target
- `sensoriumd` daemon — follow-up plan

**Type consistency verified:** `SensorySignal`, `PerceptionState`, `AttentionDirective` names match across all tasks and the spec. `KeyExpr::matches(&concrete)` direction (self=pattern, arg=concrete) is consistent across registry and tests.

**No placeholders remain:** every code step shows actual code; every test step shows actual test; every commit message is concrete.