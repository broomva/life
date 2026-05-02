# Spec C₃ §6.5 amendment — WebSocket close-code policy

**Date:** 2026-04-29 (carried forward to 2026-05-01 in M7 Sub-phase E)
**Author:** lifegw maintainers
**Status:** **AMENDS** the formal Spec C₃ §6.5 definition that lives in
Linear ticket BRO-922 body. This file is the authoritative source for
the close-code policy until BRO-922 is updated.

## Status note

The formal Spec C₃ design document does NOT live in this repository. It
lives in the body of Linear ticket **BRO-922** (M6 Spec C₃ — lifegw
edge gateway design). When agents reference "Spec C₃" in code comments
or commit messages, they are referencing decisions locked in BRO-922.

This file (`docs/superpowers/specs/2026-04-29-spec-c3-close-codes.md`)
amends BRO-922 §6.5 specifically. Any close-code definition mismatch
between the BRO-922 body and this file should be resolved by treating
this file as authoritative until the next M-phase rebases the Linear
spec body.

## Close-code policy (lifegw `/v1/agent/stream` WS)

The following table is the canonical close-code mapping the gateway
emits on the WebSocket upgraded at `/v1/agent/stream`. Codes 0-2999
are RFC 6455 reserved; codes 3000-3999 are IANA-registered protocol
codes; codes 4000-4999 are application-defined.

| Code | `CloseReason` variant | When emitted | Reason payload prefix |
|---:|---|---|---|
| 1000 | `Normal` | Client-initiated graceful close, OR upstream `tonic::Code::Cancelled`/`Aborted` (treated as graceful by the bidi pump). | `normal` |
| 1001 | `GoingAway` | Server is shutting down (gateway drain). | `going_away` |
| 1008 | `PolicyViolation` | Tier-1 token expired mid-stream (`Unauthenticated` or `PermissionDenied` from the auth layer). Mapped to 1008 rather than 1011 so operator dashboards distinguish auth violations from server faults. | `policy_violation:token_expired` |
| 1011 | `InternalError` | Server-side fault — heartbeat-pong-deadline expired (Sub-phase D D5) OR unhandled upstream `tonic::Code::Internal` / non-listed code. | `internal_error` |
| 4001 | `RateLimit` | Per-user OR per-IP token-bucket budget exhausted (Sub-phase D D1). Maps `tonic::Code::ResourceExhausted` from the rate-limit layer. | `rate_limit:per_user` |
| 4002 | `SlowConsumer` | Backpressure — outbound mpsc(64) backed up `STALLED_THRESHOLD` consecutive ticks; gateway closes to free memory before OOM. | `backpressure:slow_consumer` |
| 4003 | `IpBlocked` | Peer IP on the in-process blocklist (Sub-phase D D2 admin RPC `BlocklistAdd`). | `ip_blocked` |
| 4004 | `LifedUnavailable` | Upstream lifed UDS unreachable / circuit breaker open (`tonic::Code::Unavailable`). | `lifed_circuit_open` |
| 4005 | `SequenceRetired` | Client `from_sequence` lifed already evicted (`tonic::Code::OutOfRange`). | `sequence_retired` |

**Note on 1003 (Unsupported Data):** Sub-phase D D9's dispatcher rejects
unknown inbound frame kinds by **dropping them silently** rather than
closing the connection — see the `inbound_frame_drops_unknown_kind`
test at `crates/life-runtime/lifegw/src/services/ws.rs`. Forward-compat
clients can keep the connection alive while introducing a frame kind
the gateway hasn't shipped support for. If a future sub-phase decides
to close 1003 instead, this row gets added to the table.

## Mapping table — tonic `Status::Code` → `CloseReason`

The bidi pump consults `services::ws::map_status_to_close` to translate
upstream gRPC errors into the variants above. As of M7 Sub-phase D the
mapping is:

| `tonic::Code` | `CloseReason` | WS close code |
|---|---|---:|
| `Unauthenticated` | `PolicyViolation` | 1008 |
| `PermissionDenied` | `PolicyViolation` | 1008 |
| `ResourceExhausted` | `RateLimit` | 4001 |
| `Unavailable` | `LifedUnavailable` | 4004 |
| `OutOfRange` | `SequenceRetired` | 4005 |
| `Cancelled` | `Normal` | 1000 |
| `Aborted` | `Normal` | 1000 |
| anything else (incl. `Internal`) | `InternalError` | 1011 |

The `Cancelled`/`Aborted` → `Normal` (1000) pair reflects the production
semantics: when a client cancels mid-stream we propagate the cancel as
a graceful close rather than treating it as an error condition. The
`InternalError` default (1011) covers `tonic::Code::Internal` plus
every other tonic code variant the gateway hasn't enumerated explicitly
— a defense against a future tonic version adding new codes.

## Reason payload contract

The text after the colon in `Reason payload prefix` is **stable for
operator runbook purposes**. New variants are added at the END of
the colon-separated tuple; the prefix stays stable so dashboards
keyed off `rate_limit:per_user` continue working when a future code
adds `rate_limit:per_user:tier=pro` granularity.

## Code 4xxx range reservation

The 4001-4099 sub-range is reserved for `lifegw`-emitted codes. Any
future client-emitted code must use 4100-4199. This split keeps the
operator-side dashboards (which key on `4001-4099`) unambiguous about
the source of a close.

## Test coverage

The mapping is asserted in `crates/life-runtime/lifegw/src/services/ws.rs`
unit tests:

- `close_reason_codes_match_spec` — every variant maps to the integer
  in the table above.
- `map_status_to_close_handles_known_codes` — tonic `Code` →
  `CloseReason` round-trip per the §6.5 mapping table.

The Sub-phase E chaos battery (`tests/chaos_*.rs`) exercises the
behavioural side: rate-limit surfaces 4001, lifed-down surfaces 4004,
slow consumer surfaces 4002.

## References

- Linear BRO-922 body — formal Spec C₃ design doc (not in repo)
- `crates/life-runtime/lifegw/src/services/ws.rs` — implementation
- `crates/life-runtime/lifegw/tests/integration_ws_bidi.rs` — e2e
- `crates/life-runtime/lifegw/tests/chaos_*.rs` — Sub-phase E chaos battery
