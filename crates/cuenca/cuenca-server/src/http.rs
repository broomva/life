//! The agent-facing HTTP API (spec section 0: the agent verbs, never an owner verb).
//!
//! A middleware authenticates every request ([`Authority::authenticate`]) before any
//! handler runs, so no request body is read for a caller who is not authenticated.
//! Reads are scoped ([`Caller::sees_fork`], [`Caller::sees_location`]): what a caller
//! may not read answers 404, the same as what does not exist. Fork and location names
//! are global, so creating a fork under a name taken by a fork the caller cannot read
//! answers 409; that says the name is taken, nothing about the fork. Writes go to
//! cuenca-core as the caller's [`Writer`] (its principal, on the node Tailscale named,
//! under the epoch it states), and the core's rules decide whether that writer holds
//! the fork. A draft may only name blobs the caller could read or has uploaded
//! ([`Authority::check_puts`]).
//!
//! Minting and revoking keys, registering a location and taking a fork over have no
//! route here: they are owner acts, on the admin socket only.
//!
//! | Method | Path | Does |
//! |---|---|---|
//! | GET | `/v1/whoami` | the caller's principal, kind, node and grant |
//! | GET | `/v1/locations` | locations whose main the caller reads |
//! | GET | `/v1/locations/{location}/tree` | the location's main tree |
//! | GET | `/v1/locations/{location}/blobs/{oid}` | a blob of that tree |
//! | GET | `/v1/forks` | forks the caller reads |
//! | POST | `/v1/forks` | `{fork, location}`: fork a location the caller is granted |
//! | GET | `/v1/forks/{fork}` | the fork's record |
//! | GET | `/v1/forks/{fork}/tree` | the fork's head tree |
//! | GET | `/v1/forks/{fork}/blobs/{oid}` | a blob of that tree |
//! | GET | `/v1/forks/{fork}/history` | the log records naming the fork (and only forks the caller reads) |
//! | POST | `/v1/forks/{fork}/drafts` | `{epoch, changeset}`: publish a draft |
//! | POST | `/v1/forks/{fork}/landing` | `{epoch, changeset}` (an id): ask to land |
//! | PUT | `/v1/blobs/{oid}` | upload a blob; `Content-Length` is its size |

use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{ConnectInfo, DefaultBodyLimit, Extension, Path, Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use cuenca_core::{
    ChangesetId, CoreError, Event, Fork, ForkName, LocationName, MAX_BLOB_BYTES, Writer,
};
use cuenca_tree::{Changeset, GitOid};
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::AsyncWriteExt;

use crate::authority::{AuthError, Authority, Caller};

/// Largest JSON request body (a changeset of tens of thousands of ops fits).
const MAX_JSON_BYTES: usize = 64 * 1024 * 1024;

type Shared = State<Arc<Authority>>;
type Me = Extension<Caller>;

pub fn router(authority: Arc<Authority>) -> Router {
    Router::new()
        .route("/v1/whoami", get(whoami))
        .route("/v1/locations", get(locations))
        .route("/v1/locations/{location}/tree", get(location_tree))
        .route("/v1/locations/{location}/blobs/{oid}", get(location_blob))
        .route("/v1/forks", get(forks).post(create_fork))
        .route("/v1/forks/{fork}", get(fork))
        .route("/v1/forks/{fork}/tree", get(fork_tree))
        .route("/v1/forks/{fork}/blobs/{oid}", get(fork_blob))
        .route("/v1/forks/{fork}/history", get(history))
        .route("/v1/forks/{fork}/drafts", post(publish))
        .route("/v1/forks/{fork}/landing", post(landing))
        .route(
            "/v1/blobs/{oid}",
            put(upload_blob).layer(DefaultBodyLimit::max(MAX_BLOB_BYTES as usize)),
        )
        .layer(DefaultBodyLimit::max(MAX_JSON_BYTES))
        .route_layer(middleware::from_fn_with_state(
            authority.clone(),
            authenticate,
        ))
        .with_state(authority)
}

/// Authenticate the request and hand its [`Caller`] to the handler; anything else is
/// answered 401 before the handler, and its body, are reached.
async fn authenticate(
    State(a): Shared,
    ConnectInfo(source): ConnectInfo<SocketAddr>,
    mut request: Request,
    next: Next,
) -> Response {
    match a.authenticate(bearer(request.headers()), source).await {
        Ok(caller) => {
            request.extensions_mut().insert(caller);
            next.run(request).await
        }
        Err(e) => ApiError::from(e).into_response(),
    }
}

/// An error answer: a status and `{"error": code, "message": ..}`, plus fields some
/// codes carry (`head` on `not_at_base`).
#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    body: Value,
}

