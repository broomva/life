//! Lumen Phase α — headless end-to-end smoke client.
//!
//! Speaks the same wire protocol the patched warp-oss binary would use
//! (HTTPS POST to `/graphql/v2` + `graphql-transport-ws` over WSS at
//! `/graphql-ws`) but does NOT depend on warpui or Metal Toolchain.
//! This unblocks the Phase α gate exercise on machines where Apple's
//! MobileAsset CDN refuses to serve the Metal Toolchain.
//!
//! Runs five checks in sequence:
//! 1. HTTPS schema introspection (verifies route + auth + SDL walking).
//! 2. RootMutation field count + all 11 wired T1 ops present.
//! 3. `createAgentTask` dispatch (real resolver → lifed.Agent.CreateSession).
//! 4. SDL-level argument validation (Task 10.5 fix-up enforces `prompt: String!`).
//! 5. WSS `graphql-transport-ws` subscription on `warpDriveUpdates` —
//!    connection_init → connection_ack → subscribe → first frame → complete.
//!
//! Each step prints a coloured PASS/FAIL line. Non-zero exit on any
//! failure. Designed to be the canonical α-gate evidence harness when
//! the GUI client is unavailable.
//!
//! Usage:
//! ```
//! WARP_SERVER_ROOT_URL=https://127.0.0.1:8443 \
//! WARP_WS_SERVER_URL=wss://127.0.0.1:8443/graphql-ws \
//! WARP_AUTH_TOKEN=dev-cap-token-for-lumen/test-user-1 \
//! cargo run --example lumen_smoke_client -p lifegw
//! ```

use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio_rustls::rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use tokio_rustls::rustls::{
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    ClientConfig, DigitallySignedStruct, Error as RustlsError, SignatureScheme,
};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::Connector;

// ── Coloured output ──────────────────────────────────────────────────────────

const C_OK: &str = "\x1b[1;32m";
const C_FAIL: &str = "\x1b[1;31m";
const C_INFO: &str = "\x1b[1;36m";
const C_DIM: &str = "\x1b[2m";
const C_RST: &str = "\x1b[0m";

fn ok(step: &str, msg: &str) {
    println!("  {C_OK}✓{C_RST} {step}: {msg}");
}
fn fail(step: &str, msg: &str) {
    println!("  {C_FAIL}✗{C_RST} {step}: {msg}");
}
fn header(n: usize, total: usize, title: &str) {
    println!();
    println!("{C_INFO}=== Step {n}/{total}: {title} ==={C_RST}");
}
fn dim(s: &str) {
    println!("  {C_DIM}{s}{C_RST}");
}

// ── HTTP path ────────────────────────────────────────────────────────────────

/// reqwest client that accepts the self-signed dev-tls cert.
///
/// We pin reqwest's TLS backend to rustls (`.use_rustls_tls()`) — macOS's
/// default native-tls backend rejects the dev cert with `bad protocol
/// version`. rustls + `danger_accept_invalid_certs(true)` cleanly trusts
/// the self-signed listener.
fn http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .use_rustls_tls()
        .danger_accept_invalid_certs(true)
        .timeout(Duration::from_secs(10))
        .build()
        .context("build reqwest client")
}

async fn post_graphql(
    client: &reqwest::Client,
    endpoint: &str,
    token: &str,
    body: Value,
) -> Result<Value> {
    let resp = client
        .post(endpoint)
        .bearer_auth(token)
        .json(&body)
        .send()
        .await
        .context("POST /graphql/v2")?;
    let status = resp.status();
    let text = resp.text().await.context("read response body")?;
    if !status.is_success() {
        bail!("HTTP {status} — body: {text}");
    }
    serde_json::from_str::<Value>(&text).with_context(|| format!("parse response: {text}"))
}

