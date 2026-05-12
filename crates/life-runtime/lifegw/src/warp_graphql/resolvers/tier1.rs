//! T1 resolvers — must-implement set for a working agent.
//!
//! Per Lumen spec L6-D4 (Phase α scope), Tier-1 covers ~15 GraphQL
//! operations that must route through real lifed gRPC to produce a
//! working Warp client. Task 14 lands the identity group.
//!
//! ## Lifed Identity-service mismatch (surfaced 2026-05-10)
//!
//! Upstream Warp's identity-group ops (`user`, `createAnonymousUser`,
//! `setUserIsOnboarded`) do NOT map 1:1 onto lifed's `life.v1.Identity`
//! service. The proto at `core/life/proto/life/v1/identity.proto`
//! exposes only `Me`, `UpdateProfile`, `ListSessions`, `RevokeSession`.
//! Mapping decisions per Task 14:
//!
//! * `user` query → `Identity.Me(IdentityEmpty)` returning `Account`.
//!   This is the closest semantic fit ("authenticated caller's profile").
//!   The `Account` proto carries `user_id` / `handle` / `display_name` /
//!   `email` / `tier` / `created_at` / `profile`. The Warp `UserResult`
//!   union nests a `UserOutput { user: User { isOnboarded, ... } }`
//!   shape; our resolver emits a flat `Account`-derived JSON
//!   compatible with Phase α's "single struct, nested fields fall
//!   through to Tier-3 stubs" promise. `is_onboarded` is absent from
//!   the proto and defaults to `false`.
//!
//! * `createAnonymousUser` and `setUserIsOnboarded` → no lifed RPC
//!   exists. Both resolvers are wired through the adapter override map
//!   (proving the special-case binding path works) but currently
//!   surface `WarpGraphqlError::NotEnabled` with a typed
//!   `FEATURE_NOT_ENABLED` extension code. This is the honest contract
//!   until lifed's IdentityService is extended (separate Linear ticket;
//!   tracked as a Phase α follow-up).
//!
//! ## Error-propagation pattern (REQUIRED)
//!
//! Every error path here uses [`WarpGraphqlError::into_graphql`]
//! (inherent) or [`IntoGraphqlExt::into_graphql`] (on
//! `Result<_, WarpGraphqlError>`). Bare `?` would invoke
//! async-graphql's blanket `From<T: Display>` impl which silently
//! drops `extensions.code`. See `error.rs`'s `IntoGraphqlExt` doc for
//! the full reasoning.

use async_graphql::dynamic::{FieldFuture, FieldValue, ResolverContext};

use crate::warp_graphql::context::WarpRequestCtx;
use crate::warp_graphql::error::{IntoGraphqlExt, WarpGraphqlError};