impl ApiError {
    fn new(status: StatusCode, code: &str, message: impl Into<String>) -> Self {
        Self {
            status,
            body: json!({ "error": code, "message": message.into() }),
        }
    }

    fn not_found() -> Self {
        Self::new(
            StatusCode::NOT_FOUND,
            "not_found",
            "no such thing, or not yours to read",
        )
    }

    fn internal(what: impl std::fmt::Display) -> Self {
        tracing::error!(error = %what, "internal error");
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "internal error",
        )
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(self.body)).into_response()
    }
}

impl From<AuthError> for ApiError {
    fn from(e: AuthError) -> Self {
        tracing::warn!(reason = %e, "request refused");
        match e {
            AuthError::Internal(e) => Self::internal(e),
            _ => Self::new(StatusCode::UNAUTHORIZED, "unauthorized", "unauthorized"),
        }
    }
}

impl From<CoreError> for ApiError {
    fn from(e: CoreError) -> Self {
        use StatusCode as S;
        let message = e.to_string();
        let (status, code) = match &e {
            CoreError::NotAtBase { head, .. } => {
                return Self {
                    status: S::CONFLICT,
                    body: json!({
                        "error": "not_at_base",
                        "message": message,
                        "head": head.to_hex(),
                    }),
                };
            }
            CoreError::Name(_)
            | CoreError::Path(_)
            | CoreError::Tree(_)
            | CoreError::Changeset(_)
            | CoreError::BadRemote(_)
            | CoreError::WrongLocation { .. } => (S::BAD_REQUEST, "invalid"),
            CoreError::ReservedName(_) => (S::BAD_REQUEST, "reserved_name"),
            CoreError::Hash(_) | CoreError::OidMismatch { .. } => {
                (S::UNPROCESSABLE_ENTITY, "bad_blob")
            }
            CoreError::TooLarge { .. } => (S::PAYLOAD_TOO_LARGE, "too_large"),
            CoreError::MissingBlob(_) => (S::UNPROCESSABLE_ENTITY, "missing_blob"),
            CoreError::UnknownLocation(_)
            | CoreError::UnknownFork(_)
            | CoreError::UnknownChangeset(_) => return Self::not_found(),
            CoreError::LocationExists(_) | CoreError::ForkExists(_) => (S::CONFLICT, "exists"),
            CoreError::NotHead { .. } => (S::CONFLICT, "not_head"),
            CoreError::StaleEpoch { .. } => (S::CONFLICT, "stale_epoch"),
            CoreError::EpochMismatch { .. } => (S::CONFLICT, "epoch_mismatch"),
            CoreError::NotHolder { .. } => (S::FORBIDDEN, "not_holder"),
            CoreError::Revoked(_) => (S::UNAUTHORIZED, "unauthorized"),
            _ => return Self::internal(e),
        };
        Self::new(status, code, message)
    }
}

type ApiResult<T = Json<Value>> = Result<T, ApiError>;

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
}

/// Run `f` on the blocking pool: the store's calls are synchronous.
async fn blocking<T: Send + 'static>(
    a: &Arc<Authority>,
    f: impl FnOnce(&Authority) -> ApiResult<T> + Send + 'static,
) -> ApiResult<T> {
    let a = a.clone();
    tokio::task::spawn_blocking(move || f(&a))
        .await
        .map_err(ApiError::internal)?
}

fn parse<'s, T, E: std::fmt::Display>(
    s: &'s str,
    f: impl FnOnce(&'s str) -> Result<T, E>,
) -> ApiResult<T> {
    f(s).map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, "invalid", e.to_string()))
}

fn to_json(v: impl serde::Serialize) -> ApiResult {
    serde_json::to_value(v)
        .map(Json)
        .map_err(ApiError::internal)
}

/// The fork named `name`, if the caller reads it; otherwise 404.
fn visible_fork(a: &Authority, caller: &Caller, name: &ForkName) -> ApiResult<Fork> {
    match a.store.fork(name)? {
        Some(f) if caller.sees_fork(&f) => Ok(f),
        _ => Err(ApiError::not_found()),
    }
}

/// The fork named `name`, if a write by the caller reaches the core's rules
/// ([`Caller::reaches_fork`]); otherwise 404.
fn writable_fork(a: &Authority, caller: &Caller, name: &ForkName) -> ApiResult<Fork> {
    match a.store.fork(name)? {
        Some(f) if caller.reaches_fork(&f) => Ok(f),
        _ => Err(ApiError::not_found()),
    }
}

