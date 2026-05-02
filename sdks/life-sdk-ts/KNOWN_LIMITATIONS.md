# Known Limitations — `@broomva/life-sdk` v0.1.0-pre

This SDK ships as **v0.1.0-pre** — the structural foundation (services,
typed errors, WS state machine, proto type mirrors, codec helpers,
anima browser custody, 125 unit tests) is solid and reusable. One
wire-protocol limitation remains documented here as a known follow-up
before v0.2.0 / GA. The browser-custody work in D-Sub-C Stream T
SIDESTEPS the M8.2 issue (see "M8.2 status" section below).

## B1 — SDK speaks Connect-protocol JSON; lifegw runs `tonic-web` (gRPC-Web binary)

**Status:** Known limitation. v0.1.0-pre will NOT successfully complete
unary or server-streaming calls against a real lifegw deployment. Tests
pass against the in-tree `FakeGateway` test helper because it accepts
both content-types.

### What's wrong

- The SDK's `Transport` sends `Content-Type: application/json` (unary)
  and `Content-Type: application/connect+json` (server-streaming) —
  the Connect protocol JSON wire format
  ([connectrpc.com/docs/protocol](https://connectrpc.com/docs/protocol)).
- The lifegw gateway mounts `tonic_web::GrpcWebLayer` (per
  `crates/life-runtime/lifegw/src/bootstrap.rs`) which accepts ONLY
  `application/grpc-web`, `application/grpc-web+proto`,
  `application/grpc-web-text`, `application/grpc-web-text+proto`. The
  inner tonic stack expects binary-framed protobuf. There is no
  Connect-protocol layer in the lifegw stack.
- Result: every call from this SDK to a real lifegw returns
  `415 Unsupported Media Type` or similar.

### Resolution paths (for v0.2.0)

- **Option A (preferred):** Switch SDK transport to gRPC-Web binary
  framing with `@bufbuild/protobuf` codec. Connect-ES already supports
  `grpc-web` mode — the public API surface stays the same; only
  `Transport`'s internal framing changes. Adds one runtime dep.
- **Option B:** Add a Connect-protocol translation layer to lifegw.
  Connect-rs is young; this is the bigger lift but doesn't require an
  SDK rebuild.
- **Option C:** Add a lightweight JSON-to-grpc-web translation proxy
  service that the SDK calls instead of lifegw directly. Adds a hop.

Option A is the canonical path for SDKs targeting tonic-backed
gateways. Tracked under follow-up ticket "M8.1 — SDK transport rework
to gRPC-Web binary framing".

## M8.2 status — RESOLVED for browser custody (D-Sub-C Stream T)

The original M8.2 entry in v0.1.0-pre identified the
`Sec-WebSocket-Protocol: bearer.*` subprotocol mismatch on the WS
upgrade path. D-Sub-C (Stream T) closes the **production-equivalent
gap** for browser identity custody by introducing the
`/anima/custody/*` HTTP/JSON route family — see `src/anima/`. Browser
deployments mint a Tier-User capability via passkey + HTTP/JSON,
then call wallet operations through the same plain-`fetch`
transport. The custody RPC code is structurally separate from the
existing tonic-web SDK paths (M8.1 transport rework remains
independent), so M8.1 and M8.2 land independently.

The original WS-subprotocol handshake (`Agent.StreamSession`) in the
gateway is unchanged — long-lived agent streaming still uses WS, and
that path still requires a `webSocketFactory` in Node. The custody
path no longer needs WS, so the gateway fix tracked as M8.2 is
deferred until WS-bearer subprotocol parsing becomes a non-custody
necessity.

## Migration path for early adopters

Until B1 lands (M8.2 sidestepped per above), the SDK is usable for:

- Tests against `FakeGateway`-style mocks.
- Node hosts that pass a `webSocketFactory` setting `Authorization`
  via the `ws` package.
- Anyone building on top of the typed surface (services, errors, WS
  state machine, proto types) — none of which change in v0.2.0.

Browser production deployments using `WebCryptoAnima` (custody only,
no WS-streaming) are unblocked once Stream R's `/anima/custody/*`
routes ship. Browser production deployments needing
`Agent.StreamSession` over WS still require either Node-host wrapping
or M8.1 transport rework.

## See also

- Spec C₃ §6.5 close-codes: `docs/superpowers/specs/2026-04-29-spec-c3-close-codes.md`
- lifegw bootstrap: `crates/life-runtime/lifegw/src/bootstrap.rs`
- lifegw WS upgrade handler: `crates/life-runtime/lifegw/src/services/ws.rs`
- lifegw auth middleware: `crates/life-runtime/lifegw/src/auth/middleware.rs`
