//! Error type for warp_graphql resolvers.
//!
//! Converts to `async_graphql::Error` with structured `extensions` so the
//! Warp client can render meaningful messages and our metrics can categorize
//! failures.

use async_graphql::ErrorExtensions;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum WarpGraphqlError {
    #[error("authentication required")]
    Unauthenticated,

    #[error("operation not enabled in this build: {0}")]
    NotEnabled(&'static str),

    #[error("upstream lifed error: {0}")]
    Upstream(String),

    #[error("invalid input: {0}")]
    InvalidInput(String),

    #[error("internal error: {0}")]
    Internal(String),
}

impl WarpGraphqlError {
    /// Stable error code surfaced to the Warp client in
    /// `extensions.code`. Used by upstream Warp UI to decide UX
    /// (e.g., redirect to login on UNAUTHENTICATED).
    pub fn code(&self) -> &'static str {
        match self {
            Self::Unauthenticated => "UNAUTHENTICATED",
            Self::NotEnabled(_) => "FEATURE_NOT_ENABLED",
            Self::Upstream(_) => "UPSTREAM_ERROR",
            Self::InvalidInput(_) => "INVALID_INPUT",
            Self::Internal(_) => "INTERNAL_ERROR",
        }
    }

    /// Convert into an `async_graphql::Error` with `extensions.code` set
    /// to the stable code from [`Self::code`].
    ///
    /// Inherent method (instead of a `From<WarpGraphqlError> for
    /// async_graphql::Error` impl) because async-graphql provides a
    /// blanket `impl<T: Display + Send + Sync + 'static> From<T> for
    /// Error` that would conflict with any custom impl. The blanket
    /// impl already lets resolvers use `?` on `WarpGraphqlError` —
    /// callers wanting the `extensions.code` annotation invoke this
    /// helper explicitly.
    pub fn into_graphql(self) -> async_graphql::Error {
        let code = self.code();
        async_graphql::Error::new(self.to_string()).extend_with(|_, ext| {
            ext.set("code", code);
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unauthenticated_has_stable_code() {
        let err = WarpGraphqlError::Unauthenticated;
        assert_eq!(err.code(), "UNAUTHENTICATED");
    }

    #[test]
    fn not_enabled_carries_feature_name() {
        let err = WarpGraphqlError::NotEnabled("createTeam");
        assert_eq!(err.code(), "FEATURE_NOT_ENABLED");
        assert_eq!(
            err.to_string(),
            "operation not enabled in this build: createTeam"
        );
    }

    #[test]
    fn converts_to_async_graphql_error_with_code() {
        let err: async_graphql::Error = WarpGraphqlError::InvalidInput("bad".into()).into_graphql();
        // The extension code is in the `extensions` field; we can verify
        // by serializing.
        let json = serde_json::to_value(&err).expect("error serializes");
        assert_eq!(json["extensions"]["code"], "INVALID_INPUT");
    }
}
