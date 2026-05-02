# Life Agent OS — Language SDKs

Multi-language SDK wrappers for the Life Agent OS Rust crates.

## Packages

- **life-sdk-ts** (`@broomva/life-sdk`) — Public TypeScript SDK over `lifegw`. Implements the four user-facing services (`life.v1.{Agent, Events, Wallet, Identity}`) over `grpc-web` + WebSocket. Browsers, Node, and CLIs talk to the Life Agent OS through this client.
- **haima-py** — Python SDK for x402 payments, wallet management, and framework integrations (LangChain, CrewAI)
- **haima-ts** — TypeScript SDK for x402 payments with ElizaOS and OpenAI integrations
