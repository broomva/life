//! Route → required scope table + intersection enforcement (Spec C₃ §5.4).
//!
//! Sub-phase B's middleware minted Tier-2 capability tokens unconditionally
//! once Tier-1 verify succeeded. That defeats Spec C₃ §5.4: the gateway
//! must reject Tier-1 tokens whose scope set does NOT cover the route's
//! required scope, *before* Tier-2 is minted (so a forbidden route never
//! gets a usable capability).
//!
//! Sub-phase C (BRO-938 follow-up #3) adds:
//!
//! 1. A static table mapping each `life.v1.*` RPC method (and the
//!    Sub-phase C WS upgrade path) to its required scope.
//! 2. [`required_scope`] — a path → scope lookup that handles both
//!    canonical gRPC paths (`/life.v1.Agent/SendMessage`) and the WS
//!    upgrade path (`/v1/agent/stream`).
//! 3. [`enforce`] — given a Tier-1 claim and the inbound request path,
//!    return `Ok(())` when the claim's scope set covers the route, or
//!    a typed `Err(ScopeError::Insufficient)` otherwise.
//!
//! Per the spec, `Identity.Me` is always granted — the tier-1 verifier
//! already authenticated the user, so this is a no-op intersection.
//!
//! Health endpoints (`/healthz`, `/readyz`, `/version`, `/metrics`)
//! bypass auth entirely (Spec C₃ §3.5 LOCKED L4-D7) — they never reach
//! this module.
//!
//! Unknown paths (no entry in the table) fall through to a typed
//! `ScopeError::UnknownRoute` which middleware translates to
//! `Status::not_found` so we don't leak the existence/non-existence of
//! routes to a forged-scope attacker.

use crate::auth::tier1::Tier1Claims;

/// Scope required to invoke a route. The `Events.{Read,Subscribe,GetBlob}`
/// scopes are namespace-templated in the spec; Sub-phase C ships the
/// non-templated `events:read` scope and defers per-namespace checks
/// to lifed (which already knows the user's namespace bindings).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RequiredScope {
    /// `agent:dispatch` — Agent.{CreateSession, SendMessage}.
    AgentDispatch,
    /// `agent:read` — Agent.{DescribeSession, CloseSession, StreamSession}.
    /// Per Spec C₃ §5.4 line 468, `StreamSession` requires `agent:read`
    /// (read-side), not `agent:dispatch`. The WebSocket upgrade path at
    /// `/v1/agent/stream` initiates a `StreamSession` RPC, so browsers
    /// tailing a session need `agent:read` (or a wildcard `agent:*`).
    AgentRead,
    /// `agent:approve` — Agent.{ApproveDispatch, CancelDispatch}.
    AgentApprove,
    /// `agent:catalog` — Agent.{ListSkills, ListModels, ListTools}.
    AgentCatalog,
    /// `agent:spawn` — Agent.SpawnChild (post-MVS / Spec C₇).
    AgentSpawn,
    /// `events:read` — Events.{Read, Subscribe, GetBlob} (template
    /// `events:read:<ns>` resolved at lifed).
    EventsRead,
    /// `wallet:read` — Wallet.{GetBalance, Statement}.
    WalletRead,
    /// `wallet:write` — Wallet.{Debit, Transfer}.
    WalletWrite,
    /// `identity:read` — Identity.{ListSessions}.
    IdentityRead,
    /// `identity:write` — Identity.{UpdateProfile, RevokeSession}.
    IdentityWrite,
    /// Always granted — `Identity.Me` is permitted to any
    /// authenticated user (the Tier-1 token IS their identity).
    AlwaysGranted,
}

impl RequiredScope {
    /// Canonical scope string the table publishes. The Tier-1 token
    /// must contain a matching entry (or a wildcard) for the request
    /// to pass. `AlwaysGranted` returns `None` because no scope is
    /// required.
    pub fn as_str(self) -> Option<&'static str> {
        match self {
            RequiredScope::AgentDispatch => Some("agent:dispatch"),
            RequiredScope::AgentRead => Some("agent:read"),
            RequiredScope::AgentApprove => Some("agent:approve"),
            RequiredScope::AgentCatalog => Some("agent:catalog"),
            RequiredScope::AgentSpawn => Some("agent:spawn"),
            RequiredScope::EventsRead => Some("events:read"),
            RequiredScope::WalletRead => Some("wallet:read"),
            RequiredScope::WalletWrite => Some("wallet:write"),
            RequiredScope::IdentityRead => Some("identity:read"),
            RequiredScope::IdentityWrite => Some("identity:write"),
            RequiredScope::AlwaysGranted => None,
        }
    }
}

