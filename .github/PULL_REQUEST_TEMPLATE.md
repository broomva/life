## What

<!-- One-sentence summary of the change -->

## Why

<!-- What problem does this solve? Link related issues with "Closes #123" -->

## How

<!-- Brief description of the approach. Call out anything non-obvious. -->

## Subsystem(s)

<!-- Check all that apply -->

- [ ] Arcan (agent runtime)
- [ ] Lago (persistence)
- [ ] aiOS (kernel contract)
- [ ] Autonomic (homeostasis)
- [ ] Haima (finance)
- [ ] Anima (identity)
- [ ] Nous (evaluation)
- [ ] Praxis (tool execution)
- [ ] Spaces (networking)
- [ ] Vigil (observability)
- [ ] CI / Build / Docs

## Checklist

- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace -- -D warnings` passes
- [ ] `cargo fmt --all` applied
- [ ] New public APIs have doc comments
- [ ] Architecture dependency rules maintained (no cross-subsystem internal imports)