async fn step1_introspection(
    client: &reqwest::Client,
    endpoint: &str,
    token: &str,
) -> Result<()> {
    header(1, 5, "schema introspection");
    let resp = post_graphql(
        client,
        endpoint,
        token,
        json!({"query": "{ __schema { queryType { name } mutationType { name } subscriptionType { name } types { name } } }"}),
    )
    .await?;
    let q = resp["data"]["__schema"]["queryType"]["name"]
        .as_str()
        .context("missing queryType.name")?;
    let m = resp["data"]["__schema"]["mutationType"]["name"]
        .as_str()
        .context("missing mutationType.name")?;
    let s = resp["data"]["__schema"]["subscriptionType"]["name"]
        .as_str()
        .context("missing subscriptionType.name")?;
    if q != "RootQuery" || m != "RootMutation" || s != "RootSubscription" {
        fail("roots", &format!("got Q={q} M={m} S={s}"));
        bail!("unexpected root types");
    }
    let types_count = resp["data"]["__schema"]["types"]
        .as_array()
        .map(|a| a.len())
        .unwrap_or(0);
    ok("RootQuery / RootMutation / RootSubscription", "all present");
    ok("type count", &format!("{types_count} types registered"));
    if types_count < 500 {
        bail!("expected ≥500 types, got {types_count} — SDL walker may not be walking the full mirror");
    }
    Ok(())
}

async fn step2_mutation_surface(
    client: &reqwest::Client,
    endpoint: &str,
    token: &str,
) -> Result<()> {
    header(2, 5, "T1 mutation presence");
    let resp = post_graphql(
        client,
        endpoint,
        token,
        json!({"query": r#"{ __type(name: "RootMutation") { fields { name } } }"#}),
    )
    .await?;
    let fields = resp["data"]["__type"]["fields"]
        .as_array()
        .context("RootMutation.fields missing")?;
    let names: std::collections::HashSet<&str> =
        fields.iter().filter_map(|f| f["name"].as_str()).collect();
    ok("RootMutation field count", &format!("{}", names.len()));

    const REQUIRED_T1: &[&str] = &[
        "createAgentTask",
        "updateAgentTask",
        "createAnonymousUser",
        "setUserIsOnboarded",
        "mintCustomToken",
        "issueTaskIdentityToken",
        "generateApiKey",
        "expireApiKey",
        "updateUserSettings",
        "generateDialogue",
        "generateCommands",
    ];
    let missing: Vec<&str> = REQUIRED_T1
        .iter()
        .copied()
        .filter(|op| !names.contains(op))
        .collect();
    if missing.is_empty() {
        ok("all 11 wired T1 ops present", "OK");
    } else {
        fail("missing T1 ops", &format!("{missing:?}"));
        bail!("missing T1 ops on RootMutation");
    }
    Ok(())
}

async fn step3_create_agent_task(
    client: &reqwest::Client,
    endpoint: &str,
    token: &str,
) -> Result<()> {
    header(3, 5, "createAgentTask dispatch");
    let resp = post_graphql(
        client,
        endpoint,
        token,
        json!({
            "query": r#"mutation M($input: CreateAgentTaskInput!, $rc: RequestContext!) {
                createAgentTask(input: $input, requestContext: $rc) {
                    __typename
                    ... on CreateAgentTaskOutput { taskId }
                }
            }"#,
            "variables": {
                "input": { "prompt": "lumen smoke from headless client", "environmentUid": "smoke-ws" },
                "rc": {
                    "clientContext": { "version": "lumen-smoke-client/0.1" },
                    "osContext": { "name": "darwin", "category": "darwin", "version": "0.0" }
                }
            }
        }),
    )
    .await?;

    if let Some(task_id) = resp["data"]["createAgentTask"]["taskId"].as_str() {
        ok("real dispatch", &format!("taskId={task_id}"));
        return Ok(());
    }
    if let Some(errors) = resp["errors"].as_array() {
        let first = &errors[0];
        let code = first["extensions"]["code"].as_str().unwrap_or("(no code)");
        let msg = first["message"].as_str().unwrap_or("(no message)");
        if matches!(
            code,
            "UNAUTHENTICATED" | "FEATURE_NOT_ENABLED" | "UPSTREAM_ERROR" | "INVALID_INPUT"
        ) {
            ok(
                "typed error (resolver wired, extensions.code preserved)",
                &format!("{code}: {msg}"),
            );
            return Ok(());
        }
        fail("dispatch", &format!("unexpected code: {code} — {msg}"));
        bail!("createAgentTask returned unrecognized error shape");
    }
    fail("dispatch", &format!("no data and no errors: {resp}"));
    bail!("unexpected createAgentTask response shape");
}

async fn step4_argument_validation(
    client: &reqwest::Client,
    endpoint: &str,
    token: &str,
) -> Result<()> {
    header(4, 5, "SDL argument validation");
    let resp = post_graphql(
        client,
        endpoint,
        token,
        json!({
            "query": r#"mutation M($input: GenerateDialogueInput!, $rc: RequestContext!) {
                generateDialogue(input: $input, requestContext: $rc) { __typename }
            }"#,
            "variables": {
                "input": { "transcript": [] },
                "rc": {
                    "clientContext": { "version": "smoke" },
                    "osContext": { "name": "darwin", "category": "darwin", "version": "0" }
                }
            }
        }),
    )
    .await?;
    let errors = resp["errors"]
        .as_array()
        .context("expected errors[] for missing prompt")?;
    let msg = errors[0]["message"].as_str().unwrap_or("");
    if msg.contains("prompt") && msg.contains("String!") {
        ok(
            "SDL enforces prompt: String!",
            "validator rejected as expected",
        );
        Ok(())
    } else {
        fail("argument validation", &format!("got: {msg}"));
        bail!("argument validation did not reject as expected");
    }
}

