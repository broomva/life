//! The agent-facing HTTP API (spec section 0: the agent verbs, never an owner verb).
//!
//! Every route authenticates the request first ([`Authority::authenticate`]). Reads are
//! scoped ([`Caller::sees_fork`], [`Caller::sees_location`]); what a caller may not
//! read answers 404, the same as what does not exist. Writes go to cuenca-core as
//! the caller's [`Writer`] (its principal, on the node Tailscale named, under the epoch
//! it states), and the core's rules decide whether that writer holds the fork.
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
//! | GET | `/v1/forks/{fork}/history` | the log records naming the fork |
//! | POST | `/v1/forks/{fork}/drafts` | `{epoch, changeset}`: publish a draft |
//! | POST | `/v1/forks/{fork}/landing` | `{epoch, changeset}` (an id): ask to land |
//! | PUT | `/v1/blobs/{oid}` | upload a blob; `Content-Length` is its size |

use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{ConnectInfo, DefaultBodyLimit, Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use cuenca_core::{ChangesetId, CoreError, ForkName, LocationName, MAX_BLOB_BYTES, Writer};
use cuenca_tree::{BlobHasher, Changeset, GitOid};
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::AsyncWriteExt;

use crate::authority::{AuthError, Authority, Caller};

/// Largest JSON request body (a changeset of tens of thousands of ops fits).
const MAX_JSON_BYTES: usize = 64 * 1024 * 1024;

type Shared = State<Arc<Authority>>;
type Source = ConnectInfo<SocketAddr>;

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
        .with_state(authority)
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

async fn auth(a: &Authority, headers: &HeaderMap, source: SocketAddr) -> ApiResult<Caller> {
    Ok(a.authenticate(bearer(headers), source).await?)
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
fn visible_fork(a: &Authority, caller: &Caller, name: &ForkName) -> ApiResult<cuenca_core::Fork> {
    match a.store.fork(name)? {
        Some(f) if caller.sees_fork(&f) => Ok(f),
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

async fn whoami(State(a): Shared, ConnectInfo(src): Source, h: HeaderMap) -> ApiResult {
    let c = auth(&a, &h, src).await?;
    Ok(Json(json!({
        "principal": c.key.principal,
        "kind": c.key.kind,
        "node": c.key.node,
        "grant": c.key.grant,
    })))
}

async fn locations(State(a): Shared, ConnectInfo(src): Source, h: HeaderMap) -> ApiResult {
    let c = auth(&a, &h, src).await?;
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
    ConnectInfo(src): Source,
    h: HeaderMap,
    Path(location): Path<String>,
) -> ApiResult {
    let c = auth(&a, &h, src).await?;
    let name = parse(&location, LocationName::new)?;
    blocking(&a, move |a| {
        let l = visible_location(a, &c, &name)?;
        to_json(json!({ "oid": l.main.to_hex(), "tree": a.store.tree(l.main)? }))
    })
    .await
}

async fn location_blob(
    State(a): Shared,
    ConnectInfo(src): Source,
    h: HeaderMap,
    Path((location, oid)): Path<(String, String)>,
) -> ApiResult<Response> {
    let c = auth(&a, &h, src).await?;
    let name = parse(&location, LocationName::new)?;
    let oid = parse(&oid, GitOid::from_hex)?;
    blocking(&a, move |a| {
        let tree = visible_location(a, &c, &name)?.main;
        blob_of(a, tree, oid)
    })
    .await
}

async fn forks(State(a): Shared, ConnectInfo(src): Source, h: HeaderMap) -> ApiResult {
    let c = auth(&a, &h, src).await?;
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

async fn create_fork(
    State(a): Shared,
    ConnectInfo(src): Source,
    h: HeaderMap,
    Json(req): Json<CreateFork>,
) -> ApiResult {
    let c = auth(&a, &h, src).await?;
    let fork = parse(&req.fork, ForkName::new)?;
    let location = parse(&req.location, LocationName::new)?;
    blocking(&a, move |a| {
        visible_location(a, &c, &location)?;
        let offset = a.store.create_fork(&fork, &location, &c.holder)?;
        to_json(json!({ "offset": offset, "fork": a.store.fork(&fork)? }))
    })
    .await
}

async fn fork(
    State(a): Shared,
    ConnectInfo(src): Source,
    h: HeaderMap,
    Path(fork): Path<String>,
) -> ApiResult {
    let c = auth(&a, &h, src).await?;
    let name = parse(&fork, ForkName::new)?;
    blocking(&a, move |a| to_json(visible_fork(a, &c, &name)?)).await
}

async fn fork_tree(
    State(a): Shared,
    ConnectInfo(src): Source,
    h: HeaderMap,
    Path(fork): Path<String>,
) -> ApiResult {
    let c = auth(&a, &h, src).await?;
    let name = parse(&fork, ForkName::new)?;
    blocking(&a, move |a| {
        let f = visible_fork(a, &c, &name)?;
        to_json(json!({ "oid": f.head.to_hex(), "tree": a.store.tree(f.head)? }))
    })
    .await
}

async fn fork_blob(
    State(a): Shared,
    ConnectInfo(src): Source,
    h: HeaderMap,
    Path((fork, oid)): Path<(String, String)>,
) -> ApiResult<Response> {
    let c = auth(&a, &h, src).await?;
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

async fn history(
    State(a): Shared,
    ConnectInfo(src): Source,
    h: HeaderMap,
    Path(fork): Path<String>,
) -> ApiResult {
    let c = auth(&a, &h, src).await?;
    let name = parse(&fork, ForkName::new)?;
    blocking(&a, move |a| {
        visible_fork(a, &c, &name)?;
        to_json(a.store.fork_history(&name)?)
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
    ConnectInfo(src): Source,
    h: HeaderMap,
    Path(fork): Path<String>,
    Json(req): Json<Publish>,
) -> ApiResult {
    let c = auth(&a, &h, src).await?;
    let name = parse(&fork, ForkName::new)?;
    blocking(&a, move |a| {
        visible_fork(a, &c, &name)?;
        let writer = Writer::new(c.holder.clone(), req.epoch);
        let p = a.store.publish_draft(&name, &writer, &req.changeset)?;
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
    ConnectInfo(src): Source,
    h: HeaderMap,
    Path(fork): Path<String>,
    Json(req): Json<Landing>,
) -> ApiResult {
    let c = auth(&a, &h, src).await?;
    let name = parse(&fork, ForkName::new)?;
    let id = parse(&req.changeset, ChangesetId::from_hex)?;
    blocking(&a, move |a| {
        visible_fork(a, &c, &name)?;
        let writer = Writer::new(c.holder.clone(), req.epoch);
        let offset = a.store.record_landing_intent(&name, &writer, id)?;
        Ok(Json(json!({ "offset": offset })))
    })
    .await
}

/// Upload a blob. The body is streamed to a staging file while it is hashed as git
/// does (hyper reads no more than `Content-Length`); only bytes that are exactly that
/// long and hash to `oid` are ingested. An upload of a blob the store already holds is verified the same way, so
/// the answer never says whether the store had it.
async fn upload_blob(
    State(a): Shared,
    ConnectInfo(src): Source,
    h: HeaderMap,
    Path(oid): Path<String>,
    body: Body,
) -> ApiResult {
    auth(&a, &h, src).await?;
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
    let mut hasher = BlobHasher::new(size);
    let mut stream = body.into_data_stream();
    while let Some(chunk) = stream.next().await {
        let chunk =
            chunk.map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, "bad_body", e.to_string()))?;
        hasher.update(&chunk);
        file.write_all(&chunk).await.map_err(ApiError::internal)?;
    }
    file.flush().await.map_err(ApiError::internal)?;
    drop(file);
    let blob = hasher.finish().map_err(CoreError::Hash)?;
    if blob.oid() != oid {
        return Err(CoreError::OidMismatch {
            claimed: oid,
            actual: blob.oid(),
        }
        .into());
    }
    blocking(&a, move |a| {
        let content = staged.reopen().map_err(ApiError::internal)?;
        let info = a.store.ingest_blob(oid, size, content)?;
        Ok(Json(json!({
            "oid": oid.to_hex(),
            "size": info.size,
            "sha256": info.sha256.to_hex(),
        })))
    })
    .await
}
