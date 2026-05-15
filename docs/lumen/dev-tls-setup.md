# Lumen dev-TLS — local CA + SAN leaf setup

**One-time per developer machine.** Establishes a `Lumen Dev CA` in
the system trust store so strict-verifying TLS clients (Lumen,
browsers, curl-without-`-k`, future SDKs) accept HTTPS connections
to `https://127.0.0.1:8443` without per-client bypasses.

This is the **best-practice** local-dev TLS pattern (same model
`mkcert` uses): trust a local CA once, sign all dev leaf certs with
it, no `danger_accept_invalid_certs` trapdoors anywhere.

## Why we replaced the prior self-signed cert

The previous `crates/life-runtime/lifegw/dev-tls/cert.pem` was:

- self-signed (no CA chain)
- CN-only (no `subjectAltName` extension)

Modern strict verifiers (RFC 2818 / RFC 6125, every browser, reqwest
with rustls, macOS Security framework) reject CN-only certs —
they require a SAN entry naming the host. Result: every Lumen attempt
to fetch `https://127.0.0.1:8443/...` failed with `errSecNotTrusted`
(-67843), and the Layer 2-GUI run dead-ended without any GraphQL
traffic ever reaching lifegw.

The new pair fixes both: signed by our local CA + SAN includes
`DNS:localhost`, `DNS:lumen.local`, `IP:127.0.0.1`, `IP:::1`.

## Setup (run once per machine)

### 1. Generate the CA + leaf

```bash
./scripts/dev-tls-bootstrap.sh
```

Idempotent — re-running reuses the existing `dev-ca.pem` if present
and only refreshes the leaf. Produces:

- `crates/life-runtime/lifegw/dev-tls/dev-ca.pem` (CA cert, 10-year)
- `crates/life-runtime/lifegw/dev-tls/dev-ca-key.pem` (CA key — dev fixture)
- `crates/life-runtime/lifegw/dev-tls/cert.pem` (server leaf, 825-day)
- `crates/life-runtime/lifegw/dev-tls/key.pem` (server leaf key)

The leaf chains cleanly to the CA — the script verifies this with
`openssl verify` before exiting.

### 2. Trust the CA system-wide (the one sudo command)

```bash
sudo security add-trusted-cert -d -r trustRoot \
  -k /Library/Keychains/System.keychain \
  crates/life-runtime/lifegw/dev-tls/dev-ca.pem
```

Adds the `Lumen Dev CA` to the system trust store. After this,
every TLS client on your Mac that consults the system trust pool
will accept any leaf cert signed by this CA — including Lumen.

Verify:

```bash
security find-certificate -c "Lumen Dev CA" /Library/Keychains/System.keychain
```

### 3. Restart lifegw (only needed if it was running)

```bash
./scripts/lumen-smoke.sh down
./scripts/lumen-smoke.sh up
```

lifegw picks up the new cert on bootstrap.

### 4. Verify

```bash
curl https://127.0.0.1:8443/_health   # no -k flag needed
echo | openssl s_client -connect 127.0.0.1:8443 -servername localhost 2>&1 \
  | grep "Verify return code"
# Expect: Verify return code: 0 (ok)
```

## Threat surface

The CA private key (`dev-tls/dev-ca-key.pem`) lives in the repo
alongside the existing dev leaf key. Anyone with read access to the
repo + your Mac can forge certs your Mac trusts. **This is a dev-only
key — never deploy it.** The threat model is unchanged from the
previous self-signed leaf key already committed to the same dir.

When promoting Life beyond local dev (TLS termination on Vercel /
production lifegw), the production cert chain is a separate concern;
this dev CA is solely for the on-laptop loopback path.

## Removal

```bash
sudo security delete-certificate -c "Lumen Dev CA" \
  /Library/Keychains/System.keychain
```

## How this composes with the rest of the patch stack

The Lumen Phase α stack has four patches in `apps/warp-oss-patch/`:

1. `channel-oss-override.patch` — `WARP_SERVER_ROOT_URL` flows through on OSS
2. `skip-metal-when-wgpu.patch` — Metal-shader compile gated on feature
3. `skip-oss-login.patch` — bypasses the post-customize OAuth dead-end
4. `rename-to-lumen.patch` — full namespace separation (dev.broomva.Lumen)

The TLS setup here is the fifth gate-closer — **but it's not a patch**.
The warp-oss source already verifies TLS correctly; the problem was
the cert it was being asked to verify. Fixing the cert chain in dev
is the right scope for this concern; modifying upstream's TLS
verification logic would be a bandaid in the wrong layer.
