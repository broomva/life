---
title: "Sensorium Architecture Specification"
tags:
  - spec
  - architecture
  - sensorium
  - pneuma
  - life-os
  - perception
created: "2026-04-19"
updated: "2026-04-19"
status: draft
related:
  - "[[pneuma-plexus-architecture]]"
  - "[[pneuma-trait-surface]]"
  - "[[pneuma-vertical-retrofits]]"
  - "[[life-plexus-implementation]]"
  - "[[ARCHITECTURE]]"
  - "[[LAGO_ARCHITECTURE]]"
  - "[[METALAYER]]"
---

# Sensorium Architecture Specification

## Overview

This specification defines **Sensorium** — the perception substrate of the Life Agent OS. Sensorium implements `Pneuma<B = ExternalToL0>`: the boundary between the external world (arbitrary data sources — consumer, industrial, scientific, digital) and the agent's L0 plant. It is how Life agents perceive.

Sensorium is a family of publisher crates (screen, audio, Modbus, OPC-UA, MQTT, BLE, CAN bus, serial, Opsis, network) feeding a typed pub/sub fabric with QoS negotiation, schema registries, and state lifecycle. Above the fabric: a single Pneuma trait surface so Arcan treats "perceive a PLC register" and "perceive a screen frame" with the same API. Below the fabric: protocol-specific I/O isolated behind a stable internal contract.

**Design principle:** Sensorium is *not* a new top-level pillar with bespoke abstractions. It is the concrete implementation of an existing trait family (Pneuma) at a boundary not previously filled. This follows the "trait, not rename" discipline from the Pneuma/Plexus architecture.

## Motivation

Life's existing perception paths are narrow and ad-hoc:

- Arcan receives user messages (HTTP) and tool results (in-process) — no continuous external perception.
- `arcan-opsis::WorldStateInjector` subscribes to Opsis SSE with severity thresholds — one hand-rolled implementation for one data source.
- Voice event types exist in `EventKind` but have no implementation.
- Industrial protocols (Modbus, OPC-UA), wearable data (BLE), screen context, ambient audio — all unsupported.

This blocks the following use cases:

- A Life agent operating a wind farm must observe turbine SCADA data with ISA-18.2 alarm semantics.
- A Life agent assisting a desktop user must perceive screen context and audio with the same coherence as Arcan perceives user messages.
- A Life agent at an edge gateway must multiplex 1000+ sensor topics with deadband filtering and quality propagation.
- A Life agent collaborating with an Omi wearable must ingest BLE audio with VAD, diarization, and state-aware source lifecycle.

All of these are instances of one pattern: arbitrary external data flowing across a boundary into L0, with filtering, schema, and quality semantics. Pneuma already names this pattern. Sensorium realizes it for the `ExternalToL0` boundary.

## Terminology

- **Sensorium** (Latin, "seat of sensation"): the perception substrate. A family of crates implementing `Pneuma<B = ExternalToL0>`.
- **SensorySignal**: typed payload crossing the external→L0 boundary. Produced by publishers, delivered to L0 via the fabric.
- **PerceptionState**: the aggregate observation — what L0 "sees" at a point in time across all active sources.
- **AttentionDirective**: control input from L0 into the sensory substrate — subscription changes, QoS adjustments, enable/disable sources.
- **SensoriumFabric**: the typed pub/sub substrate that multiplexes publishers. The concrete type implementing `Pneuma<ExternalToL0>`.
- **Publisher**: a crate exposing one external source family (screen, audio, Modbus, etc.) through a uniform internal contract.
- **Transformer**: a topic→topic function that adds structure (OCR, transcription, unit scaling, alarm evaluation).
- **TagCatalog**: the schema registry — maps topic keys to data types, engineering units, scaling, alarm limits, deadband.
- **BirthCertificate** (Sparkplug-inspired): publisher metadata announced on connect — declared topics, schema, QoS profile, capabilities.

## Architecture

### 1. Core trait extension (in `aios-protocol`)

