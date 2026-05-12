//! axum routes for the Warp-compatible GraphQL surface.
//!
//! Two endpoints:
//! - POST `/graphql/v2`   — queries + mutations
//! - WebSocket `/graphql-ws` — the single `warpDriveUpdates` subscription
//!
//! Both endpoints expect a valid Bearer JWT (audience = "lumen", issuer =
//! the configured Tier-2 issuer, typically "lifegw" per Spec C₃ §5) in
//! the `Authorization` header. The one exception is anonymous-mode used
//! by the `createAnonymousUser` mutation: when no Bearer is supplied, an
//! ephemeral [`WarpClaims::anonymous`] context is constructed and the
//! resolver gates downstream operations on `claims.has_scope(...)`.

use std::collections::HashMap;
use std::sync::Arc;

use async_graphql_axum::{GraphQLProtocol, GraphQLRequest, GraphQLResponse, GraphQLWebSocket};
use axum::{
    extract::{Query, State, WebSocketUpgrade},
    http::HeaderMap,
    response::{IntoResponse, Response},
    routing::{get, post},
    Router,
};

use crate::auth::JwksCache;
use crate::warp_graphql::{
    adapter::WarpSchema,
    auth::{extract_bearer, verify_to_claims, WarpClaims},
    context::WarpRequestCtx,
    dataloader::LifedDataloaders,
};

/// Per-request shared state mounted on the warp_graphql Router.
///
/// Built once at lifegw startup by `bootstrap::serve_with_listener_*`
/// and threaded through every handler via axum's `State` extractor.
/// All four fields are cheap to clone (Arc + Channel pools).
#[derive(Clone)]
pub struct WarpRouteState {
    pub schema: WarpSchema,
    pub jwks: Arc<JwksCache>,
    pub dataloaders: Arc<LifedDataloaders>,
    /// Issuer the JWKS verifier expects on inbound Tier-User caps.
    /// Per Spec C₃ §5.4 this is typically "lifegw". Read from
    /// `cfg.auth.tier2_issuer` at bootstrap so dev/staging/prod can
    /// differ. (Lumen caps are minted via the Tier-2 mint path, so the
    /// issuer matches `tier2_issuer`, not `tier1_issuer`.)
    pub expected_issuer: String,
}

/// Router segment for the warp_graphql endpoints. Mount into the lifegw
/// top-level Router via `app.merge(routes(state))`.
pub fn routes(state: WarpRouteState) -> Router {
    Router::new()
        .route("/graphql/v2", post(post_handler))
        .route("/graphql-ws", get(ws_handler))
        .with_state(state)
}

async fn post_handler(
    State(state): State<WarpRouteState>,
    Query(_query_params): Query<HashMap<String, String>>,
    headers: HeaderMap,
    req: GraphQLRequest,
) -> Result<GraphQLResponse, Response> {
    let mut req = req.into_inner();

    let claims = match extract_bearer(&headers) {
        Some(token) => verify_to_claims(&state.jwks, token, "lumen", &state.expected_issuer)
            .await
            .map_err(|_| auth_failure_response())?,
        None => {
            // Anonymous-mode entry-point used by `createAnonymousUser`.
            // The resolver gates downstream behavior on
            // `claims.has_scope("agent:anonymous")`; any other operation
            // hitting an anonymous context is rejected by the resolver's
            // `require_scope` check (`WarpRequestCtx::require_scope`).
            WarpClaims::anonymous(format!("eph-{}", uuid::Uuid::new_v4()))
        }
    };

    let trace_id = headers
        .get("traceparent")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("trace-anon")
        .to_string();

    let ctx = WarpRequestCtx::new(claims, state.dataloaders.clone(), trace_id);
    req = req.data(ctx);

    Ok(state.schema.inner().execute(req).await.into())
}

async fn ws_handler(
    State(state): State<WarpRouteState>,
    headers: HeaderMap,
    // `GraphQLProtocol` is an axum extractor that parses the
    // `Sec-WebSocket-Protocol` header and rejects with HTTP 400 if the
    // client didn't advertise `graphql-transport-ws` or
    // `graphql-ws`. We accept either; the Warp client uses
    // `graphql-transport-ws`.
    protocol: GraphQLProtocol,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    let schema = state.schema.inner().clone();
    let dataloaders = state.dataloaders.clone();
    let jwks = state.jwks.clone();
    let expected_issuer = state.expected_issuer.clone();

    ws.protocols(["graphql-transport-ws", "graphql-ws"])
        .on_upgrade(move |socket| async move {
            // Subscriptions require auth (no anonymous subscribes).
            // On failure we drop the upgraded socket which the client
            // sees as `1011 InternalError`. Distinct from the existing
            // lifegw `/v1/agent/stream` WS close-code 1003 for invalid
            // Tier-1 caps; warp_graphql sits behind Tier-User auth so
            // the failure mode is different.
            let claims = match extract_bearer(&headers) {
                Some(token) => match verify_to_claims(&jwks, token, "lumen", &expected_issuer)
                    .await
                {
                    Ok(c) => c,
                    Err(_) => return,
                },
                None => return,
            };

            let ctx = WarpRequestCtx::new(claims, dataloaders, "trace-ws".into());

            let mut data = async_graphql::Data::default();
            data.insert(ctx);

            GraphQLWebSocket::new(socket, schema, protocol)
                .with_data(data)
                .serve()
                .await;
        })
}

/// 401 response used when a Bearer token is supplied but fails JWKS
/// verification. Returned as raw HTTP rather than as a GraphQL error
/// so the Warp client routes it through its UI auth-recovery path
/// (which keys on HTTP status, not GraphQL `extensions.code`).
fn auth_failure_response() -> Response {
    Response::builder()
        .status(401)
        .body(axum::body::Body::empty())
        .unwrap_or_else(|_| {
            // Body construction is infallible for empty Body; this
            // branch exists only to satisfy the `Result` type without
            // an `unwrap`. If it ever fires it's a panic-grade env
            // failure and the default 200-OK response is harmless
            // because the request body is empty.
            Response::default()
        })
}

#[cfg(test)]
mod tests {
    // No unit tests at the route level — handler types are typechecked
    // by the compiler, and end-to-end exercise lands in Task 12
    // (conformance harness against the schema introspection) + Task 22
    // (E2E smoke against a patched warp-oss binary).
}
