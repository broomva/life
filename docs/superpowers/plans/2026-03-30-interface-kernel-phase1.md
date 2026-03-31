# Interface Kernel (IKR) Phase 1 — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the TypeScript foundation for the Life Interface Kernel — semantic UI IR, Pretext+Yoga layout kernel, constraint policy, AI repair loop, signal reactivity, and DOM+Terminal renderers.

**Architecture:** Monorepo workspace at `core/life/interface/` with 8 packages + 1 playground app. Bun as package manager, Biome for linting, TypeScript strict mode. Packages follow `@life/ikr-*` naming.

**Tech Stack:** TypeScript 5.x, Bun, Biome, @chenglou/pretext, yoga-layout, @preact/signals-core, ai (AI SDK v6), vitest

---

## Dependency Chain

```
Task 1: Scaffold workspace (foundation, blocks everything)
  ├── Task 2: ikr-ir (types, no internal deps) ─── parallelizable with Task 3
  ├── Task 3: ikr-signals (reactivity, no internal deps) ─── parallelizable with Task 2
  │
  ├── Task 4: ikr-layout (needs ikr-ir) ─── after Task 2
  │     ├── Task 5: ikr-policy (needs ikr-layout) ─── after Task 4
  │     │     └── Task 6: ikr-repair (needs ikr-policy + ai) ─── after Task 5
  │     └── Task 7: ikr-render-terminal (needs ikr-ir) ─── parallelizable with Task 5
  │
  ├── Task 8: ikr-render-dom (needs ikr-ir + ikr-signals) ─── after Task 2+3
  ├── Task 9: ikr umbrella package (needs all packages) ─── after Tasks 2-8
  └── Task 10: Playground app (needs ikr umbrella) ─── after Task 9
```

## Parallel Execution Opportunities

- **Wave 1**: Task 1 (scaffold) — sequential
- **Wave 2**: Task 2 (ir) + Task 3 (signals) — parallel agents
- **Wave 3**: Task 4 (layout) — sequential (needs ir)
- **Wave 4**: Task 5 (policy) + Task 7 (render-terminal) + Task 8 (render-dom) — parallel agents
- **Wave 5**: Task 6 (repair) — sequential (needs policy)
- **Wave 6**: Task 9 (umbrella) + Task 10 (playground) — sequential

---

### Task 1: Scaffold Workspace (BRO-295)

**Files:**
- Create: `interface/package.json`
- Create: `interface/tsconfig.json`
- Create: `interface/biome.json`
- Create: `interface/CLAUDE.md`
- Create: `interface/packages/ir/package.json`
- Create: `interface/packages/ir/tsconfig.json`
- Create: `interface/packages/ir/src/index.ts`
- Create: `interface/packages/layout/package.json`
- Create: `interface/packages/layout/tsconfig.json`
- Create: `interface/packages/layout/src/index.ts`
- Create: `interface/packages/policy/package.json`
- Create: `interface/packages/policy/tsconfig.json`
- Create: `interface/packages/policy/src/index.ts`
- Create: `interface/packages/repair/package.json`
- Create: `interface/packages/repair/tsconfig.json`
- Create: `interface/packages/repair/src/index.ts`
- Create: `interface/packages/signals/package.json`
- Create: `interface/packages/signals/tsconfig.json`
- Create: `interface/packages/signals/src/index.ts`
- Create: `interface/packages/render-dom/package.json`
- Create: `interface/packages/render-dom/tsconfig.json`
- Create: `interface/packages/render-dom/src/index.ts`
- Create: `interface/packages/render-terminal/package.json`
- Create: `interface/packages/render-terminal/tsconfig.json`
- Create: `interface/packages/render-terminal/src/index.ts`
- Create: `interface/packages/ikr/package.json`
- Create: `interface/packages/ikr/tsconfig.json`
- Create: `interface/packages/ikr/src/index.ts`
- Modify: `.gitignore` (add node_modules, bun.lockb patterns)

- [ ] **Step 1: Create feature branch**

