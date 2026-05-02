# Known Limitations — `@broomva/life-sdk` v0.1.0-pre

This SDK ships as **v0.1.0-pre** — the structural foundation (services,
typed errors, WS state machine, proto type mirrors, codec helpers, 50
unit tests) is solid and reusable. Two wire-protocol limitations are
documented here as known follow-ups before v0.2.0 / GA:

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

## B2 — Browser WebSocket auth uses subprotocol; gateway reads only `Authorization` header

**Status:** Browser path of `Agent.StreamSession` will fail Tier-1 auth
on production lifegw without a gateway-side change. Node hosts can pass
a `webSocketFactory` that sets the `Authorization` header on the
underlying `ws` package.

### What's wrong

- Browsers cannot set arbitrary headers on the WS upgrade request — the
  Fetch API doesn't expose `Authorization` for the WS path.
- The SDK forwards the Tier-1 bearer token via the
  `Sec-WebSocket-Protocol: bearer.<token>` subprotocol header, which
  IS settable from the browser.
- The lifegw gateway's `services::ws::parse_upgrade_request` reads ONLY
  the `Authorization` header. The subprotocol is silently ignored.
- The `auth/middleware.rs::AuthLayer` runs BEFORE `WsLayer` and rejects
  upgrade requests without a `Bearer` token in `Authorization` — the
  upgrade response is never sent.

### Resolution paths (for v0.2.0)

- **Option A (preferred):** Add `Sec-WebSocket-Protocol: bearer.*`
  parsing to lifegw's `parse_upgrade_request` + `AuthLayer`. Mirrors
  the SDK's existing approach. Gateway-side change only.
- **Option B:** Use cookie-based auth for WS. The gateway sets an
  HttpOnly cookie at login; the browser includes it automatically on
  upgrade requests. Requires session-management work and CSRF
  defenses.
- **Option C:** Use a query-param token for WS only. Less secure
  (tokens may leak into logs); only suitable for short-lived tokens.

Option A is the lowest-friction. Tracked under follow-up ticket
"M8.2 — lifegw WS subprotocol-bearer auth support".

## Migration path for early adopters

Until B1+B2 land, the SDK is usable for:

- Tests against `FakeGateway`-style mocks.
- Node hosts that pass a `webSocketFactory` setting `Authorization`
  via the `ws` package.
- Anyone building on top of the typed surface (services, errors, WS
  state machine, proto types) — none of which change in v0.2.0.

Avoid using v0.1.0-pre for browser production deployments until
B1+B2 are resolved.

## See also

- Spec C₃ §6.5 close-codes: `docs/superpowers/specs/2026-04-29-spec-c3-close-codes.md`
- lifegw bootstrap: `crates/life-runtime/lifegw/src/bootstrap.rs`
- lifegw WS upgrade handler: `crates/life-runtime/lifegw/src/services/ws.rs`
- lifegw auth middleware: `crates/life-runtime/lifegw/src/auth/middleware.rs`
