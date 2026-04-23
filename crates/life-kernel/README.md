# life-kernel

Implementation cluster for the aiOS kernel contract at the µVM isolation tier.

In Phase 0, this cluster contains only the `life-kernel-conformance` scaffold
crate so the workspace resolves and downstream tooling works. Phase 1 adds
`life-kernel-proto`, `life-kernel-core`, and `life-kernel-gate`; Phase 2 adds
the `lifed` daemon binary.

See [CLAUDE.md](./CLAUDE.md) for the cluster overview and dependency rules,
and the kernel daemon design spec at
`docs/superpowers/specs/2026-04-23-lifed-kernel-daemon-design.md` for the
full motivation and roadmap.
