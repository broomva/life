# life-kernel-proto

Wire contract for the soma kernel daemon (formerly `lifed`; renamed
2026-04-25 per Spec C M0) — generated Rust stubs for
`KernelPort`. The `pb` module ships the prost-generated message types and
the tonic-generated `KernelService` server/client stubs; `convert` bridges
those generated types to the canonical `aios_protocol` types consumed
elsewhere in the workspace.

## Fuzzing

The crate ships a `cargo-fuzz` smoke target that feeds arbitrary bytes into
the `VmSpec` proto decoder:

```bash
cargo install cargo-fuzz  # one-time
cargo fuzz run parse_vm_spec -- -max_total_time=60
```

A 30-second smoke run is sufficient for CI; longer runs (nightly) are
appropriate for continuous fuzz coverage. Corpus and artifacts are gitignored
under `fuzz/corpus/` and `fuzz/artifacts/`.

### Extending

Additional targets can exercise other decoders — `pb::VmHandle`,
`pb::ToolResult`, etc. — via the same `Message::decode` entry point.
