# Examples

Runnable examples for the Life Agent OS.

## basic_agent

Demonstrates the core primitives (Provider, Tool, ToolRegistry) without API keys.

```bash
cargo run -p arcan-core --example basic_agent
```

Source: [`crates/arcan/arcan-core/examples/basic_agent.rs`](../crates/arcan/arcan-core/examples/basic_agent.rs)

## Interactive Shell

For a full interactive experience with all tools:

```bash
cargo install arcan
arcan shell --provider mock
```