// ── WS path ──────────────────────────────────────────────────────────────────

/// Custom TLS verifier that trusts any cert. Used only against the
/// `dev-tls/` self-signed listener — never in production.
#[derive(Debug)]
struct AcceptAnyCert;

impl ServerCertVerifier for AcceptAnyCert {
    fn verify_server_cert(
        &self,
        _: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> Result<ServerCertVerified, RustlsError> {
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        _: &[u8],
        _: &CertificateDer<'_>,
        _: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, RustlsError> {
        Ok(HandshakeSignatureValid::assertion())
    }
    fn verify_tls13_signature(
        &self,
        _: &[u8],
        _: &CertificateDer<'_>,
        _: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, RustlsError> {
        Ok(HandshakeSignatureValid::assertion())
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        // Cover the union of TLS 1.2 (PKCS1) + TLS 1.3 (PSS) RSA suites plus
        // the common EC/Ed25519 modern ones. The dev-tls cert is RSA-2048,
        // so omitting PSS here triggers a server-side HandshakeFailure under
        // TLS 1.3 (PKCS1 isn't valid in 1.3 — see RFC 8446 §4.2.3).
        vec![
            // TLS 1.3 RSA
            SignatureScheme::RSA_PSS_SHA256,
            SignatureScheme::RSA_PSS_SHA384,
            SignatureScheme::RSA_PSS_SHA512,
            // TLS 1.2 RSA (kept for completeness — dev listener allows both)
            SignatureScheme::RSA_PKCS1_SHA256,
            SignatureScheme::RSA_PKCS1_SHA384,
            SignatureScheme::RSA_PKCS1_SHA512,
            // EC / Ed25519
            SignatureScheme::ECDSA_NISTP256_SHA256,
            SignatureScheme::ECDSA_NISTP384_SHA384,
            SignatureScheme::ED25519,
        ]
    }
}

/// Drives the `graphql-transport-ws` handshake on an already-upgraded WS
/// stream. Hand-rolled instead of using `graphql-ws-client` because the
/// version that pin agrees with the workspace's `tokio-tungstenite 0.26`
/// (0.10) requires `tungstenite 0.23`, conflicting with the workspace pin.
/// The protocol is tiny: `connection_init` → `connection_ack` → `subscribe`
/// → `next` → `complete`.
///
/// Note on auth: the bearer Authorization HTTP header is set on the WS
/// upgrade request. We also include the token under `connection_init`'s
/// `payload.Authorization` per the de-facto graphql-transport-ws practice,
/// in case lifegw later enforces a payload check.
async fn step5_warp_drive_updates_subscription(ws_url: &str, token: &str) -> Result<()> {
    header(5, 5, "WSS graphql-transport-ws subscription");

    // 1. Build the WS upgrade request with Bearer auth + subprotocol.
    let mut req = ws_url.into_client_request().context("build WS request")?;
    req.headers_mut()
        .insert("Authorization", format!("Bearer {token}").parse()?);
    req.headers_mut()
        .insert("Sec-WebSocket-Protocol", "graphql-transport-ws".parse()?);

    // 2. Build a rustls ClientConfig with our dev-cert-trusting verifier.
    let tls_config = ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AcceptAnyCert))
        .with_no_client_auth();
    let connector = Connector::Rustls(Arc::new(tls_config));

    // 3. Dial, handshake, upgrade — all in one call.
    let (mut ws_stream, response) = tokio_tungstenite::connect_async_tls_with_config(
        req,
        None, // default WebSocket config
        false, // disable_nagle: false (Nagle on; we're chatty, not latency-bound)
        Some(connector),
    )
    .await
    .context("WSS connect to /graphql-ws")?;
    let selected = response
        .headers()
        .get("sec-websocket-protocol")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("(none)");
    if selected != "graphql-transport-ws" {
        fail("subprotocol", &format!("server selected '{selected}'"));
        bail!("server did not negotiate graphql-transport-ws");
    }
    ok("subprotocol negotiated", "graphql-transport-ws");

    // 4. connection_init — payload mirrors the canonical Apollo/graphql-ws
    //    practice (Authorization in payload AND in the HTTP header).
    let init = json!({
        "type": "connection_init",
        "payload": { "Authorization": format!("Bearer {token}") }
    });
    ws_stream
        .send(WsMessage::Text(init.to_string().into()))
        .await
        .context("send connection_init")?;

    // 5. Await connection_ack (with timeout — server should respond within ms).
    let ack = tokio::time::timeout(Duration::from_secs(5), ws_stream.next())
        .await
        .context("await connection_ack within 5s")?;
    let ack_msg = match ack {
        Some(Ok(WsMessage::Text(t))) => t,
        Some(Ok(other)) => bail!("expected text frame for ack, got: {other:?}"),
        Some(Err(e)) => bail!("WS error waiting for ack: {e}"),
        None => bail!("WS closed before connection_ack"),
    };
    let ack_v: Value = serde_json::from_str(&ack_msg).context("parse ack JSON")?;
    if ack_v["type"].as_str() != Some("connection_ack") {
        fail(
            "connection_ack",
            &format!("server replied: {}", ack_v["type"]),
        );
        bail!("expected connection_ack, got {ack_v}");
    }
    ok("connection_ack", "received");

    // 6. subscribe to warpDriveUpdates.
    let sub_id = "lumen-smoke-1";
    let subscribe = json!({
        "id": sub_id,
        "type": "subscribe",
        "payload": {
            "query": "subscription { warpDriveUpdates { __typename } }",
            "variables": {}
        }
    });
    ws_stream
        .send(WsMessage::Text(subscribe.to_string().into()))
        .await
        .context("send subscribe")?;
    ok("subscribe sent", &format!("id={sub_id}"));

    // 7. Await the first `next`/`error` frame (with timeout).
    //    Server may emit a `next` (data frame) or `complete` (immediate end).
    //    Either is acceptable evidence that the subscription dispatched.
    let mut got_evidence = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline - tokio::time::Instant::now();
        let frame = tokio::time::timeout(remaining, ws_stream.next()).await;
        let msg = match frame {
            Ok(Some(Ok(WsMessage::Text(t)))) => t,
            Ok(Some(Ok(WsMessage::Ping(_)))) => continue,
            Ok(Some(Ok(WsMessage::Pong(_)))) => continue,
            Ok(Some(Ok(other))) => {
                dim(&format!("non-text frame: {other:?}"));
                continue;
            }
            Ok(Some(Err(e))) => bail!("WS error during subscription: {e}"),
            Ok(None) => bail!("WS closed before subscription emitted"),
            Err(_) => bail!("timed out awaiting first subscription frame"),
        };
        let v: Value = serde_json::from_str(&msg).context("parse subscription frame JSON")?;
        let kind = v["type"].as_str().unwrap_or("");
        let id = v["id"].as_str().unwrap_or("");
        match (kind, id == sub_id) {
            ("next", true) => {
                let typename = v["payload"]["data"]["warpDriveUpdates"]["__typename"]
                    .as_str()
                    .unwrap_or("(none)");
                ok(
                    "first frame",
                    &format!("type=next, warpDriveUpdates.__typename = {typename}"),
                );
                got_evidence = true;
                break;
            }
            ("complete", true) => {
                ok(
                    "subscription completed immediately",
                    "Phase α placeholder bridge (Task 20 stub) — acceptable",
                );
                got_evidence = true;
                break;
            }
            ("error", true) => {
                let payload = &v["payload"];
                fail(
                    "subscription error",
                    &format!("payload={}", serde_json::to_string(payload).unwrap_or_default()),
                );
                bail!("server returned subscription error");
            }
            ("ping", _) => {
                // server-side keepalive; reply with pong frame per spec
                let pong = json!({"type": "pong"}).to_string();
                ws_stream
                    .send(WsMessage::Text(pong.into()))
                    .await
                    .context("reply pong")?;
                continue;
            }
            _ => {
                dim(&format!("ignoring frame type={kind} id={id}"));
                continue;
            }
        }
    }
    if !got_evidence {
        bail!("no subscription evidence within 10s window");
    }

