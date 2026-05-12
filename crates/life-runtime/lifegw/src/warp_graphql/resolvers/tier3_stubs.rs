//! Tier-3 stub helpers for unimplemented operations.
//!
//! Per Lumen spec L6-D4, ~90 of Warp's ~122 GraphQL operations are
//! Warp.dev SaaS scaffolding (teams, Stripe, Merkle sync, file
//! artifacts, managed secrets) that Lumen v1 does not reproduce.
//! Stubs here return safe SDL-compliant defaults so the Warp client
//! treats these as feature-flag-off rather than erroring.
//!
//! When a stubbed operation graduates to real implementation, move
//! its resolver from this file to tier1.rs or tier2.rs.

use crate::warp_graphql::error::WarpGraphqlError;

/// SDL-compliant safe default for a stubbed mutation that returns
/// a `MutationResponse`-shaped object. The actual Warp schema has
/// many such response types; the stub just returns `success: false`
/// with a clear reason so the UI can render "feature not enabled".
pub fn stub_mutation_response(operation: &'static str) -> serde_json::Value {
    serde_json::json!({
        "success": false,
        "reason": format!("operation '{}' is not enabled in this Lumen build", operation),
    })
}

/// Stubbed query that should return `null` (nullable single-object).
pub fn stub_null<T>() -> Option<T> {
    None
}

/// Stubbed query that should return an empty list.
pub fn stub_empty_list<T>() -> Vec<T> {
    Vec::new()
}

/// Stubbed query that the Warp client treats as a feature flag.
/// Returns a recognized "off" value rather than an error.
pub fn stub_feature_off(_feature: &'static str) -> Result<serde_json::Value, WarpGraphqlError> {
    Ok(serde_json::json!({
        "enabled": false,
        "reason": "not provisioned in this build",
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_mutation_response_includes_operation_name() {
        let v = stub_mutation_response("createTeam");
        assert_eq!(v["success"], false);
        assert!(v["reason"].as_str().unwrap().contains("createTeam"));
    }

    #[test]
    fn stub_empty_list_is_empty() {
        let v: Vec<i32> = stub_empty_list();
        assert!(v.is_empty());
    }

    #[test]
    fn stub_null_is_none() {
        let v: Option<i32> = stub_null();
        assert!(v.is_none());
    }

    #[test]
    fn stub_feature_off_returns_disabled() {
        let v = stub_feature_off("teamSync").unwrap();
        assert_eq!(v["enabled"], false);
    }
}