```bash
cd /Users/broomva/broomva/core/life
git checkout -b feature/bro-295-ikr-scaffold
```

- [ ] **Step 2: Update .gitignore for TypeScript**

Add to root `.gitignore`:
```
# TypeScript / Node.js
node_modules/
interface/node_modules/
*.tsbuildinfo
interface/packages/*/dist/
interface/apps/*/dist/
interface/apps/*/.next/
bun.lockb
```

- [ ] **Step 3: Create workspace root package.json**

Create `interface/package.json`:
```json
{
  "name": "@life/interface-kernel",
  "private": true,
  "workspaces": [
    "packages/*",
    "apps/*"
  ],
  "scripts": {
    "build": "bun run --filter './packages/*' build",
    "test": "bun run --filter './packages/*' test",
    "lint": "biome check .",
    "lint:fix": "biome check --write .",
    "typecheck": "bun run --filter './packages/*' typecheck",
    "clean": "rm -rf packages/*/dist apps/*/dist apps/*/.next"
  },
  "devDependencies": {
    "@biomejs/biome": "^1.9.0",
    "typescript": "^5.7.0"
  }
}
```

- [ ] **Step 4: Create workspace tsconfig.json**

Create `interface/tsconfig.json`:
```json
{
  "compilerOptions": {
    "strict": true,
    "target": "ES2022",
    "module": "ESNext",
    "moduleResolution": "bundler",
    "esModuleInterop": true,
    "declaration": true,
    "declarationMap": true,
    "sourceMap": true,
    "outDir": "dist",
    "rootDir": "src",
    "skipLibCheck": true,
    "forceConsistentCasingInFileNames": true,
    "resolveJsonModule": true,
    "isolatedModules": true,
    "verbatimModuleSyntax": true
  }
}
```

- [ ] **Step 5: Create biome.json**

Create `interface/biome.json`:
```json
{
  "$schema": "https://biomejs.dev/schemas/1.9.0/schema.json",
  "organizeImports": { "enabled": true },
  "linter": {
    "enabled": true,
    "rules": { "recommended": true }
  },
  "formatter": {
    "enabled": true,
    "indentStyle": "tab",
    "lineWidth": 100
  }
}
```

- [ ] **Step 6: Create all package scaffolds**

For each package (`ir`, `layout`, `policy`, `repair`, `signals`, `render-dom`, `render-terminal`, `ikr`), create:

`interface/packages/{name}/package.json`:
```json
{
  "name": "@life/ikr-{name}",
  "version": "0.0.1",
  "type": "module",
  "main": "./dist/index.js",
  "types": "./dist/index.d.ts",
  "exports": {
    ".": { "import": "./dist/index.js", "types": "./dist/index.d.ts" }
  },
  "scripts": {
    "build": "tsc",
    "test": "vitest run",
    "typecheck": "tsc --noEmit"
  },
  "files": ["dist"]
}
```

`interface/packages/{name}/tsconfig.json`:
```json
{
  "extends": "../../tsconfig.json",
  "compilerOptions": {
    "outDir": "dist",
    "rootDir": "src"
  },
  "include": ["src"]
}
```

`interface/packages/{name}/src/index.ts`:
```typescript
// @life/ikr-{name}
export {}
```

- [ ] **Step 7: Add vitest as dev dependency**

```json
// Add to interface/package.json devDependencies:
"vitest": "^3.0.0"
```

- [ ] **Step 8: Create CLAUDE.md**

Create `interface/CLAUDE.md` with project context (IKR overview, conventions, commands).

- [ ] **Step 9: Install dependencies**

```bash
cd interface && bun install
```

- [ ] **Step 10: Verify workspace builds**

```bash
cd interface && bun run typecheck
```

- [ ] **Step 11: Commit scaffold**

```bash
git add interface/ .gitignore
git commit -m "feat(ikr): scaffold interface kernel workspace with 8 packages"
```

---

### Task 2: Semantic UI IR Types — @life/ikr-ir (BRO-296)

