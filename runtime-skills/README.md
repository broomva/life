# runtime-skills/ — Blessed Runtime Skill Set (BRO-1469)

This directory is the **blessed set of runtime skills** baked into the arcan
container images and served on the public chat surface (anonymous / free / pro
tiers behind `lifegw`). It is the runtime analog of `agents/`: a curated,
in-repo, human-reviewed asset shipped in the image so the daemon discovers a
known set at boot instead of `skills_found=0`.

Each `runtime-skills/<name>/SKILL.md` is a Claude-Code-style skill: YAML
frontmatter + a markdown body. Arcan's `SkillRegistry` (praxis-skills) walks
this directory at boot when `--skills-dir` / `ARCAN_SKILLS_DIR` points here.

## Why a *blessed* set (the capability decision)

The runtime skill set shapes agent behavior on a **public, unauthenticated
surface**. Overshipping is not free — a skill that declares tools it can invoke
becomes new attack surface for whoever can reach the chat endpoint. So the set
is curated and reviewed, not auto-synced from developer skill directories.

**These are NOT the developer/build skills** under `.agents/skills/`
(control-metalayer-loop, harness-engineering-playbook, check, release-check).
Those exist to build *Life itself* and must never reach a chat user.

## The zero-tool invariant

Every skill in this directory declares `allowed_tools: []`.

This is a hard safety property, enforced by a regression test in
`crates/arcan/arcan/src/skills.rs`:

- A skill with `allowed_tools: []` requests **no tool capability**. It is pure
  behavioral guidance (a prompt) injected into the system prompt.
- Arcan's tier gate (`arcand/src/canonical.rs`) admits a skill to a tier's
  catalog only when its `allowed_tools` are a subset of that tier's safe set.
  The **anonymous tier grants zero capabilities**, so only zero-tool skills are
  ever visible or activatable there. `allowed_tools: []` satisfies every tier
  (the empty set is a subset of every set), so these skills are safe at
  anonymous / free / pro alike while adding **no** new execution surface.

Adding a skill that needs tools is a **deliberate capability expansion**: it
requires bumping the tier that will see it, documenting the tools, and operator
sign-off in the PR — not a silent drop-in here.

## The current blessed set

| Skill | Purpose | Tools |
|---|---|---|
| `getting-started` | Onboards a new chat user; explains what Life can do and how to interact | none |
| `explain` | Explains a concept, error, or pasted code clearly and pedagogically | none |
| `summarize` | Condenses pasted text into a faithful TL;DR + key points + action items | none |
| `brainstorm` | Generates and pressure-tests options for a decision, with honest tradeoffs | none |

All four are prompt-only, `user_invocable: true`, and safe on the anonymous
surface.

## How the image ships it

- **`deploy/railway/lifegw-stack/Dockerfile`** copies `runtime-skills/` to
  `/opt/life/skills/`; `entrypoint.sh` passes `--skills-dir /opt/life/skills`
  to `arcan serve` (alongside `--agents-dir /opt/life/agents`).
- **`docker/arcan.Dockerfile`** copies `runtime-skills/` to `/home/arcan/skills/`
  and sets `ENV ARCAN_SKILLS_DIR=/home/arcan/skills`.

`--skills-dir` (env `ARCAN_SKILLS_DIR`) is scanned **first**, ahead of the
config defaults (`.arcan/skills`, `.agents/skills`, `~/.agents/skills`), so
discovery does not depend on the process CWD/HOME — the exact fragility that
produced `skills_found=0` in prod (the boot log showed `/root/.agents/skills`
because `~` resolved to root under `runuser`).

## Adding or changing a skill

1. Add `runtime-skills/<name>/SKILL.md` with `allowed_tools: []`.
2. Keep it genuinely useful and safe on a public surface — no internal/dev
   concerns, no data exfiltration prompts, no jailbreak bait.
3. Update the table above and the count in the `skills.rs` regression test.
4. Get operator sign-off in the PR — this is a public-surface capability change.
