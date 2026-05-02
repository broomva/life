# Life Agent OS — Language SDKs

## Overview

Public-facing client libraries for the Life Agent OS Rust monorepo.
Two product areas live here:

1. **`life-sdk-ts`** — public TypeScript SDK over `lifegw` (M8). Browsers,
   Node apps, and CLIs talk to the Life Agent OS through this client.
   Wraps the four user-facing services from `life.v1.*` (Agent, Events,
   Wallet, Identity) plus the WebSocket transport for `Agent.StreamSession`.
2. **`haima-py` + `haima-ts`** (BRO-42) — multi-framework SDKs for x402
   payments through the Haima facilitator. The Stripe Connect play.

## Structure

```
sdks/
├── life-sdk-ts/       # Public Life Agent OS SDK (npm: @broomva/life-sdk) — M8
│   ├── src/           # client, transport, ws, errors
│   │   ├── proto/     # Hand-curated TS mirror of proto/life/v1/*
│   │   └── services/  # AgentClient, EventsClient, WalletClient, IdentityClient
│   ├── examples/      # quickstart, streaming
│   └── tests/         # vitest — 49 tests across 6 files
│
├── haima-py/          # Python SDK (PyPI: haima)
│   ├── src/haima/     # Core: client, wallet, types, x402
│   │   └── integrations/  # LangChain, CrewAI
│   ├── examples/      # Working agent examples
│   └── tests/
│
├── haima-ts/          # TypeScript SDK (npm: @haima/sdk)
│   ├── src/           # Core: client, wallet, types, x402
│   │   └── integrations/  # ElizaOS, OpenAI Agents
│   ├── examples/      # Working agent examples
│   └── tests/
│
└── CLAUDE.md          # This file
```

## Architecture

Both SDKs wrap the Haima facilitator HTTP API:
- `POST /v1/facilitate` — submit x402 payment
- `GET /v1/facilitator/stats` — dashboard stats
- `GET /v1/credit/{agent_id}` — credit score
- `POST /v1/credit/{agent_id}/check` — spend check
- `GET /health` — health check

## Key Types (from haima-core)

- **micro-credits**: 1 USDC = 1,000,000 μc
- **ChainId**: CAIP-2 format (e.g., `eip155:8453` for Base)
- **x402 headers**: base64-encoded JSON (PAYMENT-REQUIRED, PAYMENT-SIGNATURE, PAYMENT-RESPONSE)
- **PaymentPolicy**: auto_approve_cap (100 μc), hard_cap_per_tx (1M μc), session_spend_cap (10M μc)

## Conventions

- Python: ruff for linting, pytest for tests
- TypeScript: Bun for runtime, vitest for tests
- Both SDKs target the same facilitator API
- Wallet: secp256k1 (eth_account for Python, viem for TypeScript)
