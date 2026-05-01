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

| Code | Name | When emitted | Reason payload prefix |
|---:|---|---|---|
| 1000 | Normal | Client-initiated graceful close. | (none) |
| 1001 | Going away | Server is shutting down (gateway drain). | `going_away` |
| 1003 | Unsupported data | Client sent a frame `lifegw` does not understand (Sub-phase D D9 dispatcher rejects unknown frame kinds). | `unsupported:<frame_kind>` |
| 1008 | Policy violation | Client sent a frame that violates the protocol contract (e.g. text body must be valid JSON; `Subscribe` from outside the per-session pump). | `policy:<violation>` |
| 1011 | Internal error | Server-side fault (heartbeat timeout / unhandled upstream error). Sub-phase D D5 maps a missing-pong window onto this. | `server_error:<reason>` |
| 4001 | Rate limit | Per-user OR per-IP token-bucket budget exhausted (Sub-phase D D1). Maps `tonic::Status::resource_exhausted` from the auth layer. | `rate_limit:per_user` or `rate_limit:per_ip` |
| 4002 | Backpressure | Slow consumer — outbound mpsc(64) backed up; gateway closes to free memory before OOM. | `backpressure:slow_consumer` |
| 4003 | IP blocked | The peer's IP is on the in-process blocklist (Sub-phase D D2 admin RPC `BlocklistAdd`). | `ip_blocked:<reason>` |
| 4004 | lifed unavailable | Upstream lifed UDS is unreachable / circuit breaker open. | `lifed_unavailable:<reason>` |
| 4005 | Sequence retired | Client requested resume from a `from_sequence` lifed has already evicted (lifed responds `out_of_range`). | `sequence_retired:from_sequence=<n>` |

## Mapping table — tonic `Status::Code` → WS close

The auth layer + bidi pump consult `services::ws::map_status_to_close`
to translate upstream gRPC errors into the table above. Sub-phase D
finalised the mapping:

| `tonic::Code` | `CloseReason` | WS close code |
|---|---|---:|
| `Unauthenticated` | `Auth` (RateLimit slot used pre-D5; now mapped to 1011 with the `auth_failed:` prefix) | 1011 |
| `PermissionDenied` | `Auth` | 1011 |
| `ResourceExhausted` | `RateLimit` | 4001 |
| `Unavailable` | `LifedUnavailable` | 4004 |
| `OutOfRange` | `SequenceRetired` | 4005 |
| `Aborted` | `SlowConsumer` (gateway-side abort during mpsc overflow) | 4002 |
| `Internal` (default) | `InternalError` | 1011 |

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
