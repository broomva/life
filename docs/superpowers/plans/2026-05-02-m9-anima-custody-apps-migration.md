# M9 — Anima Custody Apps Migration Plan

**Date**: 2026-05-02
**Status**: PLANNED (Spec D substrate shipped 2026-05-02; chatOS apps migration begins)
**Owner**: Wave 4 (apps integration)
**Critical-path predecessors**: ✅ Spec D 100% complete (Waves 1–3) — all 6 production custody backends shipped; ✅ M8 SDK v0.1.0-pre at `sdks/life-sdk-ts/` with WebCryptoAnima + RemoteAnimaClient + SessionCap; ✅ lifegw `/anima/custody/*` 6 routes + TierUserMinter + WS bearer subprotocol shipped.
**Critical-path successors**: M10 (public launch).

## What Wave 3 shipped (substrate ready for M9)

- **AnimaCustody trait** locked at `crates/anima/anima-identity/src/custody.rs` with 6 backend variants (`InProcess`, `Vault`, `Tpm`, `Soma`, `HardwareWallet`, `WebCrypto`, `Remote`)
- **Browser-side TS SDK** at `sdks/life-sdk-ts/src/anima/`:
  - `WebCryptoAnima` composition wrapper (auth = passkey, wallet = remote)
  - `PasskeyOracle` (WebAuthn `navigator.credentials.create/get` + COSE_Key parser + IndexedDB cache)
  - `RemoteAnimaClient` (fetch wrapper for `/anima/custody/*`)
  - `SessionCap` (15-min Tier-User cap lifecycle)
  - `did:key:zDn…` derivation (multicodec `0x1200`, byte-identical to Rust)
- **Rust-side bridge** at `crates/anima/anima-identity/src/remote.rs`:
  - `RemoteAnima` for non-browser callers (CLIs, agents, native apps)
- **Server-side gateway** at `crates/life-runtime/lifegw/src/services/anima_custody.rs`:
  - 6 HTTP/JSON routes — `sign_auth`, `sign_wallet`, `get_auth_pubkey/{user_id}`, `get_wallet_pubkey/{user_id}`, `mint_session_cap`, `enroll_passkey`
  - `TierUserMinter` — sibling of Tier2Minter, `aud="anima.user-cap"`, 15-min TTL
  - `JwksCache::verify_capability_token` — multi-audience verifier with sub-binding + scope intersection
  - WS bearer subprotocol (`Sec-WebSocket-Protocol: bearer.<jwt>`) — closes M8.2

## M9 scope (this plan)

Migrate `apps/chatOS` to use AnimaCustody for browser users + server-side anima daemon for wallet operations, settling on Base testnet via real Vault.

### Sub-phases

#### M9.1 — chatOS SDK integration (~3 days)
- Add `@broomva/life-sdk` (npm name TBD or `file:../../core/life/sdks/life-sdk-ts` for monorepo dev) as dep in `apps/chatOS/apps/web` + `apps/chatOS/apps/bot`
- Add `packages/anima` (or extend `packages/auth`) with chatOS-specific WebCryptoAnima factory + SessionCap factory
- Wire `client = createLifeClient({ baseUrl, anima: { ... } })` from SDK index
- Smoke test: instantiate WebCryptoAnima in jsdom + verify enrollment + signing flow ends in mocked lifegw

#### M9.2 — Passkey enrollment UI (~4 days)
- New `/account/security/passkey` settings page in `apps/chatOS/apps/web`
- States: not-enrolled, enrolling (passkey prompt), enrolled (DID + wallet address), error
- Use shadcn/ui dialog + form patterns
- Hook into existing `packages/auth` Better Auth flow (passkey enrollment is a post-signin step; user must already be logged in)
- Show DID + wallet address + custody backend kind in settings UI
- Provide "view rotation history" link → reads journal events via lifegw `/v1/events/stream` (Lago anima.identity_rotated events)

#### M9.3 — chatOS sign-in via Tier-User cap (~3 days)
- After passkey enrollment, every sign-in mints a Tier-User cap via `mint_session_cap`
- Pass the cap as `Authorization: Bearer <jwt>` on `/anima/custody/*` calls
- Browser WS upgrades use `Sec-WebSocket-Protocol: bearer.<tier1-token>` (Tier-1 only — Tier-User caps are HTTP-only per Spec D)
- Persist enrollment state via IndexedDB (already handled by PasskeyOracle); re-mint cap on tab open if cached cap expired

