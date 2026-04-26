# lifed

The Life Runtime facade-aggregator daemon. Tonic-over-UDS public + admin planes; mounts `life.v1.{Agent, Events, Wallet, Identity}` and `life.admin.v1.{Runtime, Saga, RoutingCache}`. Forwards every RPC to the appropriate substrate (`arcan`, `lago`, `haima`, `anima`, `soma`). Stateless except for an in-memory routing cache rebuildable from `lago`. Hosts saga orchestration for cross-substrate writes (`Agent.CreateSession`).

- **Spec:** `docs/superpowers/specs/2026-04-26-spec-c2-lifed-facade.md` (design ground truth)
- **Master spec:** `docs/superpowers/specs/2026-04-25-life-runtime-architecture-spec.md` §L0–§L14
- **Implementation plan:** `docs/superpowers/plans/2026-04-26-m5-lifed-build.md`
- **Linear:** [BRO-930](https://linear.app/broomva/issue/BRO-930)

## Run (after sub-phase A merges)

```bash
cargo run -p lifed -- daemon --config /etc/life/lifed.toml
```

## Operator CLI

```bash
lifed sessions ls
lifed sessions show <sid>
lifed routing-cache dump
lifed saga show <saga_id>
```

All operator subcommands speak the admin-plane tonic surface on `/run/life/life-admin.sock`. Caller must be in the `life-admin` group.