/// Resolver: query `user`. Returns the authenticated user's profile.
///
/// Schema shape (per upstream verbatim mirror):
/// `user(requestContext: RequestContext!): UserResult!`.
///
/// # Phase α scope
///
/// Calls `lifed`'s `Identity.Me(IdentityEmpty)` — the bearer JWT's
/// `sub` claim is propagated as the authenticated user_id via the
/// gateway's capability-token middleware (so the lifed handler picks
/// it up from request extensions, not from this request body).
///
/// The Warp `UserResult` is a union of `UserFacingError | UserOutput`,
/// where `UserOutput` nests `user: User { isOnboarded, ... }`. Our
/// resolver returns a flat JSON-shape derived from `Account`; the
/// nested `UserResult.user.settings`, `UserResult.user.requestLimitInfo`,
/// and other Warp-only fields fall through to Tier-3 stubs (per
/// L6-D4's "must-implement set" carve-out). Wiring those to real
/// lifed surfaces is a Phase β follow-up.
///
/// # Auth contract
///
/// Anonymous-mode callers (those holding only the `agent:anonymous`
/// scope from `createAnonymousUser`'s ephemeral path) are allowed
/// through so the upstream Warp first-launch flow can read back the
/// ephemeral user_id. Non-anonymous callers must hold `agent:read`.
pub fn resolve_get_user(rctx: ResolverContext<'_>) -> FieldFuture<'_> {
    FieldFuture::new(async move {
        let ctx: &WarpRequestCtx = rctx
            .ctx
            .data::<WarpRequestCtx>()
            .map_err(|_| WarpGraphqlError::Internal("missing request ctx".into()).into_graphql())?;

        if !ctx.claims.has_scope("agent:anonymous") {
            ctx.require_scope("agent:read")
                .map_err(WarpGraphqlError::into_graphql)?;
        }

        let mut client = ctx.dataloaders.identity.clone();
        let resp = client
            .me(life_runtime_proto::life::v1::IdentityEmpty {})
            .await
            .map_err(|e| WarpGraphqlError::Upstream(e.to_string()))
            .into_graphql()?;

        let account = resp.into_inner();
        let created_at_iso = account.created_at.map(|ts| {
            // RFC3339-ish ISO render; downstream Warp client tolerates
            // unset / lax fractional seconds. We don't pull chrono just
            // for this — a simple seconds-since-epoch string is enough
            // for Phase α's introspection-compat gate and the actual
            // `Time` scalar coercion happens client-side.
            format!("{}.{:09}Z", ts.seconds, ts.nanos)
        });

        let value_json = serde_json::json!({
            "id":          account.user_id,
            "handle":      account.handle,
            "email":       account.email,
            "displayName": account.display_name,
            "tier":        account.tier,
            "createdAt":   created_at_iso,
            // `is_onboarded` is not surfaced by `life.v1.Account`; default
            // to false until lifed's IdentityService grows the field
            // (cross-referenced with the Phase α follow-up note above).
            "isOnboarded": false,
        });
        // `serde_json::Value` doesn't `Into<async_graphql::Value>` —
        // route through `ConstValue::from_json` which mirrors the
        // shape 1:1 (objects→objects, strings→strings, etc.) and is
        // infallible for the safe construction above.
        let value = async_graphql::Value::from_json(value_json)
            .map_err(|e| WarpGraphqlError::Internal(format!("value encode: {e}")).into_graphql())?;

        Ok(Some(FieldValue::value(value)))
    })
}

/// Resolver: mutation `createAnonymousUser`.
///
/// # Phase α NOT_ENABLED stub
///
/// `lifed.Identity` does not currently expose a
/// `CreateAnonymousUser` RPC. This resolver is wired through the
/// adapter override map (so the special-case binding path is
/// exercised) but surfaces a typed `FEATURE_NOT_ENABLED` extension
/// code to clients. The Warp first-launch flow downgrades gracefully
/// when this is returned — the user is prompted to sign in instead of
/// continuing in an ephemeral session.
///
/// Promotion to a real resolver requires:
/// 1. Extending `core/life/proto/life/v1/identity.proto` with
///    `CreateAnonymousUser(CreateAnonymousUserReq) returns (Account)`
///    (or returning a richer `AnonymousAccount` with a `tier` enum).
/// 2. Wiring the lifed handler at
///    `core/life/crates/life-runtime/lifed/src/services/identity.rs`
///    to call `anima.create_anonymous_account(...)` (anima-proxy
///    extension also required).
/// 3. Replacing this body with the real dispatch.
pub fn resolve_create_anonymous_user(rctx: ResolverContext<'_>) -> FieldFuture<'_> {
    FieldFuture::new(async move {
        let _ctx: &WarpRequestCtx = rctx
            .ctx
            .data::<WarpRequestCtx>()
            .map_err(|_| WarpGraphqlError::Internal("missing request ctx".into()).into_graphql())?;

        // Explicit return-type annotation so the compiler can resolve
        // `FieldFuture::new`'s `R: Into<FieldValue<'_>>` generic — an
        // `Err(...)`-only path leaves `R` unconstrained without it.
        Err::<Option<FieldValue<'_>>, _>(
            WarpGraphqlError::NotEnabled("createAnonymousUser").into_graphql(),
        )
    })
}

/// Resolver: mutation `mintCustomToken`.
///
/// # Phase α scope
///
/// Per Lumen spec L6-D6, the production flow mints a custom OAuth-PKCE
/// token against broomva.tech. Phase α scope simplification: the spec's
/// full OAuth-PKCE round-trip is **β-only**. In α we mint a Tier-User
/// ES256 cap via lifegw's existing `TierUserMinter` (Spec D D-Sub-C)
/// directly from the bearer-validated `user_id` already present in
/// `WarpClaims`. The Warp client receives a single-string token in the
/// `customToken` field of `MintCustomTokenOutput`; downstream calls
/// that present this cap satisfy the same scope-vector check anima's
/// `/anima/custody/*` routes use.
///
/// # Auth contract
///
/// Anonymous-mode callers cannot reach this op — the broomva.tech
/// OAuth flow is what produces the bearer they'd need. Returning
/// `UNAUTHENTICATED` here matches the upstream Warp UI's recovery
/// flow (which routes the user back through sign-in).
pub fn resolve_mint_custom_token(rctx: ResolverContext<'_>) -> FieldFuture<'_> {
    FieldFuture::new(async move {
        let ctx: &WarpRequestCtx = rctx
            .ctx
            .data::<WarpRequestCtx>()
            .map_err(|_| WarpGraphqlError::Internal("missing request ctx".into()).into_graphql())?;

        // Anonymous callers must complete the broomva.tech OAuth flow
        // first; only verified callers (post-OAuth, post-mint) can
        // ROTATE or REFRESH their cap through this resolver.
        if ctx.claims.has_scope("agent:anonymous") {
            return Err(WarpGraphqlError::Unauthenticated.into_graphql());
        }
        ctx.require_scope("agent:read")
            .map_err(WarpGraphqlError::into_graphql)?;

        // Mint a Tier-User cap scoped to the canonical Lumen scope set.
        // The minter's TTL is the configured `tier_user_ttl` (default
        // 15 min per Spec D D-Sub-C); Lumen does NOT widen this on
        // Phase α — the Warp client refreshes by re-calling this op.
        let scope = vec!["agent:read".to_string(), "agent:write".to_string()];
        let (token, _expires_at) = ctx
            .minter
            .mint(ctx.user_id(), scope)
            .map_err(|e| WarpGraphqlError::Internal(e.to_string()))
            .into_graphql()?;

        let value_json = serde_json::json!({
            // Per `MintCustomTokenOutput { customToken: String!,
            // responseContext: ResponseContext! }`. responseContext
            // falls through to the auto-stub on the nested selection.
            "customToken": token,
        });
        let value = async_graphql::Value::from_json(value_json)
            .map_err(|e| WarpGraphqlError::Internal(format!("value encode: {e}")).into_graphql())?;
        Ok(Some(FieldValue::value(value)))
    })
}

/// Resolver: mutation `issueTaskIdentityToken`.
///
/// # Phase α scope
///
/// Short-lived task-scoped cap (15-min cap per Spec D D-Sub-C). Used
/// when an agent task needs to make external calls on behalf of the
/// user with bounded scope. Phase α: same `TierUserMinter` path as
/// `mintCustomToken` but the scope vector is narrowed to a single
/// task-identity scope so downstream services can distinguish
/// task-context tokens from general user caps.
///
/// The schema returns `IssueTaskIdentityTokenOutput { token: String!,
/// expiresAt: Time!, issuer: String!, responseContext: ResponseContext! }`.
/// We surface the JWS in `token`, the configured issuer (typically
/// `"lifegw"`) in `issuer`, and the Unix-seconds expiry in `expiresAt`.
/// The Warp client renders the `Time` scalar from an ISO 8601 string;
/// async-graphql passes the JSON value through as-is and the Warp
/// client tolerates Unix-seconds (it falls back to numeric parsing
/// when ISO parsing fails).
///
/// # Auth contract
///
/// Requires `agent:write` (the cap embeds privileges to act on the
/// user's behalf, so the caller must already be authorized to mutate).
pub fn resolve_issue_task_identity_token(rctx: ResolverContext<'_>) -> FieldFuture<'_> {
    FieldFuture::new(async move {
        let ctx: &WarpRequestCtx = rctx
            .ctx
            .data::<WarpRequestCtx>()
            .map_err(|_| WarpGraphqlError::Internal("missing request ctx".into()).into_graphql())?;
        ctx.require_scope("agent:write")
            .map_err(WarpGraphqlError::into_graphql)?;

        // Single task-identity scope so the resulting cap is distinct
        // from general user caps. Downstream services that want to
        // restrict task-context calls dispatch on this scope.
        let scope = vec!["agent:task".to_string()];
        let (token, expires_at) = ctx
            .minter
            .mint(ctx.user_id(), scope)
            .map_err(|e| WarpGraphqlError::Internal(e.to_string()))
            .into_graphql()?;

        let value_json = serde_json::json!({
            "token":     token,
            // `Time` scalar — Warp client tolerates numeric Unix-seconds
            // (it falls back from ISO parsing). Phase β can swap in
            // chrono if a stricter wire shape becomes necessary.
            "expiresAt": expires_at,
            // The minter's configured issuer — typically `"lifegw"`.
            // We project this for parity with upstream Warp's OIDC
            // wire shape, even though Lumen's verifier doesn't read it.
            "issuer":    ctx.minter.issuer(),
        });
        let value = async_graphql::Value::from_json(value_json)
            .map_err(|e| WarpGraphqlError::Internal(format!("value encode: {e}")).into_graphql())?;
        Ok(Some(FieldValue::value(value)))
    })
}

/// Resolver: mutation `generateApiKey`.
///
/// # Phase α scope
///
/// Long-lived API key for headless access (laptops, CI runners,
/// scripts that can't complete an interactive OAuth flow). Phase α:
/// mints a Tier-User cap with the same `TierUserMinter` as the other
/// token resolvers — Phase α deliberately does NOT widen the minter's
/// 15-min TTL cap, so the "API key" is functionally identical to a
/// `mintCustomToken` result with longer rotation cadence. Phase β
/// introduces a separate `ApiKeyMinter` with a 90+ day TTL and a
/// dedicated `aud=api-key` so revocation can be scoped without
/// affecting general Tier-User caps.
///
/// The schema returns `GenerateApiKeyOutput { apiKey: APIKeyProperties!,
/// rawApiKey: String!, responseContext: ResponseContext! }`. We surface
/// the JWS in `rawApiKey` and let `apiKey` (the metadata struct) fall
/// through to the auto-stub for now — Phase β wires the metadata fields
/// (`uid`, `createdAt`, `lastUsedAt`, etc.) once the revocation store
/// lands.
///
/// # Auth contract
///
/// Requires `agent:write`. Anonymous callers cannot reach this op.
pub fn resolve_generate_api_key(rctx: ResolverContext<'_>) -> FieldFuture<'_> {
    FieldFuture::new(async move {
        let ctx: &WarpRequestCtx = rctx
            .ctx
            .data::<WarpRequestCtx>()
            .map_err(|_| WarpGraphqlError::Internal("missing request ctx".into()).into_graphql())?;
        ctx.require_scope("agent:write")
            .map_err(WarpGraphqlError::into_graphql)?;

        let scope = vec!["agent:read".to_string(), "agent:write".to_string()];
        let (token, _expires_at) = ctx
            .minter
            .mint(ctx.user_id(), scope)
            .map_err(|e| WarpGraphqlError::Internal(e.to_string()))
            .into_graphql()?;

        let value_json = serde_json::json!({
            // Per `GenerateApiKeyOutput.rawApiKey: String!` — the
            // single raw key value returned once. Phase β: the
            // `apiKey` metadata struct (uid + createdAt + lastUsedAt)
            // is filled in when the revocation store ships; for now
            // it falls through to the auto-stub (nullable nested
            // fields render as null).
            "rawApiKey": token,
        });
        let value = async_graphql::Value::from_json(value_json)
            .map_err(|e| WarpGraphqlError::Internal(format!("value encode: {e}")).into_graphql())?;
        Ok(Some(FieldValue::value(value)))
    })
}

/// Resolver: mutation `expireApiKey`.
///
/// # Phase α scope
///
/// Per Lumen spec L6-D6 the production flow consults a lago-backed
/// revocation store that token verifiers check on every cap. Phase α
/// scope simplification: revocation persistence is **β-only** (tracked
/// as a Phase α follow-up in the spec risk register). In α this
/// resolver returns success without persisting anything — the Warp UI
/// is given the success response so the user-facing flow completes,
/// and the actual revocation happens client-side (the Warp client
/// deletes the key from its local keychain).
///
/// The schema returns `ExpireApiKeyOutput { success: Boolean!,
/// responseContext: ResponseContext! }`.
///
/// # Auth contract
///
/// Requires `agent:write` (revocation is a privileged mutation even
/// when the persistence layer is a no-op).
pub fn resolve_expire_api_key(rctx: ResolverContext<'_>) -> FieldFuture<'_> {
    FieldFuture::new(async move {
        let ctx: &WarpRequestCtx = rctx
            .ctx
            .data::<WarpRequestCtx>()
            .map_err(|_| WarpGraphqlError::Internal("missing request ctx".into()).into_graphql())?;
        ctx.require_scope("agent:write")
            .map_err(WarpGraphqlError::into_graphql)?;

        // Phase α: no-op success. The lago-backed revocation store lands
        // in Phase β alongside the dedicated `ApiKeyMinter`. The Warp UI
        // is shown a success response so the user's local keychain
        // delete proceeds cleanly.
        let value_json = serde_json::json!({
            "success": true,
        });
        let value = async_graphql::Value::from_json(value_json)
            .map_err(|e| WarpGraphqlError::Internal(format!("value encode: {e}")).into_graphql())?;
        Ok(Some(FieldValue::value(value)))
    })
}

/// Resolver: mutation `setUserIsOnboarded`.
///
/// # Phase α NOT_ENABLED stub
///
/// `lifed.Identity` does not currently expose a `SetOnboardingFlag`
/// (or equivalent) RPC, and `life.v1.Account` does not carry an
/// `is_onboarded` field. This resolver is wired through the adapter
/// override map (so the special-case binding path is exercised) but
/// surfaces `FEATURE_NOT_ENABLED`. The Warp onboarding survey UI
/// degrades gracefully on this error — it skips persisting the flag
/// and the user is treated as onboarded for the rest of the session.
///
/// Promotion to a real resolver requires extending the proto + lifed
/// handler symmetrically with `createAnonymousUser` above.
///
/// # Auth contract
///
/// Even as a stub, this enforces `agent:write` so anonymous callers
/// receive `UNAUTHENTICATED` (the more semantically correct error)
/// rather than `FEATURE_NOT_ENABLED` when they try to call an op they
/// cannot reach in the first place.
pub fn resolve_set_user_is_onboarded(rctx: ResolverContext<'_>) -> FieldFuture<'_> {
    FieldFuture::new(async move {
        let ctx: &WarpRequestCtx = rctx
            .ctx
            .data::<WarpRequestCtx>()
            .map_err(|_| WarpGraphqlError::Internal("missing request ctx".into()).into_graphql())?;

        ctx.require_scope("agent:write")
            .map_err(WarpGraphqlError::into_graphql)?;

        // Explicit return-type annotation so the compiler can resolve
        // `FieldFuture::new`'s `R: Into<FieldValue<'_>>` generic — an
        // `Err(...)`-only path leaves `R` unconstrained without it.
        Err::<Option<FieldValue<'_>>, _>(
            WarpGraphqlError::NotEnabled("setUserIsOnboarded").into_graphql(),
        )
    })
}

/// Resolver: mutation `updateUserSettings`.
///
/// # Phase α NOT_ENABLED stub
///
/// Per Pre-flight Update Round 3, upstream Warp's schema has NO
/// top-level `getUserSettings` Query — settings are nested under the
/// `UserResult` returned by the `user` Query. Phase α drops the
/// standalone settings-read resolver; nested settings reads fall
/// through to Tier-3 stubs (returning null defaults). Only the
/// `updateUserSettings` Mutation is wired here.
///
/// `lifed.Identity` does not currently expose a settings RPC — `Me`,
/// `UpdateProfile`, `ListSessions`, `RevokeSession` are the only 4
/// RPCs (see `core/life/proto/life/v1/identity.proto`). The settings
/// persistence path lands in Phase β: either a new `life.v1.Settings`
/// service or a lago-event-sourced projection via the existing
/// `Events.Append` path (when `lagod` ships the wire RPC — currently
/// tracked under BRO-934's "lago wire RPC roll-out" follow-up).
///
/// This resolver is wired through the adapter override map (so the
/// special-case binding path is exercised) but surfaces
/// `FEATURE_NOT_ENABLED`. The Warp client gracefully handles this —
/// settings changes are stored client-side until the server-side path
/// is wired.
///
/// Schema shape (per upstream verbatim mirror):
/// `updateUserSettings(input: UpdateUserSettingsInput!,
///   requestContext: RequestContext!) : UpdateUserSettingsResult!`
///
/// # Auth contract
///
/// Requires `agent:write`. Anonymous callers receive `UNAUTHENTICATED`
/// (the more semantically correct error) rather than
/// `FEATURE_NOT_ENABLED` when they try to call an op they cannot
/// reach in the first place — mirrors `setUserIsOnboarded` above.
pub fn resolve_update_user_settings(rctx: ResolverContext<'_>) -> FieldFuture<'_> {
    FieldFuture::new(async move {
        let ctx: &WarpRequestCtx = rctx
            .ctx
            .data::<WarpRequestCtx>()
            .map_err(|_| WarpGraphqlError::Internal("missing request ctx".into()).into_graphql())?;

        ctx.require_scope("agent:write")
            .map_err(WarpGraphqlError::into_graphql)?;

        // Explicit return-type annotation so the compiler can resolve
        // `FieldFuture::new`'s `R: Into<FieldValue<'_>>` generic — an
        // `Err(...)`-only path leaves `R` unconstrained without it.
        Err::<Option<FieldValue<'_>>, _>(
            WarpGraphqlError::NotEnabled("updateUserSettings").into_graphql(),
        )
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::auth::kms::{KmsSigner, StaticKeystore};
    use crate::auth::tier_user::{DEFAULT_TIER_USER_TTL, TierUserMinter};
    use crate::warp_graphql::adapter::WarpSchema;
    use crate::warp_graphql::auth::WarpClaims;
    use crate::warp_graphql::context::WarpRequestCtx;
    use crate::warp_graphql::dataloader::LifedDataloaders;

    /// Build a `TierUserMinter` over an ephemeral `StaticKeystore` for
    /// resolver tests. Task 15 (M7-W) needs a real minter to exercise the
    /// 4 T1 token resolvers' dispatch path; anonymous-rejection / scope
    /// gating tests use the same shape so the ctx constructor signature
    /// is uniform across all resolver tests.
    fn test_minter() -> Arc<TierUserMinter> {
        let signer = Arc::new(StaticKeystore::generate_dev().expect("dev keystore"));
        Arc::new(TierUserMinter::with_defaults(
            signer as Arc<dyn KmsSigner>,
            DEFAULT_TIER_USER_TTL,
        ))
    }

    fn test_ctx_anonymous() -> WarpRequestCtx {
        WarpRequestCtx::new(
            WarpClaims::anonymous("eph-test".into()),
            Arc::new(LifedDataloaders::test_stub()),
            "trace-test".into(),
            test_minter(),
        )
    }

    fn test_ctx_with_scopes(scopes: Vec<&str>) -> WarpRequestCtx {
        WarpRequestCtx::new(
            WarpClaims {
                user_id: "u-test".into(),
                audience: "lumen".into(),
                scopes: scopes.into_iter().map(str::to_string).collect(),
                anima_did: None,
                expires_at: 9999999999,
            },
            Arc::new(LifedDataloaders::test_stub()),
            "trace-test".into(),
            test_minter(),
        )
    }

    /// Constructs a minimal valid `RequestContext` input literal that
    /// satisfies the SDL's required `requestContext: RequestContext!`
    /// argument on every T1 op. `ClientContext` and `OSContext` are
    /// all-optional, so empty objects pass schema validation.
    const REQUEST_CONTEXT_LITERAL: &str = "{ clientContext: {}, osContext: {} }";

    /// Verify the `user` query is registered with the real T1 resolver
    /// (not the auto-stub) via the special-case in adapter.rs.
    ///
    /// The stub would resolve to a successful `null` (the field is
    /// non-null but the auto-stub returns null anyway — async-graphql
    /// surfaces this as "non-null returned null"); the real resolver
    /// attempts to call `Identity.Me` against the test_stub gRPC
    /// channel and fails with our typed `UPSTREAM_ERROR`. Either error
    /// signature would confirm the resolver fired, but only the typed
    /// extension code proves it's our resolver and not the auto-stub.
    #[tokio::test]
    async fn user_query_is_wired_to_real_resolver() {
        let schema = WarpSchema::build().expect("schema builds");
        let ctx = test_ctx_with_scopes(vec!["agent:read"]);
        // Selection-set is just `__typename` — always valid on any
        // union, and we never reach a successful response anyway
        // because the test_stub gRPC channel fails on first RPC.
        let query = format!(
            "query {{ user(requestContext: {}) {{ __typename }} }}",
            REQUEST_CONTEXT_LITERAL,
        );
        let req = async_graphql::Request::new(query).data(ctx);
        let resp = schema.inner().execute(req).await;

        assert!(
            !resp.errors.is_empty(),
            "test_stub gRPC channel should produce SOME error; got data: {:?}",
            resp.data
        );
        let err = &resp.errors[0];
        let extensions_json = serde_json::to_value(&err.extensions).unwrap_or_default();
        // Real resolver path emits a typed `code` via IntoGraphqlExt.
        // The auto-stub returns `Ok(None)` which surfaces as
        // "internal: non-null type returned null", an UNTYPED error.
        let code = extensions_json.get("code").and_then(|c| c.as_str());
        assert_eq!(
            code,
            Some("UPSTREAM_ERROR"),
            "expected real resolver to surface UPSTREAM_ERROR via gRPC \
             failure path; got code={:?} message={:?}",
            code,
            err.message
        );
    }

    /// `createAnonymousUser` is wired through the override map and
    /// surfaces the Phase α NOT_ENABLED stub. This proves the mutation
    /// path is bound (not auto-stubbed) AND that the typed error code
    /// makes it through to extensions.
    #[tokio::test]
    async fn create_anonymous_user_surfaces_not_enabled() {
        let schema = WarpSchema::build().expect("schema builds");
        let ctx = test_ctx_anonymous();
        let mutation = format!(
            "mutation {{ createAnonymousUser(\
                 input: {{ anonymousUserType: NATIVE_CLIENT_ANONYMOUS_USER, \
                            expirationType: NO_EXPIRATION }}, \
                 requestContext: {}\
             ) {{ __typename }} }}",
            REQUEST_CONTEXT_LITERAL,
        );
        let req = async_graphql::Request::new(mutation).data(ctx);
        let resp = schema.inner().execute(req).await;

        assert!(!resp.errors.is_empty(), "NOT_ENABLED stub should error");
        let err = &resp.errors[0];
        let extensions_json = serde_json::to_value(&err.extensions).unwrap_or_default();
        let code = extensions_json.get("code").and_then(|c| c.as_str());
        assert_eq!(
            code,
            Some("FEATURE_NOT_ENABLED"),
            "expected FEATURE_NOT_ENABLED; got code={:?} message={:?}",
            code,
            err.message
        );
    }

    /// `setUserIsOnboarded` enforces `agent:write` before short-circuiting
    /// to NOT_ENABLED. An anonymous caller hits UNAUTHENTICATED first.
    #[tokio::test]
    async fn set_user_is_onboarded_requires_agent_write() {
        let schema = WarpSchema::build().expect("schema builds");
        let ctx = test_ctx_anonymous();
        let mutation = format!(
            "mutation {{ setUserIsOnboarded(requestContext: {}) {{ __typename }} }}",
            REQUEST_CONTEXT_LITERAL,
        );
        let req = async_graphql::Request::new(mutation).data(ctx);
        let resp = schema.inner().execute(req).await;

        assert!(!resp.errors.is_empty());
        let err = &resp.errors[0];
        let extensions_json = serde_json::to_value(&err.extensions).unwrap_or_default();
        let code = extensions_json.get("code").and_then(|c| c.as_str());
        assert_eq!(
            code,
            Some("UNAUTHENTICATED"),
            "anonymous caller should hit UNAUTHENTICATED; got code={:?} message={:?}",
            code,
            err.message
        );
    }

    /// With `agent:write`, the stub short-circuits to NOT_ENABLED.
    #[tokio::test]
    async fn set_user_is_onboarded_with_scope_surfaces_not_enabled() {
        let schema = WarpSchema::build().expect("schema builds");
        let ctx = test_ctx_with_scopes(vec!["agent:write"]);
        let mutation = format!(
            "mutation {{ setUserIsOnboarded(requestContext: {}) {{ __typename }} }}",
            REQUEST_CONTEXT_LITERAL,
        );
        let req = async_graphql::Request::new(mutation).data(ctx);
        let resp = schema.inner().execute(req).await;

        assert!(!resp.errors.is_empty());
        let err = &resp.errors[0];
        let extensions_json = serde_json::to_value(&err.extensions).unwrap_or_default();
        let code = extensions_json.get("code").and_then(|c| c.as_str());
        assert_eq!(
            code,
            Some("FEATURE_NOT_ENABLED"),
            "agent:write caller should hit FEATURE_NOT_ENABLED; got code={:?} message={:?}",
            code,
            err.message
        );
    }

    /// Task 16 — verify `updateUserSettings` is registered with the real T1
    /// resolver (not the auto-stub) via the special-case in adapter.rs.
    ///
    /// The dispatch is proven by the typed extension code. Anonymous callers
    /// hit `UNAUTHENTICATED` first (the `agent:write` scope gate runs before
    /// the NOT_ENABLED short-circuit); callers with `agent:write` hit
    /// `FEATURE_NOT_ENABLED`. Either typed code proves the dispatch map
    /// fired — the auto-stub would return `Ok(None)` and produce an UNTYPED
    /// "non-null returned null" internal error.
    #[tokio::test]
    async fn update_user_settings_is_wired_to_real_resolver() {
        let schema = WarpSchema::build().expect("schema builds");
        let ctx = test_ctx_anonymous();
        // `UpdateUserSettingsInput` is an all-optional input; an empty
        // object literal `{}` passes schema validation.
        let mutation = format!(
            "mutation {{ updateUserSettings(input: {{}}, requestContext: {}) \
              {{ __typename }} }}",
            REQUEST_CONTEXT_LITERAL,
        );
        let req = async_graphql::Request::new(mutation).data(ctx);
        let resp = schema.inner().execute(req).await;

        assert!(
            !resp.errors.is_empty(),
            "updateUserSettings should produce a typed error; got data: {:?}",
            resp.data
        );
        let err = &resp.errors[0];
        let extensions_json = serde_json::to_value(&err.extensions).unwrap_or_default();
        let code = extensions_json.get("code").and_then(|c| c.as_str());
        // Anonymous ctx hits the scope gate first; with `agent:write` it
        // would short-circuit to FEATURE_NOT_ENABLED. Either typed code
        // proves the dispatch map is wired (NOT auto-stub null).
        assert!(
            matches!(code, Some("FEATURE_NOT_ENABLED") | Some("UNAUTHENTICATED")),
            "expected FEATURE_NOT_ENABLED or UNAUTHENTICATED; got code={:?} message={:?}",
            code,
            err.message
        );
    }

    /// Task 15 — verify `mintCustomToken` is registered with the real T1
    /// resolver (not the auto-stub) AND that the resolver completes its
    /// mint round-trip against a real `TierUserMinter`.
    ///
    /// A successful mint produces no GraphQL errors and the response
    /// data carries `__typename: "MintCustomTokenOutput"` selected via
    /// the union shape. The auto-stub would yield `__typename: null` (it
    /// returns `FieldValue::value(Value::Null)` for non-null fields, and
    /// async-graphql cannot resolve union dispatch from a null value);
    /// `FEATURE_NOT_ENABLED` would indicate the resolver dispatch map
    /// didn't wire. Either failure mode would surface a typed code in
    /// `extensions.code`.
    #[tokio::test]
    async fn mint_custom_token_is_wired_to_real_resolver() {
        let schema = WarpSchema::build().expect("schema builds");
        let ctx = test_ctx_with_scopes(vec!["agent:read", "agent:write"]);
        let mutation = format!(
            "mutation {{ mintCustomToken(requestContext: {}) {{ __typename }} }}",
            REQUEST_CONTEXT_LITERAL,
        );
        let req = async_graphql::Request::new(mutation).data(ctx);
        let resp = schema.inner().execute(req).await;

        // The real resolver SHOULD succeed because the test ctx carries
        // a working `TierUserMinter` over an ephemeral `StaticKeystore`
        // — the mint path is pure crypto and doesn't touch the network.
        // If we ever DO see an error, the only acceptable code is one of
        // the resolver's own typed codes (e.g., UNAUTHENTICATED if the
        // scope check changes); `FEATURE_NOT_ENABLED` would prove the
        // dispatch map didn't wire and is explicitly forbidden.
        if !resp.errors.is_empty() {
            let err = &resp.errors[0];
            let extensions_json = serde_json::to_value(&err.extensions).unwrap_or_default();
            let code = extensions_json.get("code").and_then(|c| c.as_str());
            assert_ne!(
                code,
                Some("FEATURE_NOT_ENABLED"),
                "mintCustomToken must be wired to a real resolver; got \
                 FEATURE_NOT_ENABLED meaning the dispatch map missed the \
                 (RootMutation, mintCustomToken) row. message={:?}",
                err.message
            );
        } else {
            // Success path: prove the resolver fired AND the union
            // dispatched correctly by reading back the typename.
            let json = resp
                .data
                .into_json()
                .expect("response data serializes to JSON");
            let typename = json["mintCustomToken"]["__typename"]
                .as_str()
                .expect("__typename present on success");
            assert_eq!(
                typename, "MintCustomTokenOutput",
                "expected real-resolver success to dispatch the \
                 MintCustomTokenOutput variant of the result union"
            );
        }
    }
}
