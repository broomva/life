# x402 live base-sepolia round-trip (BRO-1365 #2)

Reproducible harness that drives haimad's **real `X402Pay` handler** through a
full x402 payment that **settles on-chain on Base Sepolia** — confirming the
slice-2 transport rail (BRO-1354) end-to-end against live infrastructure.

The rail (`haimad::substrate::SubstrateService::x402_pay`) is exercised
unmodified: resolve custody → `CustodyWalletAdapter` (EIP-3009 signing from the
user's secp256k1 wallet) → `X402Client` → `pay_x402`. The only non-production
pieces are the **seller** (a tiny resource server) and the **relayer** (the
gas-payer that submits the EIP-3009 `transferWithAuthorization`), because
haima's own facilitator does not yet broadcast on-chain (F4 gap).

## Confirmed run (2026-06-03, Base Sepolia)

| | |
|---|---|
| Settlement tx | [`0xc94a975375dcf6678d330eddd5150e044d0b2a45862cb1c590d776f44f23023a`](https://sepolia.basescan.org/tx/0xc94a975375dcf6678d330eddd5150e044d0b2a45862cb1c590d776f44f23023a) |
| Status / block | `1` (success) / `42379188` |
| USDC contract | `0x036CbD53842c5426634e7929541eC2318f3dCF7e` (Circle FiatTokenV2) |
| Transfer | `0x6b9ca44686d5d7ea0e6e019767c40cc03e81a2ba` (rail wallet) → `0x389b6a704d3b34688863def723b3890453b53aee` (recipient), value `100` (0.0001 USDC) |
| Payer USDC | `1000000` → `999900` (−100) · Recipient USDC: `100` |
| Rail handler result | `status="settled"`, `settled=true`, `tx_hash=0xc94a…23023a` |

The payer wallet was funded autonomously via the **CDP faucet** (programmatic,
API-key-authenticated) — the public web faucets are all login/captcha-gated.

## Architecture

```text
haimad X402Pay (rail, unmodified)            seller (this dir)           Base Sepolia
  resolve_custody(user,project)                                          USDC FiatTokenV2
  → InProcessAnima (deterministic)
  → CustodyWalletAdapter.sign_eip712  ──X-PAYMENT (payment-signature)──►  decode auth + sig
  pay_x402: GET → 402 → policy → sign → retry                            relayer submits
                                          ◄──payment-response (tx)──────  transferWithAuthorization
                                                                          → real on-chain transfer
```

## Reproduce

Prereqs: a CDP API key JSON (`{ "id", "privateKey" }`); Python venv with
`cdp-sdk` + `web3` + `eth-account`; the workspace built.

```bash
# 0. venv
python3 -m venv .venv && . .venv/bin/activate && pip install cdp-sdk web3 eth-account

# 1. compute the rail's deterministic signing wallet for (user, project)
cargo test -p haimad print_live_wallet_address -- --ignored --nocapture
#   → X402_LIVE_WALLET_ADDRESS=0x...

# 2. fund it with base-sepolia USDC (+ ETH) via the CDP faucet
CDP_KEY_FILE="/path/to/CDP API Key.json" FUND_ADDRESS=0x<rail-wallet> \
  python3 scripts/x402-live/fund.py

# 3. generate + ETH-fund a relayer, then run the seller (settles on-chain)
#    (the seller reads /tmp/x402-live/relayer.key — a 0600 EOA key; see seller.py header)
RECIPIENT=0x<recipient> AMOUNT=100 PORT=8402 python3 scripts/x402-live/seller.py &

# 4. drive the rail's real handler against the seller
X402_LIVE_RESOURCE_URL=http://127.0.0.1:8402/paid X402_LIVE_USER=x402-live \
  X402_LIVE_PROJECT=base-sepolia \
  cargo test -p haimad live_base_sepolia_roundtrip -- --ignored --nocapture
#   → LIVE_X402_STATUS=settled, LIVE_X402_TX=0x...
```

## Security

- The CDP API key is **never** committed or echoed; `fund.py` reads it by
  `CDP_KEY_FILE` path.
- The relayer EOA key is generated locally to a `0600` file outside the repo.
- AMOUNT defaults to `100` atomic units (0.0001 USDC) — ≤ haima's 100-µc
  auto-approve cap, so the rail's `PaymentPolicy::default()` auto-approves.

## base-sepolia only

`network="base"` (mainnet) is rejected by the rail with `failed_precondition`
(BRO-1354). Mainnet is slice 3, behind the financial control gate.
