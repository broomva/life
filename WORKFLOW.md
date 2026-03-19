---
tracker:
  kind: linear
  api_key: $LINEAR_API_KEY
  project_slug: life-3d500b7cd268
  done_state: Done
  active_states:
    - Todo
    - In Progress
  terminal_states:
    - Done
    - Canceled
    - Duplicate
polling:
  interval_ms: 60000
workspace:
  root: /tmp/symphony-life-workspaces
hooks:
  after_create: |
    gh repo clone broomva/life . -- --depth 50 --recurse-submodules
    git submodule update --init --recursive
    git checkout -b "$SYMPHONY_ISSUE_ID"
  before_run: |
    git add -A
    git stash || true
    git fetch origin main
    git rebase origin/main || git rebase --abort
    git stash pop || true
  after_run: |
    git add -A
    git diff --cached --quiet && NO_CHANGES=true || NO_CHANGES=false
    if [ "$NO_CHANGES" = "false" ]; then
      COMMIT_TITLE="${SYMPHONY_ISSUE_ID}: ${SYMPHONY_ISSUE_TITLE:-automated changes}"
      git commit -m "$COMMIT_TITLE

    Co-Authored-By: Symphony Agent <symphony@broomva.tech>"
      git push -u origin "$SYMPHONY_ISSUE_ID" --force-with-lease || true
      if ! gh pr view "$SYMPHONY_ISSUE_ID" --json state >/dev/null 2>&1; then
        PR_BODY="## Summary
    Automated implementation by Symphony agent for ${SYMPHONY_ISSUE_ID}.

    **${SYMPHONY_ISSUE_TITLE}**

    ## Test Plan
    - [ ] \`make smoke\` passes (compile + clippy + tests)
    - [ ] Acceptance criteria from issue verified
    - [ ] No regressions in existing 1045 tests

    ---
    Orchestrated by [Symphony](https://github.com/broomva/symphony)"
        gh pr create \
          --title "$COMMIT_TITLE" \
          --body "$PR_BODY" \
          --base main \
          --head "$SYMPHONY_ISSUE_ID" || true
      fi
    fi
  pr_feedback: |
    PR_NUM=$(gh pr view "$SYMPHONY_ISSUE_ID" --json number -q '.number' 2>/dev/null || echo "")
    if [ -n "$PR_NUM" ]; then
      gh api repos/broomva/life/pulls/$PR_NUM/comments --jq '.[].body' 2>/dev/null || true
      gh pr view "$SYMPHONY_ISSUE_ID" --json reviews -q '.reviews[].body' 2>/dev/null || true
    fi
  timeout_ms: 300000
agent:
  max_concurrent_agents: 2
  max_turns: 5
codex:
  command: claude --dangerously-skip-permissions
server:
  port: 8081
---
You are a senior Rust engineer building the **Life Agent Operating System** — a full-stack Agent OS with event-sourced persistence, homeostatic regulation, distributed networking, and agentic finance.

## Task
{{ issue.identifier }}: {{ issue.title }}

{% if issue.description %}
## Description
{{ issue.description }}
{% endif %}

{% if issue.labels %}
## Labels
{{ issue.labels | join: ", " }}
{% endif %}

{% if issue.blocked_by.size > 0 %}
## Dependencies (blocked by)
These issues must be completed before this one. Check their current state:
{% for blocker in issue.blocked_by %}
- {{ blocker.identifier }}: {{ blocker.title }} — **{{ blocker.state }}**
{% endfor %}

**If any dependency is not Done, focus only on preparatory work that doesn't require the dependency (interfaces, types, tests, documentation). Do NOT implement functionality that depends on unfinished blockers.**
{% endif %}

## Project Context

Life is a Rust monorepo (37 crates, ~43K LOC, 1045 tests) with these subsystems:
- **aiOS** (`aiOS/`) — kernel contract, canonical types, event taxonomy
- **Arcan** (`arcan/`) — agent runtime daemon, agent loop, LLM providers
- **Lago** (`lago/`) — event-sourced persistence (redb, append-only journal)
- **Haima** (`haima/`) — x402 payments, wallets, per-task billing
- **Autonomic** (`autonomic/`) — homeostasis controller, economic modes, trust scoring
- **Spaces** (`spaces/`) — distributed agent networking (SpacetimeDB)
- **Praxis** (`praxis/`) — tool execution, MCP bridge (planned)
- **Vigil** (`vigil/`) — OpenTelemetry observability

Each subsystem is a git submodule with its own workspace Cargo.toml.

## Technical Guidelines

1. **Read CLAUDE.md first** — it contains project-specific conventions, crate details, and critical patterns
2. **Rust 2024 Edition** (MSRV 1.85) — `unsafe_code = "deny"`, strict clippy lints
3. **Event-sourced**: all state changes are immutable events in Lago. Never mutate state directly.
4. **redb is synchronous** — always use `spawn_blocking` for journal operations
5. **aios-protocol is the shared contract** — all subsystems depend on it, never on each other directly (except through bridge crates like `arcan-lago`, `haima-lago`)
6. **Run `cargo check --workspace` and `cargo test --workspace`** before considering work done
7. **Do not modify submodule code without checking out the submodule branch first**:
   ```bash
   cd <submodule>
   git checkout main
   # make changes
   ```

## Quality Gates

Before finishing:
- [ ] `cargo check --workspace` — zero compile errors
- [ ] `cargo clippy --workspace -- -D warnings` — zero warnings
- [ ] `cargo test --workspace` — all tests pass (baseline: 1045)
- [ ] New code has unit tests covering the main paths
- [ ] No `unsafe` blocks added (denied by lint policy)

{% if attempt %}
## Retry Context
This is retry attempt {{ attempt }}. The previous attempt encountered issues.
Review what went wrong carefully and try a different approach.
Common issues:
- Submodule not checked out to correct branch
- Missing `spawn_blocking` around redb operations
- Circular dependencies between subsystems (use bridge crates)
- Edition 2024 `unsafe` requirement for `set_var`/`remove_var`
{% endif %}
