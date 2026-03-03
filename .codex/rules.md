# Rules

## Core Engineering Rules

- Run commands from the correct project directory (`arcan/` or `lago/`).
- Keep Rust formatting clean with `cargo fmt`.
- Treat all clippy warnings as actionable (`cargo clippy --workspace`).
- Ensure code compiles (`cargo check`) and tests pass (`cargo test --workspace`).
- Add tests for all new behavior.
- Prefer safe Rust; avoid `unsafe` unless unavoidable and justified.
- Use `thiserror` in library crates and `anyhow` in binary crates.
- Never commit secrets or `.env` files; use environment variables.

## Rust 2024 Rules

- Do not use `gen` as an identifier (reserved keyword).
- Wrap `std::env::set_var` and `std::env::remove_var` in `unsafe {}` when used.
- Prefer native `async fn` in traits; use boxed futures only when dyn compatibility is required.
- Use file-based module layout (`name.rs`) rather than `mod.rs`.

## Architecture Rules

- Keep event sourcing intact: do not mutate or rewrite historical events.
- Preserve replay determinism: state reconstruction must match original run behavior.
- In Lago journal code, execute redb operations via `spawn_blocking`.
- For dyn journal interfaces, keep `BoxFuture` usage consistent.
- In redb table access code, import required traits (for example `redb::ReadableTable`) when using table iterator/get/range APIs.

## Process Rules

- After every feature/fix, update documentation status when applicable:
  - `/Users/broomva/broomva.tech/life/docs/STATUS.md`
  - `/Users/broomva/broomva.tech/life/docs/ARCHITECTURE.md`
  - `/Users/broomva/broomva.tech/life/docs/ROADMAP.md`
  - relevant `CLAUDE.md` or `.claude/rules/` guidance
- Keep documentation and implementation in sync in the same change window.
