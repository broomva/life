//! Auth subsystem — Tier-1 verify (in) → Tier-2 mint (out).
//!
//! Sub-phase B scope (Spec C₃ §5):
//! - `jwks` — JWKS-fetched ES256/RS256 verifier with kid lookup, refetch
//!   on miss, 30 min rotation grace, alg allowlist (`ES256`/`RS256`,
//!   `none` rejected, alg derived from JWKS — never the JWT header).
//! - `dev_signer` — Tier-1 entry-point `verify(bearer: &str)` whose body
//!   delegates to a global [`jwks::JwksCache`]. The shortcut
//!   `Bearer dev-token-for-{user_id}` survives behind
//!   [`jwks::JwksCache::dev_only`] for existing tests.
//! - `keystore` — local P-256 ES256 keypair material (used by the
//!   [`kms`] StaticKeystore provider in dev / CI / unit tests).
//! - `kms` — `KmsSigner` trait + provider impls (StaticKeystore,
//!   VaultTransit primary, AwsKms / GcpKms feature-gated). Production
//!   builds use `kms-vault`; only `StaticKeystore` is wired by default.
//! - `middleware` — tower Layer that validates inbound bearer, swaps it
//!   for a freshly-minted Tier-2 capability JWS, and forwards.
//! - `tier1` — Tier-1 claim shape (used by both the dev signer and the
//!   real verifier).
//! - `tier2` — Tier-2 capability claim shape + mint helper. Sub-phase B
//!   extends the claim body with `nbf`, `iat`, `jti`.

pub mod dev_signer;
pub mod jwks;
pub mod keystore;
pub mod kms;
pub mod middleware;
pub mod scope;
pub mod tier1;
pub mod tier2;

pub use jwks::{JwksCache, JwksCacheConfig, JwksDoc, JwksEntry, JwksSource};
pub use keystore::Keystore;
pub use kms::{KmsSigner, StaticKeystore};
pub use middleware::AuthLayer;
pub use scope::{RequiredScope, ScopeError, enforce as enforce_scope, required_scope};
pub use tier1::{DEFAULT_TIER, Tier1Claims};
pub use tier2::Tier2Claims;
