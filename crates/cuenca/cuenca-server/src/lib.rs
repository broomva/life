//! The Cuenca authority (spec `docs/specs/2026-10-19-agent-native-change-plane.html` in
//! broomva/workspace, section 0, and the fleet ADR
//! `docs/adrs/2026-10-19-fleet-runtime-boundaries.html`).
//!
//! [`run`] serves two surfaces over one [`Authority`] (a `cuenca_core::Store`, the keys
//! file and a node resolver):
//!
//! - **the agent API** ([`http`]): HTTP/JSON on the authority's tailnet address. Every
//!   request carries a bearer key minted for one principal and bound to one Tailscale
//!   stable node ID; the authority checks the key, that the log has not revoked the
//!   principal, that `tailscale whois` names the request's source as the key's node, and
//!   that the node serves no subnet route. Reads are scoped to what the key may read.
//! - **owner acts** ([`admin`]): a local Unix socket, mode 0600, answering only its own
//!   OS user. Minting and revoking keys, registering locations and taking forks over
//!   live only there.
//!
//! The authority binds only a tailnet address (100.64.0.0/10 or fd7a:115c:a1e0::/48)
//! unless told `unsafe_bind`.
//!
//! Crash safety is the store's: each write is one durable redb transaction, and every
//! write the API offers is idempotent by its content (a draft by its changeset id, an
//! intent by its changeset), so a client that lost an answer to a crash sends the same
//! request again.

pub mod admin;
mod authority;
pub mod http;
mod identity;
mod keys;

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;

pub use authority::{AuthError, Authority, Caller, DEFAULT_MIN_FREE};
pub use identity::{Peer, Tailscale, Whois, WhoisFuture};
pub use keys::{Grant, KeyError, KeyRecord, KeyRing, PrincipalKind};

/// How [`run`] serves.
#[derive(Debug, Clone)]
pub struct Config {
    /// The authority's state: `store/`, `keys.json`, `staging/`.
    pub state: PathBuf,
    /// Where the agent API listens: the host's tailnet address.
    pub bind: SocketAddr,
    /// Where the admin socket is created.
    pub admin_socket: PathBuf,
    /// Allow a bind address outside the tailnet ranges.
    pub unsafe_bind: bool,
    /// The `tailscale` binary.
    pub tailscale: PathBuf,
    /// Free bytes blob uploads must leave on the state's filesystem.
    pub min_free: u64,
}

/// Whether `ip` is in Tailscale's address ranges: 100.64.0.0/10 and
/// fd7a:115c:a1e0::/48.
pub fn is_tailnet(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            o[0] == 100 && (o[1] & 0xc0) == 64
        }
        IpAddr::V6(v6) => {
            let s = v6.segments();
            s[0] == 0xfd7a && s[1] == 0x115c && s[2] == 0xa1e0
        }
    }
}

/// Refuse a bind address outside the tailnet unless `unsafe_bind`.
pub fn check_bind(addr: SocketAddr, unsafe_bind: bool) -> Result<(), String> {
    if unsafe_bind || is_tailnet(addr.ip()) {
        Ok(())
    } else {
        Err(format!(
            "{addr} is not a tailnet address (100.64.0.0/10, fd7a:115c:a1e0::/48); \
             pass --unsafe-bind to bind it anyway"
        ))
    }
}

/// Serve the agent API on `listener` until the future is dropped.
pub async fn serve_http(
    listener: tokio::net::TcpListener,
    authority: Arc<Authority>,
) -> std::io::Result<()> {
    axum::serve(
        listener,
        http::router(authority).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
}

/// Run the authority: check the bind address, open the state, then serve the agent
/// API and the admin socket until either stops or the process gets Ctrl-C.
pub async fn run(config: Config) -> anyhow::Result<()> {
    check_bind(config.bind, config.unsafe_bind).map_err(anyhow::Error::msg)?;
    let whois: Arc<dyn Whois> = Arc::new(Tailscale::new(&config.tailscale));
    let state = config.state.clone();
    let authority =
        Arc::new(tokio::task::spawn_blocking(move || Authority::open(&state, whois)).await??);
    let listener = tokio::net::TcpListener::bind(config.bind).await?;
    let admin = admin::bind(&config.admin_socket)?;
    tracing::info!(bind = %config.bind, admin = %config.admin_socket.display(), "cuenca authority up");
    tokio::select! {
        r = serve_http(listener, authority.clone()) => r?,
        () = admin::serve(admin, authority) => {}
        r = tokio::signal::ctrl_c() => r?,
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_tailnet_addresses_bind_by_default() {
        for ok in [
            "100.64.0.1:7",
            "100.82.195.109:7",
            "100.127.255.254:7",
            "[fd7a:115c:a1e0::a937:c36e]:7",
        ] {
            assert!(check_bind(ok.parse().unwrap(), false).is_ok(), "{ok}");
        }
        for bad in [
            "0.0.0.0:7",
            "127.0.0.1:7",
            "100.63.255.255:7",
            "100.128.0.1:7",
            "10.0.0.1:7",
            "[::]:7",
            "[::1]:7",
            "[fd7a:115c:a1e1::1]:7",
        ] {
            assert!(check_bind(bad.parse().unwrap(), false).is_err(), "{bad}");
            assert!(check_bind(bad.parse().unwrap(), true).is_ok(), "{bad}");
        }
    }
}
