//! Span-attribute helpers per Spec C₂ §9.2.
//!
//! Each lifed RPC handler should create a span via [`rpc_span`] so the
//! trace tree is annotated with `rpc.system`, `rpc.service`,
//! `rpc.method`, and (when the auth middleware has parsed the bearer)
//! `life.user_id`, `life.project_id`, `life.session_id`.

use crate::auth::capability::CapabilityClaims;

/// Build a span with the canonical lifed RPC attributes.
///
/// When `claims` is `Some`, the user/project/session ids are recorded
/// as span fields too — tracing's `info_span!` only allocates the
/// metadata once per span construction so this is cheap.
pub fn rpc_span(service: &str, method: &str, claims: Option<&CapabilityClaims>) -> tracing::Span {
    let span = tracing::info_span!(
        "lifed.rpc",
        rpc.system = "grpc",
        rpc.service = service,
        rpc.method = method,
        life.user_id = tracing::field::Empty,
        life.project_id = tracing::field::Empty,
        life.session_id = tracing::field::Empty,
    );
    if let Some(c) = claims {
        span.record("life.user_id", c.user_id.as_str());
        span.record("life.project_id", c.project_id.as_str());
        span.record("life.session_id", c.sid.value.as_str());
    }
    span
}

#[cfg(test)]
mod tests {
    use super::*;
    use aios_proto::aios::v1 as aios_v1;

    use crate::auth::capability::Tier;

    #[test]
    fn rpc_span_records_attributes_without_panic() {
        let span = rpc_span("life.v1.Agent", "CreateSession", None);
        let _enter = span.enter();
        // Span is real; no panic.
    }

    #[test]
    fn rpc_span_with_claims_records_life_attributes() {
        let claims = CapabilityClaims {
            user_id: "alice".to_string(),
            project_id: "proj".to_string(),
            sid: aios_v1::SessionId {
                value: "sid-1".to_string(),
            },
            scopes: vec![],
            tier: Tier::Free,
            exp: std::time::Instant::now() + std::time::Duration::from_secs(60),
        };
        let span = rpc_span("life.v1.Wallet", "Debit", Some(&claims));
        let _enter = span.enter();
    }
}
