# Interface Kernel (IKR)

The IO layer of the Life Agent OS — a semantic reactive interface runtime that makes agent-generated UI measurable, constraint-aware, and self-repairing.

## Architecture

```
Semantic UI IR → Layout Kernel → Constraint Policy → AI Repair → Renderer
     (ir)         (layout)         (policy)          (repair)   (render-*)
                                                                     ↑
                                                              Signal Runtime
                                                               (signals)
```

## Packages

| Package | Purpose | Key Dependency |
|---------|---------|----------------|
| `@life/ikr-ir` | Semantic UI node types, constraints, violations | None (types only) |
| `@life/ikr-signals` | Reactive primitives + ownership tree | `@preact/signals-core` |
| `@life/ikr-layout` | Pretext text measurement + Yoga box layout | `@chenglou/pretext`, `yoga-layout` |
| `@life/ikr-policy` | Constraint validation rules | `@life/ikr-ir` |
| `@life/ikr-repair` | Two-tier repair (deterministic + AI) | `ai` (AI SDK v6) |
| `@life/ikr-render-dom` | Browser DOM renderer | `@life/ikr-signals` |
| `@life/ikr-render-terminal` | ANSI terminal renderer | None |
| `@life/ikr` | Umbrella re-export | All packages |

## Commands

```bash
cd interface
bun install              # Install all dependencies
bun run build            # Build all packages
bun run test             # Run all tests
bun run typecheck        # Type-check all packages
bun run lint             # Lint with Biome
```

## Conventions

- **Package manager**: Bun
- **Linter**: Biome (never ESLint/Prettier)
- **Module format**: ESM only (`"type": "module"`)
- **Testing**: vitest
- **TypeScript**: strict mode, ES2022 target, bundler module resolution
- **Naming**: `@life/ikr-*` for packages, `kebab-case` for files
- **No `index.ts` barrels** except at package entry points
- **Workspace deps**: Always `"workspace:*"`

## Design Spec

See `docs/superpowers/specs/2026-03-30-interface-kernel-design.md` for the full architecture.

## Linear Project

"Life Interface Kernel — Semantic Reactive IO Layer" (BRO-295 through BRO-305)