fn visible_location(
    a: &Authority,
    caller: &Caller,
    name: &LocationName,
) -> ApiResult<cuenca_core::Location> {
    match a.store.location(name)? {
        Some(l) if caller.sees_location(&l) => Ok(l),
        _ => Err(ApiError::not_found()),
    }
}

/// The body as JSON, read only now that the caller is authenticated.
fn json_body<T: serde::de::DeserializeOwned>(body: &[u8]) -> ApiResult<T> {
    serde_json::from_slice(body)
        .map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, "invalid", e.to_string()))
}

async fn whoami(Extension(c): Me) -> ApiResult {
    Ok(Json(json!({
        "principal": c.key.principal,
        "kind": c.key.kind,
        "node": c.key.node,
        "grant": c.key.grant,
    })))
}

async fn locations(State(a): Shared, Extension(c): Me) -> ApiResult {
    blocking(&a, move |a| {
        let all = a.store.locations()?;
        to_json(
            all.into_iter()
                .filter(|l| c.sees_location(l))
                .collect::<Vec<_>>(),
        )
    })
    .await
}

async fn location_tree(
    State(a): Shared,
    Extension(c): Me,
    Path(location): Path<String>,
) -> ApiResult {
    let name = parse(&location, LocationName::new)?;
    blocking(&a, move |a| {
        let l = visible_location(a, &c, &name)?;
        to_json(json!({ "oid": l.main.to_hex(), "tree": a.store.tree(l.main)? }))
    })
    .await
}

async fn location_blob(
    State(a): Shared,
    Extension(c): Me,
    Path((location, oid)): Path<(String, String)>,
) -> ApiResult<Response> {
    let name = parse(&location, LocationName::new)?;
    let oid = parse(&oid, GitOid::from_hex)?;
    blocking(&a, move |a| {
        let tree = visible_location(a, &c, &name)?.main;
        blob_of(a, tree, oid)
    })
    .await
}

async fn forks(State(a): Shared, Extension(c): Me) -> ApiResult {
    blocking(&a, move |a| {
        let all = a.store.forks()?;
        to_json(
            all.into_iter()
                .filter(|f| c.sees_fork(f))
                .collect::<Vec<_>>(),
        )
    })
    .await
}

#[derive(Deserialize)]
struct CreateFork {
    fork: String,
    location: String,
}

async fn create_fork(State(a): Shared, Extension(c): Me, body: axum::body::Bytes) -> ApiResult {
    let req: CreateFork = json_body(&body)?;
    let fork = parse(&req.fork, ForkName::new)?;
    let location = parse(&req.location, LocationName::new)?;
    blocking(&a, move |a| {
        visible_location(a, &c, &location)?;
        let offset = a.guarded(&c.key.principal, || {
            a.store.create_fork(&fork, &location, &c.holder)
        })?;
        to_json(json!({ "offset": offset, "fork": a.store.fork(&fork)? }))
    })
    .await
}

async fn fork(State(a): Shared, Extension(c): Me, Path(fork): Path<String>) -> ApiResult {
    let name = parse(&fork, ForkName::new)?;
    blocking(&a, move |a| to_json(visible_fork(a, &c, &name)?)).await
}

async fn fork_tree(State(a): Shared, Extension(c): Me, Path(fork): Path<String>) -> ApiResult {
    let name = parse(&fork, ForkName::new)?;
    blocking(&a, move |a| {
        let f = visible_fork(a, &c, &name)?;
        to_json(json!({ "oid": f.head.to_hex(), "tree": a.store.tree(f.head)? }))
    })
    .await
}

async fn fork_blob(
    State(a): Shared,
    Extension(c): Me,
    Path((fork, oid)): Path<(String, String)>,
) -> ApiResult<Response> {
    let name = parse(&fork, ForkName::new)?;
    let oid = parse(&oid, GitOid::from_hex)?;
    blocking(&a, move |a| {
        let tree = visible_fork(a, &c, &name)?.head;
        blob_of(a, tree, oid)
    })
    .await
}

/// Blob `oid`, if `tree` names it; otherwise 404. The content is verified against its
/// id on the way out.
fn blob_of(a: &Authority, tree: GitOid, oid: GitOid) -> ApiResult<Response> {
    if !a.blob_set(tree)?.contains(&oid) {
        return Err(ApiError::not_found());
    }
    let bytes = a.store.read_blob(oid)?;
    Ok(([(header::CONTENT_TYPE, "application/octet-stream")], bytes).into_response())
}

