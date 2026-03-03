# Commands

## Arcan (run from `/Users/broomva/broomva.tech/life/arcan`)

```bash
cargo build --workspace
cargo check
cargo test --workspace
cargo clippy --workspace
cargo fmt
cargo run -p arcan
ANTHROPIC_API_KEY=... cargo run -p arcan
```

Focused tests:

```bash
cargo test -p arcan-lago
cargo test -p arcand -- --nocapture
```

## Lago (run from `/Users/broomva/broomva.tech/life/lago`)

```bash
cargo build --workspace
cargo check
cargo test --workspace
cargo clippy --workspace
cargo fmt
```

Focused tests:

```bash
cargo test -p lago-journal
cargo test -p lago-api --test e2e_files
```

## Cross-Project Validation (run from `/Users/broomva/broomva.tech/life`)

```bash
(cd arcan && cargo fmt && cargo clippy --workspace && cargo test --workspace) && \
(cd lago && cargo fmt && cargo clippy --workspace && cargo test --workspace)
```

## Pre-Commit Sequence

```bash
cargo fmt
cargo check
cargo clippy --workspace
cargo test --workspace
```

For larger changes, add:

```bash
cargo build --workspace
```