Add one new boundary marker to the existing Pneuma trait family. This is a minimal, additive change; no existing types are modified.

```rust
// aios-protocol/src/pneuma.rs — additive

/// The boundary between the external world and the agent's L0 plant.
///
/// Vertical axis, below L0. Crossed by all sensory input — screen frames,
/// audio, industrial protocols, network feeds, wearable telemetry.
pub struct ExternalToL0;

impl Boundary for ExternalToL0 {
    fn axis_name() -> &'static str { "vertical" }
    fn boundary_name() -> &'static str { "external → L0" }
}
```

**Reserved boundary names** (documented, not implemented, to claim the namespace and communicate architectural intent):

```rust
/// Reserved: direct external feeds into L1 homeostatic state (battery level,
/// CPU temperature, exchange rate, token cost feeds — things that update
/// regulation without passing through the plant).
pub struct ExternalToL1;

/// Reserved: external feeds into L2 meta-control (eval/benchmark scores,
/// A/B test results, performance feeds — things that inform which
/// controller to deploy).
pub struct ExternalToL2;

/// Reserved: external feeds into L3 governance (regulatory updates,
/// compliance feeds, policy mandates — things that update governance
/// without passing through plant, controller, or meta-control).
pub struct ExternalToL3;

/// Reserved: shared perception within a formation (multi-agent sensor fusion).
/// Implementation deferred until multi-agent perception needs emerge.
pub struct PerceptToField;

/// Reserved: depth-1 Sensorium — hive-level perception. Follows life-plexus
/// maturity (requires P6 horizontal stability proof).
pub struct ExternalToD1;

/// Reserved: environmental field effects feeding back into individual perception.
/// Counterpart to PerceptToField.
pub struct FieldToPercept;
```

No `impl Boundary` for reserved names until their owning crate exists.

### 1a. Why pinned to `ExternalToL0`, not parametric over level

Sensorium is deliberately specific to `ExternalToL0` rather than a family generic over RCS level. The rationale:

- Semantic divergence. A screen frame, a battery percentage, an eval score, and a compliance policy update are all "external inputs," but their `Signal`/`Aggregate`/`Directive` types share almost no structure. Collapsing them under one generic trait would either force a lowest-common-denominator payload or push discrimination into runtime tags — both regressions on the type-safe design.
- Sensor data specifically is *plant-level*. It describes the world the agent operates in. Regulatory state (L1), meta-control state (L2), and governance state (L3) are not plant observations — they are properties of the agent's regulation stack, which happens to receive external updates.
- Four distinct Pneuma impls (one per external level) stay simpler to test, document, and reason about than one parametric impl with four associated-type bundles.

**Reusability at the substrate layer, not the trait layer.** The machinery inside `sensorium-fabric` — typed pub/sub, QoS negotiation, Sparkplug-style lifecycle, tag catalogs — is generic in the Signal type. Future crates implementing `Pneuma<B = ExternalToL1>` (homeostatic feeds), `ExternalToL2` (eval feeds), or `ExternalToL3` (governance feeds) should depend on a shared `typed-fabric` substrate crate and layer their own Signal/Aggregate/Directive on top. The reuse lives below Pneuma, not inside it. A follow-up refactor can extract `typed-fabric` once a second external-boundary crate exists (YAGNI until then).

### 2. Sensorium associated types

