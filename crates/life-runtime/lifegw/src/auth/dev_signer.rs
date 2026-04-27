//! Dev-mode JWT acceptance — `Bearer dev-token-for-{user_id}`.
//!
//! Sub-phase A only. Gated by `cfg.auth.dev_signer_enabled`. When enabled,
//! the middleware accepts the magic Bearer string and synthesises a
//! [`Tier1Claims`] body. Sub-phase B replaces this with a real ES256+JWKS
//! verifier; the magic-Bearer path is removed once production cuts over.

use crate::auth::tier1::Tier1Claims;
use crate::error::{LifegwError, LifegwResult};

const DEV_BEARER_PREFIX: &str = "dev-token-for-";

/// Verify a dev-mode bearer token. Returns synthesised Tier-1 claims on
/// success; `LifegwError::Auth` otherwise.
pub fn verify(bearer: &str) -> LifegwResult<Tier1Claims> {
    if let Some(user_id) = bearer.strip_prefix(DEV_BEARER_PREFIX) {
        if user_id.is_empty() {
            return Err(LifegwError::Auth("empty dev user_id".to_string()));
        }
        Ok(Tier1Claims {
            user_id: user_id.to_string(),
            project_id: "default-project".to_string(),
            scopes: vec!["agent:dispatch".to_string()],
        })
    } else {
        Err(LifegwError::Auth(
            "dev signer rejects non-dev bearer (expected dev-token-for-{user_id})".to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dev_signer_accepts_well_formed_bearer() {
        let claims = verify("dev-token-for-user-1").expect("accept dev token");
        assert_eq!(claims.user_id, "user-1");
        assert_eq!(claims.project_id, "default-project");
        assert_eq!(claims.scopes, vec!["agent:dispatch".to_string()]);
    }

    #[test]
    fn dev_signer_rejects_non_dev_bearer() {
        assert!(matches!(
            verify("eyJhbGciOiJFUzI1NiJ9..."),
            Err(LifegwError::Auth(_))
        ));
        assert!(matches!(verify(""), Err(LifegwError::Auth(_))));
    }

    #[test]
    fn dev_signer_rejects_empty_user_id() {
        assert!(matches!(
            verify("dev-token-for-"),
            Err(LifegwError::Auth(_))
        ));
    }
}