/// The fork's records, less drafts fenced into a sibling the caller does not read: a
/// fenced draft is the sibling's content, shown only to those who read the sibling.
async fn history(State(a): Shared, Extension(c): Me, Path(fork): Path<String>) -> ApiResult {
    let name = parse(&fork, ForkName::new)?;
    blocking(&a, move |a| {
        visible_fork(a, &c, &name)?;
        let mut shown = Vec::new();
        for record in a.store.fork_history(&name)? {
            let hidden = match &record.event {
                Event::DraftFenced { sibling, .. } => {
                    !a.store.fork(sibling)?.is_some_and(|f| c.sees_fork(&f))
                }
                _ => false,
            };
            if hidden {
                continue;
            }
            shown.push(record);
        }
        to_json(shown)
    })
    .await
}

#[derive(Deserialize)]
struct Publish {
    epoch: u64,
    changeset: Changeset,
}

async fn publish(
    State(a): Shared,
    Extension(c): Me,
    Path(fork): Path<String>,
    body: axum::body::Bytes,
) -> ApiResult {
    let name = parse(&fork, ForkName::new)?;
    let req: Publish = json_body(&body)?;
    blocking(&a, move |a| {
        let state = writable_fork(a, &c, &name)?;
        a.check_puts(&c, &state, &req.changeset)?;
        let writer = Writer::new(c.holder.clone(), req.epoch);
        let p = a.guarded(&c.key.principal, || {
            a.store.publish_draft(&name, &writer, &req.changeset)
        })?;
        Ok(Json(json!({
            "id": p.id.to_hex(),
            "fork": p.fork,
            "epoch": p.epoch,
            "fenced": p.fenced,
            "offset": p.offset,
            "fresh": p.fresh,
        })))
    })
    .await
}

#[derive(Deserialize)]
struct Landing {
    epoch: u64,
    changeset: String,
}

async fn landing(
    State(a): Shared,
    Extension(c): Me,
    Path(fork): Path<String>,
    body: axum::body::Bytes,
) -> ApiResult {
    let name = parse(&fork, ForkName::new)?;
    let req: Landing = json_body(&body)?;
    let id = parse(&req.changeset, ChangesetId::from_hex)?;
    blocking(&a, move |a| {
        writable_fork(a, &c, &name)?;
        let writer = Writer::new(c.holder.clone(), req.epoch);
        let offset = a.guarded(&c.key.principal, || {
            a.store.record_landing_intent(&name, &writer, id)
        })?;
        Ok(Json(json!({ "offset": offset })))
    })
    .await
}

/// Upload a blob. The body is streamed to a staging file, which the store ingests: it
/// hashes the bytes as git does and keeps them only if they are exactly
/// `Content-Length` long and hash to `oid` (hyper delivers a length-delimited body of
/// exactly that length, refuses a longer one, and drops a Content-Length sent with
/// chunked encoding, which this handler then refuses as unsized). The store ingests a
/// blob it already holds the same way as a new one, so neither the answer nor its
/// timing says whether it had it; either way the caller has now shown it holds the
/// bytes ([`Authority::check_puts`]).
async fn upload_blob(
    State(a): Shared,
    Extension(c): Me,
    Path(oid): Path<String>,
    h: HeaderMap,
    body: Body,
) -> ApiResult {
    let oid = parse(&oid, GitOid::from_hex)?;
    let size: u64 = h
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok()?.parse().ok())
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::LENGTH_REQUIRED,
                "length_required",
                "Content-Length is the blob's size",
            )
        })?;
    if size > MAX_BLOB_BYTES {
        return Err(CoreError::TooLarge {
            size,
            limit: MAX_BLOB_BYTES,
        }
        .into());
    }
    let staged = tempfile::NamedTempFile::new_in(&a.staging).map_err(ApiError::internal)?;
    let mut file = tokio::fs::File::from_std(staged.reopen().map_err(ApiError::internal)?);
    let mut stream = body.into_data_stream();
    while let Some(chunk) = stream.next().await {
        let chunk =
            chunk.map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, "bad_body", e.to_string()))?;
        file.write_all(&chunk).await.map_err(ApiError::internal)?;
    }
    file.flush().await.map_err(ApiError::internal)?;
    drop(file);
    blocking(&a, move |a| {
        let content = staged.reopen().map_err(ApiError::internal)?;
        let info = a.guarded(&c.key.principal, || {
            let info = a.store.ingest_blob(oid, size, content)?;
            a.possesses(&c.key.principal, oid);
            Ok::<_, CoreError>(info)
        })?;
        Ok(Json(json!({
            "oid": oid.to_hex(),
            "size": info.size,
            "sha256": info.sha256.to_hex(),
        })))
    })
    .await
}