```rust
// crates/sensorium/sensorium-core/src/lib.rs

/// The payload crossing the external → L0 boundary.
///
/// Universal across all publisher types. Protocol-specific structure lives
/// in the `payload` and `metadata` fields; quality and state are first-class.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SensorySignal {
    pub key: KeyExpr,
    pub timestamp: Timestamp,
    pub kind: SignalKind,
    pub payload: Payload,
    pub quality: SignalQuality,
    pub source_state: SourceState,
    pub metadata: serde_json::Value,
}

/// What L0 observes right now — a snapshot across all active topics.
#[derive(Clone, Debug)]
pub struct PerceptionState {
    pub active_sources: HashMap<SourceId, SourceStatus>,
    pub latest_by_key: HashMap<KeyExpr, SensorySignal>,
    pub quality_summary: QualitySummary,
    pub sparkplug_state: HashMap<SourceId, LifecycleState>,
    pub captured_at: Timestamp,
}

/// L0's inputs back into the sensory substrate.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum AttentionDirective {
    Subscribe { key_pattern: KeyExpr, qos: QosRequirement },
    Unsubscribe { key_pattern: KeyExpr },
    AdjustRate { key_pattern: KeyExpr, policy: RatePolicy },
    AdjustQuality { key_pattern: KeyExpr, min_quality: QualityStatus },
    SetDeadband { key: KeyExpr, deadband: f32 },
    EnableSource { source_id: SourceId },
    DisableSource { source_id: SourceId },
    SnapshotRequest { reply: SnapshotChannel },
}
```

### 3. The Pneuma implementation

```rust
// crates/sensorium/sensorium-fabric/src/lib.rs

pub struct SensoriumFabric { /* internals below */ }

impl Pneuma for SensoriumFabric {
    type B = ExternalToL0;
    type Signal = SensorySignal;
    type Aggregate = PerceptionState;
    type Directive = AttentionDirective;

    fn emit(&self, signal: SensorySignal) -> Result<(), PneumaError> {
        // 1. Match signal.key against active subscriptions
        // 2. Apply QoS policy (drop, buffer, or forward)
        // 3. Route to matching subscribers (in-process channels or Zenoh)
        // 4. Update perception snapshot atomically
        // 5. Emit vigil trace span
    }

    fn aggregate(&self) -> PerceptionState {
        // Consistent snapshot of the perception state.
        // Read-side optimized; lock-free under steady-state.
    }

    fn receive(&self) -> Option<AttentionDirective> {
        // Pending directives from L0 — reconfigurations, enable/disable.
    }

    fn substrate(&self) -> SubstrateProfile {
        // Composite profile — aggregates WarpFactors across active publishers.
        // Sensorium is always a Hybrid substrate by construction.
    }
}
```

The fabric's internals (QoS matching, topic routing, Zenoh integration) are implementation detail, invisible above the Pneuma trait.

### 4. Publisher contract

Publishers do not implement Pneuma directly — the fabric does. Publishers implement a thinner internal contract:

```rust
// crates/sensorium/sensorium-core/src/publisher.rs

#[async_trait]
pub trait Publisher: Send + Sync {
    fn descriptor(&self) -> &SourceDescriptor;

    /// Announce metadata on connect — topics, schema, QoS, capabilities.
    async fn birth(&self) -> Result<BirthCertificate, PublisherError>;

    /// Run until cancelled. Publishes signals via `fabric` handle.
    async fn run(&self, fabric: FabricHandle, shutdown: CancelToken)
        -> Result<(), PublisherError>;

    /// Gracefully disconnect. Emits death certificate.
    async fn death(&self) -> Result<(), PublisherError>;
}

pub struct BirthCertificate {
    pub source_id: SourceId,
    pub topics: Vec<TopicDeclaration>,
    pub tag_catalog: TagCatalog,
    pub qos_profile: QosProfile,
    pub capabilities: Vec<Capability>,
    pub substrate: PublisherSubstrate,
}
```

Sparkplug-inspired lifecycle: `birth` → `run` (emits data) → `death`. The fabric propagates lifecycle state to subscribers so L0 can distinguish "sensor silent" from "sensor dead."

### 5. Transformer contract

Transformers subscribe to input topics and publish to output topics — they form a DAG, not a pipeline.

```rust
// crates/sensorium/sensorium-core/src/transformer.rs

#[async_trait]
pub trait Transformer: Send + Sync {
    fn inputs(&self) -> Vec<KeyExpr>;
    fn outputs(&self) -> Vec<KeyExpr>;
    fn cost(&self) -> ComputeCost;

    async fn transform(&self, input: &SensorySignal)
        -> Result<Vec<SensorySignal>, TransformerError>;
}

pub enum ComputeCost {
    Trivial,    // Modbus decode, bit unpacking, unit scaling
    Light,      // JSON parse, threshold check, deadband
    Medium,     // On-device OCR, VAD, text embedding
    Heavy,      // Whisper transcription, image embedding
    GpuRequired,
}
```

