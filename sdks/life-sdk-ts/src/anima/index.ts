/**
 * `@broomva/life-sdk/anima` — browser custody surface.
 *
 * Spec D D-Sub-C entry point. Public exports:
 *
 *   Composition:
 *     - {@link WebCryptoAnima}      — primary browser custody handle
 *     - {@link enrollWebCryptoAnima} — first-time setup helper
 *     - {@link loadWebCryptoAnima}  — subsequent-session helper
 *
 *   Building blocks:
 *     - {@link PasskeyOracle}    — auth-half (P-256 passkey)
 *     - {@link RemoteAnimaClient} — wallet-half delegate
 *     - {@link SessionCap}        — Tier-User cap lifecycle
 *
 *   DID utilities (Spec D L4-D6):
 *     - {@link generateDidKeyP256}
 *     - {@link resolveDidKeyP256}
 *     - {@link verifyDidKeyP256}
 *
 *   Errors:
 *     - {@link AnimaError}
 *
 *   Types (mirroring Rust's `custody.rs`):
 *     - {@link BackendKind}
 *     - {@link TxRequest}
 *     - {@link EvmSignature}
 *     - {@link Eip712Domain}
 *     - {@link DidRotationEvent}
 *     - {@link AttestationObject}
 *     - {@link Assertion}
 *
 * @see docs/superpowers/specs/2026-04-29-spec-d-anima-custody.md
 */

export {
  WebCryptoAnima,
  type WebCryptoAnimaConfig,
  enrollWebCryptoAnima,
  loadWebCryptoAnima,
} from "./web_crypto.js";

export {
  PasskeyOracle,
  type PasskeyOracleConfig,
  type PasskeyEnrollResult,
  type PasskeyLoadResult,
  type CredentialsContainerLike,
  type PublicKeyCredentialLike,
  type AuthenticatorAttestationResponseLike,
  type AuthenticatorAssertionResponseLike,
  parsePubkeyFromAttestation,
  derSignatureToJoseRaw,
} from "./passkey.js";

export {
  RemoteAnimaClient,
  type RemoteAnimaClientConfig,
  type EnrollPasskeyResult,
  type MintSessionCapResult,
} from "./remote.js";

export { SessionCap, type SessionCapConfig } from "./session_cap.js";

export {
  generateDidKeyP256,
  resolveDidKeyP256,
  verifyDidKeyP256,
} from "./did.js";

export { AnimaError } from "./errors.js";

export type {
  BackendKind,
  TxRequest,
  EvmSignature,
  Eip712Domain,
  DidRotationEvent,
  AttestationObject,
  Assertion,
} from "./types.js";