**Files:**
- Create: `interface/packages/ir/src/nodes.ts`
- Create: `interface/packages/ir/src/constraints.ts`
- Create: `interface/packages/ir/src/surface.ts`
- Create: `interface/packages/ir/src/solved.ts`
- Create: `interface/packages/ir/src/violations.ts`
- Create: `interface/packages/ir/src/index.ts`
- Test: `interface/packages/ir/src/__tests__/nodes.test.ts`

(Full implementation details in each step — see design spec Section 5.1 for types)

---

### Task 3: Signal Runtime — @life/ikr-signals (BRO-300)

**Files:**
- Create: `interface/packages/signals/src/ownership.ts`
- Create: `interface/packages/signals/src/scheduler.ts`
- Create: `interface/packages/signals/src/index.ts`
- Test: `interface/packages/signals/src/__tests__/ownership.test.ts`
- Test: `interface/packages/signals/src/__tests__/scheduler.test.ts`

Dependencies to install: `@preact/signals-core`

---

### Task 4: Layout Kernel — @life/ikr-layout (BRO-297)

**Files:**
- Create: `interface/packages/layout/src/text-browser.ts`
- Create: `interface/packages/layout/src/text-mono.ts`
- Create: `interface/packages/layout/src/box-layout.ts`
- Create: `interface/packages/layout/src/solve.ts`
- Create: `interface/packages/layout/src/index.ts`
- Test: `interface/packages/layout/src/__tests__/text-mono.test.ts`
- Test: `interface/packages/layout/src/__tests__/box-layout.test.ts`
- Test: `interface/packages/layout/src/__tests__/solve.test.ts`

Dependencies to install: `@chenglou/pretext`, `yoga-layout`

---

### Task 5: Constraint Policy — @life/ikr-policy (BRO-298)

**Files:**
- Create: `interface/packages/policy/src/rules.ts`
- Create: `interface/packages/policy/src/validate.ts`
- Create: `interface/packages/policy/src/index.ts`
- Test: `interface/packages/policy/src/__tests__/rules.test.ts`
- Test: `interface/packages/policy/src/__tests__/validate.test.ts`

---

### Task 6: AI Repair Loop — @life/ikr-repair (BRO-299)

**Files:**
- Create: `interface/packages/repair/src/deterministic.ts`
- Create: `interface/packages/repair/src/llm-repair.ts`
- Create: `interface/packages/repair/src/repair.ts`
- Create: `interface/packages/repair/src/index.ts`
- Test: `interface/packages/repair/src/__tests__/deterministic.test.ts`
- Test: `interface/packages/repair/src/__tests__/repair.test.ts`

Dependencies to install: `ai`

---

### Task 7: Terminal Renderer — @life/ikr-render-terminal (BRO-302)

**Files:**
- Create: `interface/packages/render-terminal/src/buffer.ts`
- Create: `interface/packages/render-terminal/src/ansi.ts`
- Create: `interface/packages/render-terminal/src/render.ts`
- Create: `interface/packages/render-terminal/src/index.ts`
- Test: `interface/packages/render-terminal/src/__tests__/buffer.test.ts`
- Test: `interface/packages/render-terminal/src/__tests__/render.test.ts`

---

### Task 8: DOM Renderer — @life/ikr-render-dom (BRO-301)

**Files:**
- Create: `interface/packages/render-dom/src/render.ts`
- Create: `interface/packages/render-dom/src/reactive.ts`
- Create: `interface/packages/render-dom/src/index.ts`
- Test: `interface/packages/render-dom/src/__tests__/render.test.ts`

---

### Task 9: Umbrella Package — @life/ikr (BRO-303 prerequisite)

**Files:**
- Modify: `interface/packages/ikr/package.json` (add workspace deps)
- Modify: `interface/packages/ikr/src/index.ts` (re-export all)

---

### Task 10: Knowledge-Graph-Memory + Conversation Tracing

**Files:**
- Create: `interface/scripts/conversation-history.py`
- Create: `interface/scripts/conversation-bridge-hook.sh`
- Create: `interface/docs/conversations/`
- Modify: `interface/.claude/settings.json`

---