Autonomic uses `ComputeCost` for budget-aware gating — under economy pressure, heavy transformers can be suspended while trivial ones keep running.

### 6. QoS model

DDS-inspired QoS with publisher-offer / subscriber-require negotiation:

```rust
pub struct QosProfile {
    pub reliability: Reliability,     // BestEffort | Reliable
    pub durability: Durability,       // Volatile | TransientLocal { depth }
    pub history: History,             // KeepLast(u32) | KeepAll
    pub deadline: Option<Duration>,
    pub liveliness: Liveliness,       // Automatic | ManualByTopic { lease }
    pub lifespan: Option<Duration>,
}

impl QosProfile {
    pub fn sensor_stream() -> Self { /* best-effort, volatile, keep-last-5 */ }
    pub fn critical_alarm() -> Self { /* reliable, transient-local, keep-all */ }
    pub fn configuration() -> Self { /* reliable, transient-local-infinite */ }
    pub fn media_frame() -> Self { /* best-effort, volatile, keep-last-1 */ }
}
```

Incompatible QoS between publisher and subscriber fails fast at subscription time.

### 7. Schema & tag catalog

Every publisher exposes a `TagCatalog` in its birth certificate. The catalog maps topic keys to structured metadata:

```rust
pub struct TagCatalog {
    pub entries: HashMap<KeyExpr, TagDefinition>,
}

pub struct TagDefinition {
    pub data_type: DataType,
    pub engineering_unit: Option<String>,
    pub scale: Option<Scale>,             // Linear { slope, offset }
    pub range: Option<Range>,
    pub alarm_limits: Option<AlarmLimits>, // ISA-18.2 config
    pub deadband: Option<f32>,
    pub description: Option<String>,
}
```

The catalog is required for industrial protocols (Modbus register numbers are meaningless without it) and optional for media signals (screen frames, audio chunks). Fabric validates that emitted signals match declared schema.

### 8. Quality semantics

`SignalQuality` is first-class and universally applied:

```rust
pub struct SignalQuality {
    pub status: QualityStatus,
    pub timestamp_quality: TimestampQuality,
    pub confidence: f32,
}

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
```

Quality semantics are borrowed from OPC-UA and applied universally — a dropped video frame is `BadCommunicationFailure` in exactly the same way a timed-out Modbus read is. L0 consumers can filter on quality without branching by source type.

### 9. Tiered persistence

Not every signal flows through `lago-journal`. A naive path would flood the event store with video frames. Sensorium adopts a tiered policy:

| Signal class | Storage | Cadence |
|---|---|---|
| Raw media (video chunks, audio buffers, register logs) | `lago-fs` (content-addressed blobs, zstd-compressed) | High-rate, append-only |
| Perceptual aggregates (OCR text, transcription segments, alarm events) | `lago-journal` as `EventKind::Observation` | Event-level |
| Perception state snapshots | `lago-journal` as `EventKind::Custom { event_type: "sensorium.snapshot" }` | Periodic (seconds) or on significant change |
| Birth/death certificates | `lago-journal` as `EventKind::Custom { event_type: "sensorium.lifecycle" }` | On state transition |