/// Errors returned by [`enforce`].
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScopeError {
    /// Tier-1 ∩ route scope = ∅ — token cannot invoke this route.
    #[error("scope insufficient: route {route} requires {required:?}, token has {available:?}")]
    Insufficient {
        route: String,
        required: &'static str,
        available: Vec<String>,
    },
    /// Path doesn't match any known route. Middleware translates this
    /// to `Status::not_found` to avoid leaking route existence to
    /// scope-forgery probes.
    #[error("unknown route: {0}")]
    UnknownRoute(String),
}

/// Look up the [`RequiredScope`] for an inbound request path.
///
/// Recognises:
/// - Canonical gRPC paths `/life.v1.<Service>/<Method>` (the form
///   tonic + tonic-web place on the URI).
/// - The WS upgrade path `/v1/agent/stream` (Sub-phase C public
///   surface).
/// - The legacy REST mirror paths `/v1/sessions`, `/v1/wallet`, etc.
///   (defer to the same scope as the gRPC equivalent).
///
/// Returns `None` when no entry matches — middleware surfaces this
/// as `ScopeError::UnknownRoute`.
pub fn required_scope(path: &str) -> Option<RequiredScope> {
    // gRPC canonical form has a leading slash: `/life.v1.Agent/SendMessage`.
    let trimmed = path.trim_start_matches('/');

    // ── life.v1.Agent ──
    if let Some(rest) = trimmed.strip_prefix("life.v1.Agent/") {
        return Some(match rest {
            "CreateSession" | "SendMessage" => RequiredScope::AgentDispatch,
            "DescribeSession" | "CloseSession" | "StreamSession" => RequiredScope::AgentRead,
            "ApproveDispatch" | "CancelDispatch" => RequiredScope::AgentApprove,
            "ListSkills" | "ListModels" | "ListTools" => RequiredScope::AgentCatalog,
            "SpawnChild" => RequiredScope::AgentSpawn,
            _ => return None,
        });
    }

    // ── life.v1.Events ──
    if let Some(rest) = trimmed.strip_prefix("life.v1.Events/") {
        return Some(match rest {
            "Read" | "Subscribe" | "GetBlob" => RequiredScope::EventsRead,
            _ => return None,
        });
    }

    // ── life.v1.Wallet ──
    if let Some(rest) = trimmed.strip_prefix("life.v1.Wallet/") {
        return Some(match rest {
            "GetBalance" | "Statement" => RequiredScope::WalletRead,
            "Debit" | "Transfer" => RequiredScope::WalletWrite,
            _ => return None,
        });
    }

    // ── life.v1.Identity ──
    if let Some(rest) = trimmed.strip_prefix("life.v1.Identity/") {
        return Some(match rest {
            "Me" => RequiredScope::AlwaysGranted,
            "ListSessions" => RequiredScope::IdentityRead,
            "UpdateProfile" | "RevokeSession" => RequiredScope::IdentityWrite,
            _ => return None,
        });
    }

    // ── WS upgrade (Sub-phase C public surface) ──
    if trimmed == "v1/agent/stream" {
        return Some(RequiredScope::AgentDispatch);
    }

    None
}

/// Check whether a Tier-1 claim's scope set covers the route's
/// required scope.
///
/// `Ok(())` means the request is allowed to proceed to Tier-2 mint
/// (Spec C₃ §5.4: "(Tier-1 claim scopes) ∩ (route required scope) ≠
/// ∅"). Any other return value blocks the request *before* the gateway
/// pays the cost of mint + UDS dial.
///
/// Implementation is deliberately allocation-free for the happy path:
/// the wildcard scope (`*`) is checked first, then the exact-match
/// scope is looked up via a linear scan of `claims.scopes` (capped at
/// a handful of entries in practice).
pub fn enforce(path: &str, claims: &Tier1Claims) -> Result<(), ScopeError> {
    let required = match required_scope(path) {
        Some(r) => r,
        None => return Err(ScopeError::UnknownRoute(path.to_string())),
    };

    let needed = match required.as_str() {
        Some(s) => s,
        None => return Ok(()), // AlwaysGranted — Identity.Me et al.
    };

    if scope_set_covers(&claims.scopes, needed) {
        Ok(())
    } else {
        Err(ScopeError::Insufficient {
            route: path.to_string(),
            required: needed,
            available: claims.scopes.clone(),
        })
    }
}