#### M9.4 — server-side anima daemon (~5 days)
- Stand up production lifegw with `anima_custody` config block enabled
- Stand up production soma with `admin_plane` config block (custody-oracle UDS `/run/life/soma-admin.sock`)
- Stand up production Vault with secp256k1 transit key sidecar (Vault v1.15 doesn't support secp256k1 natively; either patches or HSM sidecar required — see D-Sub-B follow-up)
- Wire end-to-end: chatOS → lifegw → soma → Vault
- Operational runbook: provisioning new users, rotating keys, revoking compromised caps

#### M9.5 — Live USDC e2e on Base testnet (~3 days)
- Test wallet pre-funded with Base testnet USDC via faucet
- chatOS UI: "Send USDC" form with recipient + amount
- Sign EIP-3009 TransferWithAuthorization via WebCryptoAnima → RemoteAnimaClient → lifegw → soma → Vault → returned signature
- Broadcast to Base testnet RPC; assert receipt
- Acceptance test (Playwright e2e against staged lifegw + Base testnet)

#### M9.6 — Mission-control desktop pairing (optional, ~3 days)
- Mission-control desktop: detect TPM availability + Ledger presence
- If both present: pair `TpmAnima` (auth) + `HardwareWalletAnima` (wallet)
- If TPM only: `TpmAnima` (auth) + `RemoteAnima` (wallet via lifegw)
- If neither: `InProcessAnima` with file-based key (warn user)
- Settings UI: "custody backends" panel showing active config

#### M9.7 — broomva.tech AAP verifier coordination (cross-repo, ~2 days)
- broomva.tech adopts `lago-auth::verify_jwt` shape (or thin HTTP wrapper)
- Multi-curve verifier: ES256 (P-256) primary, EdDSA (Ed25519) legacy fallback
- Walks rotation chain via JournalResolver
- Acceptance: a JWT signed by an old (rotated) key is rejected for post-rotation timestamps; accepted for pre-rotation timestamps (matches the timestamp at the time of issuance)

### Total estimated effort

~21 days of implementation + integration + e2e validation. Parallelizable across:
- M9.1 + M9.2 (frontend track)
- M9.4 (infra track)
- M9.5 (e2e validation track, after M9.4)
- M9.6 (desktop track, parallel)
- M9.7 (cross-repo track, parallel)

## Acceptance criteria (M9 done)

1. ✅ chatOS browser user can enroll a passkey and see their DID + wallet address in settings
2. ✅ chatOS browser user can sign a JWS for the Agent Auth Protocol via passkey
3. ✅ chatOS browser user can initiate a USDC transfer; signature comes from server-side Vault; receipt confirmed on Base testnet
4. ✅ Mission-control desktop user can use TPM-auth + Ledger-wallet pairing for the strongest custody story
5. ✅ broomva.tech accepts P-256 / ES256 JWTs via the canonical verifier path
6. ✅ Rotation event end-to-end: rotate from old DID, journal event written, downstream verifier accepts new DID for new tokens, accepts old DID for pre-rotation tokens

## Tracking

- Linear epic: `BRO-XXX: M9 Anima Custody Apps Migration` (placeholder; user files when MCP re-auths)
- Sub-tickets: M9.1 .. M9.7 mapped 1:1
- Status will be tracked in `docs/STATUS.md` § "M9 progress"

## Open questions

1. **`@broomva/life-sdk` npm publish vs monorepo file: dep**: chatOS lives in a separate workspace. Do we publish `life-sdk-ts` to npm under `@broomva/life-sdk` (production-grade), or use a `file:../../core/life/sdks/life-sdk-ts` workspace dep (faster iteration, no publish dance)? Recommendation: file: dep during M9 dev; publish for M10 launch.

2. **Passkey portability and recovery**: Spec D §"Browser path" §4 puts portability on the OS (iCloud Keychain, Google Password Manager, BitWarden). What's the user-facing message when they sign in from a fresh device with no passkey? Options: (a) require re-enrollment via existing-device-issued rotation event; (b) require account recovery via email + KYC; (c) cross-device passkey sync via FIDO2 conditional UI. Lock in a UX choice early.

3. **Tier-User cap revocation under fresh device**: if a user signs in on a phone, then loses the phone, the active 15-min Tier-User cap is still valid until expiry. Should chatOS provide a "revoke active sessions" button that publishes `anima.identity_revoked` events? Yes — this should ship in M9.2.

4. **Vault sidecar cost/operations**: secp256k1 transit isn't native to Vault v1.15. Operational options: (a) HashiCorp Vault Enterprise w/ pluggable transit (cost), (b) HSM sidecar via PKCS#11 (cost + complexity), (c) wait for upstream Vault support. Recommendation: spike on (b) with softhsm in dev; revisit for production.

5. **chatOS / Sentinel / Life-Module-tenant deployment story**: M9 ships the chatOS path. How do Sentinel and Life-Module tenants get the same custody story without a chatOS-shaped UI? Likely via a shared `packages/anima-react` library and per-tenant theming. Filed as M10 work.

## Out of scope for M9

- Full FIDO2 attestation chain verification (D-Sub-C R-2 follow-up)
- WebHID hardware wallet for browser (D-Sub-F follow-up)
- Generic EIP-712 encoder (D-Sub-A/B/C/D/E/F shared follow-up)
- Live softhsm CI fixture for D-Sub-D
- Live Ledger e2e for D-Sub-F
- Connection pooling for lifegw → soma admin UDS (D-Sub-C R-2 follow-up)

## Dependencies on external infrastructure

- Production lifegw (already running for Tier-1 Vercel auth; needs `anima_custody` config block enabled)
- Production soma with admin plane (currently dev-only)
- Production Vault with secp256k1 (gap — see open question 4)
- Base testnet RPC endpoint (Coinbase / Alchemy / public)
- USDC testnet faucet on Base
- Ledger Live or Ledger device (for M9.6)
- TPM-equipped Linux host (for M9.6)