The fabric emits events to Lago via the existing L0→L1 Pneuma (lago-journal's `impl Pneuma<L0ToL1>`). Sensorium never bypasses the boundary chain.

### 10. Crate layout

```
aios-protocol/src/pneuma.rs
    ⤷ ADD: pub struct ExternalToL0 (with Boundary impl)
    ⤷ ADD: reserved markers (documented, no Boundary impl yet)

crates/sensorium/
├── sensorium-core/              # Types, traits, KeyExpr, QoS, schema
├── sensorium-fabric/            # impl Pneuma<ExternalToL0> — typed pub/sub
├── sensorium-schema/            # Tag catalog, ISA-18.2 alarm semantics
├── sensorium-qos/               # QoS negotiation, compatibility matrix
├── sensoriumd/                  # Daemon on localhost:3004
├── arcan-sensorium/             # Arcan bridge — register_pneuma_tools::<ExternalToL0>
├── sensorium-lago/              # Tiered persistence — fs for media, journal for events
├── life-sensorium/              # Umbrella re-export
│
├── publishers/
│   ├── sensorium-screen/        # ScreenCaptureKit (macOS), xcap (cross-platform)
│   ├── sensorium-audio/         # CoreAudio, cpal — mic, system audio, BLE audio
│   ├── sensorium-modbus/        # tokio-modbus — TCP/RTU, PLCs, VFDs, meters
│   ├── sensorium-opcua/         # opcua crate — SCADA historians, DCS, MES
│   ├── sensorium-mqtt/          # rumqttc + Sparkplug B — IoT sensors, edge
│   ├── sensorium-ble/           # btleplug — Omi wearable, health sensors
│   ├── sensorium-canbus/        # socketcan — vehicles, heavy machinery
│   ├── sensorium-serial/        # tokio-serial — legacy instruments
│   ├── sensorium-opsis/         # Opsis SSE bridge (replaces arcan-opsis injector)
│   └── sensorium-network/       # HTTP/SSE/WebSocket/gRPC generic
│
└── transformers/
    ├── sensorium-ocr/           # Apple Vision, Tesseract
    ├── sensorium-vad/           # Silero VAD via ONNX Runtime
    ├── sensorium-whisper/       # whisper-rs
    ├── sensorium-embed/         # Local text/image embeddings
    ├── sensorium-scale/         # Analog unit scaling, deadband
    └── sensorium-alarm/         # ISA-18.2 alarm state machine
```

Dependency direction: publishers depend on `sensorium-core`; the fabric depends on core + schema + qos; daemon wires them; Arcan bridge depends on aios-protocol + sensorium-core only.

## Relationship to Plexus

Sensorium and `life-plexus` are sibling Pneuma implementations — new crates with their own Signal/Aggregate/Directive types and their own transport substrate. They are *not* cousins of the vertical retrofits (lago, autonomic, egri, bstack-policy) which add Pneuma impls on top of pre-existing types.

### Shared by the trait family

- Same `Pneuma` trait surface (`emit`/`aggregate`/`receive`/`substrate`).
- Same Arcan integration via `register_pneuma_tools::<B>`.
- Same `MockPneuma` testing pattern.
- Same `SubstrateProfile` contract for depth-(k+1) planners.
- Same `PneumaError` failure taxonomy.

### Touching points (real data flow)

Sensorium and Plexus interact only through Arcan (L0 cognition). They do not share substrate.

**Sensorium observation triggers Plexus recruitment:**
```
Modbus register alarm → SensorySignal → SensoriumFabric.emit()
    → Arcan reads PerceptionState.aggregate()
    → Arcan decides to recruit maintenance capability
    → Arcan tool: plexus.emit(PlexusSignal::Recruit { ... })
    → Plexus propagates, formation emerges
```

**Plexus collective directive adjusts Sensorium attention:**
```
PlexusFabric aggregates PopulationState showing conservation quorum
    → CollectiveDirective::BroadcastNarrative reaches individual agent
    → Arcan tool: sensorium.receive_hint(AttentionDirective::AdjustRate { ... })
    → SensoriumFabric downshifts QoS for routine telemetry
```

Both eventually persist through `lago-journal` (Pneuma<L0ToL1>) and emit OpenTelemetry spans through Vigil.

### Gaps (where they differ, with design implications)

| Gap | Sensorium | Plexus | Implication |
|---|---|---|---|
| Temporal scale | ms–s (video, poll) | s–min (field decay) | Sensorium needs tiered persistence; Plexus doesn't |
| Extensibility | Open payload (`Custom` variants) | Closed enum | Can't share payload abstractions; fine for now |
| Trust boundary | External (untrusted) | Peer agents (identity-verified) | Aegis must enforce different policies per boundary |
| Directive semantics | Affects inbound flow (reconfiguration) | Affects outbound behavior (actuation) | Reversibility model differs; document explicitly |
| Substrate composition | Composite (one `SubstrateKind` per active publisher — always `Hybrid`) | Single (one transport) | Sensorium's `substrate()` must aggregate `WarpFactors` across publishers; `SubstrateKind::Hybrid` handles this |
| Discovery mechanism | Sparkplug-style `$birth` topics | AgentLocus broadcasts | No unified discovery; future `life-discovery` crate could reconcile |

### Missing boundaries (reserved, not implemented)

The reserved markers `PerceptToField`, `ExternalToD1`, `FieldToPercept` capture legitimate future boundaries (multi-agent sensor fusion, hive perception, environmental feedback) but are not required for the initial implementation. They are named in `aios-protocol` to claim the namespace and communicate architectural intent.

## Validation plan

The implementation must demonstrate that the abstraction truly covers arbitrary input. Four validation targets cover consumer, digital, industrial, and wearable domains:

### V1 — Opsis bridge (digital, already-structured)

Replace `arcan-opsis::WorldStateInjector` with `sensorium-opsis` publisher. Topic mapping: `world/delta/<domain>/<severity>`. Proves the fabric works with pre-existing SSE-based feeds and does not regress the current Arcan integration.

### V2 — Desktop screen awareness (consumer, media)

`sensorium-screen` publisher + `sensorium-ocr` transformer + `sensorium-embed` transformer. Topic: `desktop/screen/display-0`. Proves:
- High-bandwidth media through tiered persistence (frames → `lago-fs`, OCR text → `lago-journal`).
- Transformer DAG composes correctly (raw frame → OCR → embedding).
- `media_frame` QoS profile drops under backpressure without error.

### V3 — Modbus TCP on simulated PLC (industrial, structured)

`sensorium-modbus` publisher polling a Modbus simulator (e.g., `diagslave`) at 10ms intervals for 50 registers. `sensorium-scale` transformer applies engineering unit scaling. `sensorium-alarm` transformer evaluates ISA-18.2 limits. Proves:
- Cardinality: one connection → many topics.
- Tag catalog drives schema.
- Deadband suppresses noise.
- Alarm state machine produces transitions as separate topic.

### V4 — Omi BLE audio (wearable, streaming + ML)

`sensorium-ble` publisher connecting to Omi firmware (replicating the BLE GATT streaming protocol). Topic: `wearable/omi/audio/raw`. `sensorium-vad` transformer → `wearable/omi/audio/speech`. `sensorium-whisper` transformer → `wearable/omi/audio/transcript`. Proves:
- BLE transport works (btleplug).
- Silero VAD via ONNX Runtime runs on-device.
- Heavy transformer (Whisper) respects `ComputeCost::Heavy` budget gating.
- Sparkplug state: wearable disconnect → `SourceState::Dead` reaches L0.

Each validation target produces an integration test that exercises the full path from external source to Arcan tool invocation. V1 is the minimal viable fabric; V4 is the most stressful end-to-end.

## Sequencing

### Phase 0 — Prerequisites

1. Pneuma trait lands in `aios-protocol` (tracked separately; this spec depends on it).
2. `lago-journal` implements `Pneuma<L0ToL1>` (tracked in `pneuma-vertical-retrofits.md`).

### Phase 1 — Fabric foundation

3. Add `ExternalToL0` boundary marker to `aios-protocol::pneuma`.
4. Add reserved markers (`PerceptToField`, `ExternalToD1`, `FieldToPercept`) as documentation-only.
5. Scaffold `sensorium-core` with `SensorySignal`, `PerceptionState`, `AttentionDirective`, `KeyExpr`, QoS types, `Publisher`/`Transformer` traits.
6. Scaffold `sensorium-qos` with negotiation matrix.
7. Scaffold `sensorium-schema` with tag catalog and ISA-18.2 types.
8. Implement `sensorium-fabric` with in-process pub/sub (tokio channels). Defer Zenoh until cross-host transport is needed.
9. Implement `impl Pneuma<ExternalToL0> for SensoriumFabric`.
10. Unit tests: fabric emit/aggregate/receive, QoS negotiation, schema validation, quality propagation.

### Phase 2 — Lago persistence and Arcan bridge

11. Scaffold `sensorium-lago` with tiered persistence (`lago-fs` for media, `lago-journal` for events).
12. Scaffold `arcan-sensorium` with `register_pneuma_tools::<ExternalToL0>`.
13. `arcand` accepts `Arc<dyn Pneuma<B = ExternalToL0>>` injection.
14. Integration test: Arcan tool emits `AttentionDirective`; fabric reconfigures; subsequent signals respect new policy.

### Phase 3 — V1 validation (Opsis)

15. `sensorium-opsis` publisher — SSE subscription + `BirthCertificate`.
16. Migrate `arcan-opsis::WorldStateInjector` callers to subscribe through the fabric.
17. V1 integration test: Opsis → fabric → Arcan, with parity against existing behavior.
18. Deprecate `arcan-opsis` hand-rolled injector (keep crate for backwards compatibility; redirect to fabric).

### Phase 4 — V2 validation (screen)

19. `sensorium-screen` publisher — ScreenCaptureKit on macOS, xcap on Linux/Windows.
20. `sensorium-ocr` transformer — Apple Vision binding on macOS, Tesseract fallback.
21. `sensorium-embed` transformer — local text embeddings via `fastembed-rs`.
22. V2 integration test: screen capture → OCR → embedding → Arcan search tool.

### Phase 5 — V3 validation (Modbus)

23. `sensorium-modbus` publisher — `tokio-modbus` with tag catalog loader (TOML).
24. `sensorium-scale` transformer — linear scaling, engineering units.
25. `sensorium-alarm` transformer — ISA-18.2 alarm state machine.
26. V3 integration test: `diagslave` simulator → fabric → Arcan with alarm handling.

### Phase 6 — V4 validation (Omi BLE)

27. `sensorium-ble` publisher — btleplug with Omi GATT profile.
28. `sensorium-vad` transformer — Silero VAD via ONNX Runtime.
29. `sensorium-whisper` transformer — `whisper-rs` with model management.
30. V4 integration test: Omi wearable (or mock) → fabric → Arcan transcription tool.

### Phase 7 — Remaining publishers (parallel, as needed)

31. `sensorium-audio` (CoreAudio, cpal — system microphone, system audio).
32. `sensorium-opcua` (industrial).
33. `sensorium-mqtt` (+ Sparkplug B).
34. `sensorium-canbus`, `sensorium-serial`, `sensorium-network`.

### Phase 8 — Daemon and operational surface

35. `sensoriumd` daemon on `localhost:3004` — exposes fabric over HTTP/gRPC for out-of-process agents.
36. Operational CLI: list topics, inspect birth certificates, replay from `lago-fs`.
37. Vigil integration: OpenTelemetry spans on `emit`/`aggregate`/`receive`.

## Non-goals

- **Not replacing Opsis.** Opsis is the world model (geospatial, causal, domain-aware). Sensorium is the sensory organs feeding it. Sensorium publishes into Opsis; Opsis re-publishes back as a Sensorium source. Clean boundary.
- **Not replacing Spaces.** Spaces is human↔agent and agent↔agent communication. Sensorium is world→agent perception. The `sensorium-ble` publisher's Omi wearable produces world→agent signals; its chat transcripts are separate from Spaces channels.
- **Not a new Pneuma for outbound actuation.** Sensorium handles perception (inbound). Outbound actuation (turning a valve, sending a message) remains Praxis's domain. A future `L0ToExternal` boundary could be added if actuation patterns warrant it, but is out of scope here.
- **Not an in-kernel model executor.** Heavy ML (Whisper, image embeddings) runs in the transformer process, not in the fabric itself. Fabric is transport; transformers are compute.
- **Not Zenoh-mandated.** Phase 1 uses in-process tokio channels. Zenoh becomes a substitute fabric implementation if cross-host transport is needed, swappable behind the `SensoriumFabric` trait. The Pneuma surface does not change.

## Open questions

1. **Fabric substitutability.** Should `SensoriumFabric` itself be a trait so Zenoh and in-process impls are swappable at construction time, or should the in-process impl be the canonical `SensoriumFabric` with Zenoh as a future replacement crate? Leaning toward the latter — YAGNI until cross-host need is concrete.

2. **Backpressure semantics per `SignalKind`.** `media_frame` drops; `critical_alarm` blocks publisher. What about intermediate classes (routine analog telemetry)? A per-kind default plus per-topic override is likely needed, but the precise matrix is deferable to first integration test.

3. **Tag catalog distribution.** For Modbus, the catalog is a TOML file alongside the publisher config. For OPC-UA, the catalog can be derived from server browse. For MQTT, it needs explicit declaration. The core trait doesn't prescribe distribution — should it? Probably not (same rationale as Pneuma not prescribing transport).

4. **Arcan tool shape.** What's the tool surface Arcan exposes for Sensorium? Low-level (`emit_attention_directive`) or high-level (`focus_on`, `ignore`, `sample_frame`)? Both, probably — low-level for programmatic control, high-level for LLM-driven reasoning. Specific shape deferable until V2 integration.

5. **Lifecycle propagation.** When a publisher's `birth` advertises 1000 topics and then one topic's source disappears (one PLC register becomes unreadable), does the whole publisher go to `Dead` or just that topic? Probably per-topic with per-source aggregate — but this needs concrete handling in V3.

6. **Persistence retention.** Raw media in `lago-fs` grows fast. Retention policy (GC old chunks, tier to cold storage) is needed but not scoped in this spec — handle in a follow-up `sensorium-retention` specification.

7. **Conformance tests across Pneuma impls.** Should there be a shared test suite that every Pneuma impl must pass (round-trip emit, aggregate consistency under load, directive delivery guarantees)? Yes, but belongs in the Pneuma trait surface spec, not here.

## Companion specifications (to be produced in sequence)

- `core/life/docs/specs/sensorium-qos-matrix.md` — full QoS negotiation matrix with failure modes.
- `core/life/docs/specs/sensorium-tag-catalog.md` — schema registry TOML format, ISA-18.2 alarm configuration.
- `core/life/docs/specs/sensorium-persistence.md` — tiered persistence contract between fabric, `lago-fs`, and `lago-journal`.
- `core/life/docs/specs/sensorium-arcan-tools.md` — Arcan tool surface for perception operations.
- Per-publisher specs (one per crate) authored alongside implementation.

## References

- Pneuma/Plexus architecture: `core/life/docs/specs/pneuma-plexus-architecture.md`
- Pneuma trait surface: `core/life/docs/specs/pneuma-trait-surface.md`
- Vertical retrofits: `core/life/docs/specs/pneuma-vertical-retrofits.md`
- Plexus implementation: `core/life/docs/specs/life-plexus-implementation.md`
- Opsis feed ingestor (reference pattern): `core/life/crates/opsis/opsis-core/src/feed.rs`
- Arcan-opsis injector (to be replaced): `core/life/crates/arcan/arcan-opsis/src/injector.rs`
- Consciousness event loop (integration target): `core/life/crates/arcan/arcand/src/consciousness.rs`
- Omi research reference (patterns borrowed): `docs/references/omi-research.md`
- RCS foundations: `research/rcs/papers/p0-foundations/main.tex`
- Horizontal stability (sibling context): `research/rcs/papers/p6-horizontal-composition/README.md`