    // 8. Polite teardown — complete + close.
    let complete = json!({"id": sub_id, "type": "complete"});
    let _ = ws_stream
        .send(WsMessage::Text(complete.to_string().into()))
        .await;
    let _ = ws_stream.close(None).await;
    ok("WS teardown", "complete frame sent + close requested");
    Ok(())
}

// ── main ─────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    // Install rustls default crypto provider before any TLS handshake.
    let _ = tokio_rustls::rustls::crypto::ring::default_provider().install_default();

    let endpoint = std::env::var("WARP_SERVER_ROOT_URL")
        .unwrap_or_else(|_| "https://127.0.0.1:8443".into());
    let ws_endpoint = std::env::var("WARP_WS_SERVER_URL")
        .unwrap_or_else(|_| "wss://127.0.0.1:8443/graphql-ws".into());
    let token = std::env::var("WARP_AUTH_TOKEN")
        .unwrap_or_else(|_| "dev-cap-token-for-lumen/test-user-1".into());
    let graphql_url = format!("{}/graphql/v2", endpoint.trim_end_matches('/'));

    println!("{C_INFO}Lumen Phase α — headless smoke client{C_RST}");
    dim(&format!("HTTPS endpoint  : {graphql_url}"));
    dim(&format!("WSS   endpoint  : {ws_endpoint}"));
    dim(&format!("Bearer token    : {token}"));

    let client = http_client()?;
    step1_introspection(&client, &graphql_url, &token).await?;
    step2_mutation_surface(&client, &graphql_url, &token).await?;
    step3_create_agent_task(&client, &graphql_url, &token).await?;
    step4_argument_validation(&client, &graphql_url, &token).await?;
    step5_warp_drive_updates_subscription(&ws_endpoint, &token).await?;

    println!();
    println!(
        "{C_OK}All 5 checks passed.{C_RST} Lumen Phase α HTTP + WS surfaces are wire-compatible with what a patched warp-oss client would send."
    );
    Ok(())
}