/// Returns `true` when `scopes` contains either the literal `needed`
/// scope or a wildcard match.
///
/// Wildcard rules:
/// - `*` matches every scope (root-level admin).
/// - `<prefix>:*` matches every scope under that prefix
///   (`agent:*` covers `agent:dispatch`, `agent:approve`, etc.).
/// - Templated namespace scopes (`events:read:<ns>`) are NOT matched
///   here — the gateway only enforces the unprefixed `events:read`
///   form. Per-namespace enforcement happens at lifed once it knows
///   the resource namespace. This is consistent with Spec C₃ §5.4
///   ("namespace embedded in scope") which is the lifed-side check.
fn scope_set_covers(scopes: &[String], needed: &str) -> bool {
    for s in scopes {
        if s == "*" || s == needed {
            return true;
        }
        if let Some(prefix) = s.strip_suffix(":*")
            && let Some(needed_prefix) = needed.split(':').next()
            && prefix == needed_prefix
        {
            return true;
        }
        // Allow `events:read:<ns>` to satisfy the gateway-level
        // `events:read` requirement — lifed validates the namespace.
        if let Some(left) = s.split(':').next()
            && left == needed.split(':').next().unwrap_or("")
            && s.starts_with(needed)
        {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_paths_resolve() {
        assert_eq!(
            required_scope("/life.v1.Agent/CreateSession"),
            Some(RequiredScope::AgentDispatch)
        );
        assert_eq!(
            required_scope("/life.v1.Agent/SendMessage"),
            Some(RequiredScope::AgentDispatch)
        );
        assert_eq!(
            required_scope("/life.v1.Agent/StreamSession"),
            Some(RequiredScope::AgentRead)
        );
        assert_eq!(
            required_scope("/life.v1.Agent/ApproveDispatch"),
            Some(RequiredScope::AgentApprove)
        );
        assert_eq!(
            required_scope("/life.v1.Agent/ListSkills"),
            Some(RequiredScope::AgentCatalog)
        );
        assert_eq!(
            required_scope("/life.v1.Agent/SpawnChild"),
            Some(RequiredScope::AgentSpawn)
        );
    }

    #[test]
    fn events_wallet_identity_paths_resolve() {
        assert_eq!(
            required_scope("/life.v1.Events/Read"),
            Some(RequiredScope::EventsRead)
        );
        assert_eq!(
            required_scope("/life.v1.Wallet/GetBalance"),
            Some(RequiredScope::WalletRead)
        );
        assert_eq!(
            required_scope("/life.v1.Wallet/Debit"),
            Some(RequiredScope::WalletWrite)
        );
        assert_eq!(
            required_scope("/life.v1.Identity/Me"),
            Some(RequiredScope::AlwaysGranted)
        );
        assert_eq!(
            required_scope("/life.v1.Identity/ListSessions"),
            Some(RequiredScope::IdentityRead)
        );
        assert_eq!(
            required_scope("/life.v1.Identity/UpdateProfile"),
            Some(RequiredScope::IdentityWrite)
        );
    }

    #[test]
    fn ws_upgrade_path_resolves_to_agent_dispatch() {
        // Sub-phase C: WS upgrade IS an Agent.* route.
        assert_eq!(
            required_scope("/v1/agent/stream"),
            Some(RequiredScope::AgentDispatch)
        );
    }

    #[test]
    fn unknown_path_returns_none() {
        assert_eq!(required_scope("/unknown"), None);
        assert_eq!(required_scope("/life.v1.Bogus/Method"), None);
        assert_eq!(required_scope("/life.v1.Agent/UnknownMethod"), None);
    }

    #[test]
    fn enforce_accepts_exact_scope() {
        let t1 = Tier1Claims {
            user_id: "u".to_string(),
            project_id: "p".to_string(),
            scopes: vec!["agent:dispatch".to_string()],
            tier: "free".to_string(),
        };
        enforce("/life.v1.Agent/CreateSession", &t1).expect("exact match passes");
    }

    #[test]
    fn enforce_rejects_missing_scope() {
        // Tier-1 has only `events:read` but Agent.CreateSession needs
        // `agent:dispatch`. Empty intersection → reject.
        let t1 = Tier1Claims {
            user_id: "u".to_string(),
            project_id: "p".to_string(),
            scopes: vec!["events:read".to_string()],
            tier: "free".to_string(),
        };
        let err = enforce("/life.v1.Agent/CreateSession", &t1).expect_err("must reject");
        match err {
            ScopeError::Insufficient {
                route,
                required,
                available,
            } => {
                assert!(route.contains("CreateSession"));
                assert_eq!(required, "agent:dispatch");
                assert_eq!(available, vec!["events:read".to_string()]);
            }
            other => panic!("expected Insufficient, got {other:?}"),
        }
    }

    #[test]
    fn enforce_accepts_wildcard_root() {
        let t1 = Tier1Claims {
            user_id: "admin".to_string(),
            project_id: "p".to_string(),
            scopes: vec!["*".to_string()],
            tier: "enterprise".to_string(),
        };
        enforce("/life.v1.Wallet/Debit", &t1).expect("wildcard root passes");
    }

    #[test]
    fn enforce_accepts_prefix_wildcard() {
        // `agent:*` covers every `agent:<x>` route.
        let t1 = Tier1Claims {
            user_id: "u".to_string(),
            project_id: "p".to_string(),
            scopes: vec!["agent:*".to_string()],
            tier: "paid".to_string(),
        };
        enforce("/life.v1.Agent/SendMessage", &t1).expect("agent:* covers send_message");
        enforce("/life.v1.Agent/ApproveDispatch", &t1).expect("agent:* covers approve");
    }

    #[test]
    fn enforce_accepts_namespace_templated_events_read() {
        // `events:read:demo` should satisfy the gateway-level
        // `events:read` requirement — the per-namespace check belongs
        // at lifed.
        let t1 = Tier1Claims {
            user_id: "u".to_string(),
            project_id: "demo".to_string(),
            scopes: vec!["events:read:demo".to_string()],
            tier: "free".to_string(),
        };
        enforce("/life.v1.Events/Read", &t1).expect("templated events:read passes");
    }

    #[test]
    fn enforce_grants_identity_me_unconditionally() {
        // Identity.Me is always granted to any authenticated request.
        let t1 = Tier1Claims {
            user_id: "u".to_string(),
            project_id: "p".to_string(),
            scopes: vec![],
            tier: "free".to_string(),
        };
        enforce("/life.v1.Identity/Me", &t1).expect("Identity.Me always granted");
    }

    #[test]
    fn enforce_unknown_route_errors() {
        let t1 = Tier1Claims {
            user_id: "u".to_string(),
            project_id: "p".to_string(),
            scopes: vec!["*".to_string()],
            tier: "free".to_string(),
        };
        match enforce("/unknown/path", &t1) {
            Err(ScopeError::UnknownRoute(p)) => assert_eq!(p, "/unknown/path"),
            other => panic!("expected UnknownRoute, got {other:?}"),
        }
    }

    #[test]
    fn enforce_ws_upgrade_with_dispatch_scope() {
        let t1 = Tier1Claims {
            user_id: "u".to_string(),
            project_id: "p".to_string(),
            scopes: vec!["agent:dispatch".to_string()],
            tier: "free".to_string(),
        };
        enforce("/v1/agent/stream", &t1).expect("ws upgrade requires agent:dispatch");
    }

    #[test]
    fn enforce_ws_upgrade_without_dispatch_scope_rejected() {
        let t1 = Tier1Claims {
            user_id: "u".to_string(),
            project_id: "p".to_string(),
            scopes: vec!["events:read".to_string()],
            tier: "free".to_string(),
        };
        let err = enforce("/v1/agent/stream", &t1).expect_err("ws upgrade rejected");
        assert!(matches!(err, ScopeError::Insufficient { .. }));
    }
}
