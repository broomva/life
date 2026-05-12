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

    /// The `&'static str` is intentional — known operation names are
    /// compile-time literals supplied at Tier-3 stub sites
    /// (e.g., `WarpGraphqlError::NotEnabled("createTeam")`). For
    /// dynamic op names from request input, use `InvalidInput(String)` instead.
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

/// Ergonomic `?`-compatible conversion from `Result<T, WarpGraphqlError>`
/// to `async_graphql::Result<T>` that preserves the `extensions.code`
/// annotation.
///
/// Resolvers MUST use this trait (not bare `?`) when returning errors
/// to the Warp client. Bare `?` invokes async-graphql's blanket
/// `impl<T: Display + Send + Sync + 'static> From<T> for Error`, which
/// produces an `Error` with NO extensions — silently dropping the stable
/// `code` that this module exists to surface. The Warp client then sees
/// only `error.message` and cannot route on the typed code.
///
/// Pattern in resolver code:
///
/// ```ignore
/// let resp = client
///     .get_user(req)
///     .await
///     .map_err(|e| WarpGraphqlError::Upstream(e.to_string()))
///     .into_graphql()?;   // ← propagates with extensions.code set
/// ```
///
/// Without this trait, the bare `?` form would compile but silently drop
/// the stable code. The trait name is the same as the inherent method
/// on `WarpGraphqlError` so call-sites use one mental model.
pub trait IntoGraphqlExt<T> {
    fn into_graphql(self) -> async_graphql::Result<T>;
}

impl<T> IntoGraphqlExt<T> for Result<T, WarpGraphqlError> {
    fn into_graphql(self) -> async_graphql::Result<T> {
        self.map_err(WarpGraphqlError::into_graphql)
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

    #[test]
    fn into_graphql_ext_preserves_extensions_code_on_question_mark() {
        // The extension trait must produce an `async_graphql::Error` with
        // `extensions.code` set — proving that resolver-style `.into_graphql()?`
        // does NOT silently lose the stable code (which is what bare `?`
        // would do via async-graphql's blanket Display-based impl).
        let result: Result<u32, WarpGraphqlError> = Err(WarpGraphqlError::Unauthenticated);
        let converted: async_graphql::Result<u32> = result.into_graphql();
        let err = converted.expect_err("Err preserved");
        let json = serde_json::to_value(&err).expect("error serializes");
        assert_eq!(json["extensions"]["code"], "UNAUTHENTICATED");
    }

    #[test]
    fn into_graphql_ext_passes_ok_through() {
        let result: Result<u32, WarpGraphqlError> = Ok(42);
        let converted: async_graphql::Result<u32> = result.into_graphql();
        assert_eq!(converted.expect("Ok preserved"), 42);
    }
}
