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
surface**. So the set is curated and reviewed, not auto-synced from developer
skill directories.

**These are NOT the developer/build skills** under `.agents/skills/`
(control-metalayer-loop, harness-engineering-playbook, check, release-check).
Those exist to build *Life itself* and must never reach a chat user.

## The blessed tool palette

The blessed base toolset — approved by operator sign-off (BRO-1469) — is:

```
bash · read_file · write_file · edit_file · grep · glob · list_dir
```

i.e. **shell, read/write, and search**. Skills and other custom tooling
**compose on top of** this base. The invariant, enforced by a regression test
in `crates/arcan/arcan/src/skills.rs`, is:

> Every blessed skill's `allowed_tools` must be a **subset of the palette**.

A skill declaring a tool *outside* the palette (network egress, secrets, an
unreviewed MCP server, …) is a deliberate capability expansion — it must NOT
land here without extending the palette and re-signing off.

### Two layers keep this safe

1. **The palette (this repo)** bounds what any blessed skill may ask for.
2. **The tier gate** (`arcand/src/canonical.rs`) bounds who may actually use it.
   A skill is admitted to a tier's catalog only when its `allowed_tools` are a
   subset of that tier's safe set, and an active skill's tools are further
   *intersected* with the tier at execution time (more-restrictive wins):
   - **anonymous** — grants zero capabilities; conversation only.
   - **free** — read + search (`read_file`, `list_dir`, `glob`, `grep`); no
     writes, no shell.
   - **pro / enterprise** — the full palette.

   So `write_file` / `bash` skills are effectively pro-tier; read/search skills
   reach free; and a skill with `allowed_tools: []` is available to everyone.
   The palette is the ceiling; the tier is the floor.

## The current blessed set

| Skill | Purpose | Declared tools |
|---|---|---|
| `getting-started` | Onboards a new chat user; can peek at the workspace to orient | `read_file`, `list_dir`, `glob` |
| `explain` | Explains a concept / error / code, grounded in real workspace files | `read_file`, `grep`, `glob`, `list_dir` |
| `summarize` | Faithful TL;DR + key points + action items from pasted text or a file | `read_file`, `glob`, `list_dir` |
| `brainstorm` | Generates & pressure-tests options with honest tradeoffs | *(none)* |
| `workspace` | The general working agent — read/search/edit files + run shell | `bash`, `read_file`, `write_file`, `edit_file`, `grep`, `glob`, `list_dir` |

All are `user_invocable: true`, and every declared tool is within the palette.

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

1. Add `runtime-skills/<name>/SKILL.md` with `allowed_tools` ⊆ the palette.
2. Keep it genuinely useful and safe on a public surface — no internal/dev
   concerns, no data exfiltration prompts, no jailbreak bait.
3. Update the table above and the count/palette in the `skills.rs` regression
   test.
4. Get operator sign-off in the PR — this is a public-surface capability change.
   Widening the palette itself (a new tool category) always requires sign-off.
