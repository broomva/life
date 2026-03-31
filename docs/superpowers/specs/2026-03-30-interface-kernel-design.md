# Life Interface Kernel (IKR) — Design Spec

**Date**: 2026-03-30
**Author**: Carlos Escobar + Claude Opus 4.6
**Status**: Approved
**Location**: `core/life/interface/`
**Linear Project**: Life Interface Kernel — Semantic Reactive IO Layer

---

## 1. Problem Statement

AI-generated interfaces are fundamentally broken. The current model is open-loop:

```
Model emits tokens → CSS renders → Human inspects → Maybe patches
```

The model has no access to text line counts, overflow risk, layout stability under resize, or spatial relationships between elements. It generates against a black box and prays.

This affects every agent producing visible output — web dashboards, chat messages, terminal displays, reports, copilot panels.

## 2. Vision

The Interface Kernel is the **IO layer of the Life Agent OS** — a semantic reactive interface runtime that makes agent-generated UI measurable, constraint-aware, and self-repairing.

```
Model → Semantic Spec → Deterministic Solve → Validate → Repair → Render
         ↑                                       ↓
         └──── AI rewrites if constraints fail ───┘
```

This turns UI generation into a **closed-loop control system** — the same philosophical move the Agent OS applies to tool execution (praxis), networking (spaces), and persistence (lago).

## 3. Core Thesis

- **Text layout is the missing primitive.** Pretext (`@chenglou/pretext`) makes multiline text measurement programmable and DOM-free. This is the key enabler.
- **Layout should be data, not a side effect.** Once you can measure text and boxes arithmetically, layout becomes a solvable constraint problem.
- **AI repair is the genuinely novel layer.** Deterministic rules handle 80% of violations. LLM-based semantic compression handles the rest.
- **Surface-agnostic design.** The same semantic spec renders to DOM, Canvas, Terminal, PDF. Only the renderer changes.

## 4. Architecture Overview

```
┌─────────────────────────────────────────────────────────────────┐
│                    Life Interface Kernel                         │
│                                                                 │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌───────────────┐  │
│  │ Semantic  │  │  Layout  │  │ Constraint│  │   AI Repair   │  │
│  │  UI IR    │→ │  Kernel  │→ │  Policy   │→ │     Loop      │  │
│  │  (spec)   │  │(solve)   │  │(validate) │  │  (fix)        │  │
│  └──────────┘  └──────────┘  └──────────┘  └───────────────┘  │
│       ↑              ↑              ↑              ↑            │
│  json-render    Pretext+Yoga   Rule engine    AI SDK v6        │
│  compatible     (browser)      + custom       streamText +     │
│                 Arithmetic     policies       Output.object()  │
│                 (terminal)                                      │
│                                                                 │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌───────────────┐  │
│  │  Signal  │  │ render-  │  │ render-  │  │   render-     │  │
│  │ Runtime  │  │   dom    │  │ terminal │  │   canvas      │  │
│  └──────────┘  └──────────┘  └──────────┘  └───────────────┘  │
└─────────────────────────────────────────────────────────────────┘
```

## 5. Package Structure

### 5.1 TypeScript Packages (`core/life/interface/packages/`)

#### `@life/ikr-ir` — Semantic UI Intermediate Representation

The typed contract between model output and the layout engine.

```typescript
type UINode =
  | TextBlockNode
  | InlineRowNode
  | CardNode
  | ColumnNode
  | ChipNode
  | IconNode
  | ButtonNode
  | SectionNode

type TextBlockNode = {
  kind: 'textBlock'
  id: string
  text: string | Signal<string>
  role: 'title' | 'subtitle' | 'body' | 'caption' | 'label' | 'code'
  fontToken: string
  constraints?: TextConstraints
}

type TextConstraints = {
  maxLines?: number
  maxWidth?: number
  overflowPolicy?: 'clip' | 'ellipsis' | 'summarize' | 'reflow'
}

type InlineRowNode = {
  kind: 'inlineRow'
  id: string
  children: UINode[]
  wrap: boolean
  gap: number
  collapsePolicy?: 'hide-low-priority' | 'overflow-chip' | 'wrap'
}

type CardNode = {
  kind: 'card'
  id: string
  children: UINode[]
  padding: number
  widthPolicy: 'fixed' | 'shrinkWrap' | 'fill'
  constraints?: BoxConstraints
}

type BoxConstraints = {
  minWidth?: number
  maxWidth?: number
  minHeight?: number
  maxHeight?: number
  density?: 'compact' | 'normal' | 'spacious'
}

type LayoutConstraints = {
  width: number
  height?: number
  lineHeight?: number
  surface: Surface
}

type Surface =
  | { kind: 'dom'; fontMetrics: FontMetrics }
  | { kind: 'canvas'; fontMetrics: FontMetrics }
  | { kind: 'terminal'; cols: number; rows: number; monoWidth: 1 }
  | { kind: 'pdf'; pageWidth: number; pageHeight: number }
  | { kind: 'raw' }  // IR only, no rendering
```

**json-render compatibility**: The IR is designed to be convertible to/from json-render specs. A `fromJsonRender(spec, catalog)` adapter converts json-render element trees into IKR nodes, and `toJsonRender(solved)` converts back.

#### `@life/ikr-layout` — Layout Kernel

Two text measurement strategies, one box layout engine.

**Browser path (Pretext-backed)**:
```typescript
import { prepare, layout, layoutWithLines, layoutNextLine } from '@chenglou/pretext'
import Yoga from 'yoga-layout'

// Pretext wired into Yoga's MeasureFunction
function createTextMeasure(text: string, font: string) {
  const prepared = prepare(text, font)
  return (width: number, widthMode: MeasureMode, height: number, heightMode: MeasureMode) => {
    const result = layout(prepared, width, lineHeight)
    return { width: result.width ?? width, height: result.height }
  }
}
```

**Terminal path (arithmetic)**:
```typescript
function measureMonoText(text: string, maxWidth: number): { lineCount: number; height: number } {
  // wcwidth for CJK double-width characters
  const charWidth = wcwidth(text)
  const lineCount = Math.ceil(charWidth / maxWidth)
  return { lineCount, height: lineCount }
}
```

**Solved layout output**:
```typescript
type SolvedLayout = {
  valid: boolean
  width: number
  height: number
  nodes: SolvedNode[]
  violations: Violation[]
}

type SolvedNode = {
  id: string
  x: number
  y: number
  width: number
  height: number
  lineCount?: number
  overflow: boolean
  children?: SolvedNode[]
}
```

#### `@life/ikr-policy` — Constraint Validation

Rule-based violation detection:

```typescript
type Violation = {
  nodeId: string
  rule: string
  severity: 'error' | 'warning'
  actual: number
  limit: number
  repairOptions: RepairStrategy[]
}

type RepairStrategy =
  | { kind: 'summarize_text'; nodeId: string; targetChars: number }
  | { kind: 'collapse_chips'; nodeId: string; maxVisible: number }
  | { kind: 'widen_container'; nodeId: string; targetWidth: number }
  | { kind: 'switch_density'; nodeId: string; density: 'compact' }
  | { kind: 'reduce_font_token'; nodeId: string; token: string }
  | { kind: 'increase_max_lines'; nodeId: string; lines: number }
  | { kind: 'hide_node'; nodeId: string }

// Built-in rules
const defaultRules: PolicyRule[] = [
  maxLinesRule,         // textBlock exceeds maxLines
  overflowRule,         // node exceeds container bounds
  chipAtomicityRule,    // chip broken across lines
  minTouchTargetRule,   // interactive element < 44px
  spacingRule,          // elements too close together
  widowOrphanRule,      // single word on last line
]
```

#### `@life/ikr-repair` — AI Repair Loop

Two-tier repair: deterministic rules first, LLM second.

```typescript
import { streamText, Output } from 'ai'

async function repairLayout(
  spec: UISpec,
  violations: Violation[],
  options: RepairOptions
): Promise<UISpec> {
  // Tier 1: deterministic rules
  let repaired = applyDeterministicRepairs(spec, violations)
  let resolved = solveLayout(repaired, options.constraints)

  if (resolved.valid) return repaired

  // Tier 2: LLM-based semantic compression
  const remaining = resolved.violations
  const result = await streamText({
    model: options.model ?? 'anthropic/claude-sonnet-4.6', // AI Gateway slug
    system: REPAIR_SYSTEM_PROMPT,
    prompt: formatRepairPrompt(repaired, remaining),
    output: Output.object({ schema: repairPatchSchema }),
  })

  return applyRepairPatches(repaired, result.object)
}

// Deterministic repair strategies
function applyDeterministicRepairs(spec: UISpec, violations: Violation[]): UISpec {
  for (const v of violations) {
    const strategy = v.repairOptions[0] // highest priority
    switch (strategy.kind) {
      case 'collapse_chips':
        spec = collapseChips(spec, strategy.nodeId, strategy.maxVisible)
        break
      case 'switch_density':
        spec = switchDensity(spec, strategy.nodeId, strategy.density)
        break
      case 'widen_container':
        spec = widenContainer(spec, strategy.nodeId, strategy.targetWidth)
        break
      case 'hide_node':
        spec = hideNode(spec, strategy.nodeId)
        break
      // summarize_text and reduce_font_token go to Tier 2
    }
  }
  return spec
}
```

#### `@life/ikr-signals` — Reactive Runtime

Built on `@preact/signals-core` with ownership tree from SolidJS patterns.

```typescript
import { signal, computed, effect, batch } from '@preact/signals-core'

// Re-export core primitives
export { signal, computed, effect, batch }

// Add ownership tree for hierarchical disposal
export function createRoot<T>(fn: (dispose: () => void) => T): T
export function onCleanup(fn: () => void): void
export function getOwner(): Owner | null
export function runWithOwner<T>(owner: Owner, fn: () => T): T

// Spatial dependency: signal that triggers re-layout
export function layoutSignal<T>(initial: T): Signal<T>

// Batch layout recomputation (debounced to rAF)
export function scheduleLayout(fn: () => void): void
```

#### `@life/ikr-render-dom` — Browser DOM Renderer

```typescript
function renderToDOM(solved: SolvedLayout, container: HTMLElement): void {
  for (const node of solved.nodes) {
    const el = document.createElement('div')
    el.style.position = 'absolute'
    el.style.left = `${node.x}px`
    el.style.top = `${node.y}px`
    el.style.width = `${node.width}px`
    el.style.height = `${node.height}px`
    // ... apply text content, styles, classes
    container.appendChild(el)
  }
}

// Reactive: only update changed nodes
function createReactiveRenderer(container: HTMLElement) {
  return (solved: SolvedLayout) => {
    // Diff against previous render, patch only changed nodes
  }
}
```

#### `@life/ikr-render-terminal` — ANSI Terminal Renderer

```typescript
function renderToTerminal(solved: SolvedLayout): string {
  const { cols, rows } = solved.surface as TerminalSurface
  const buffer = Array.from({ length: rows }, () => Array(cols).fill(' '))

  for (const node of solved.flatNodes()) {
    writeToBuffer(buffer, node) // ANSI escape codes for position + style
  }

  return buffer.map(row => row.join('')).join('\n')
}

// Reactive: diff previous buffer, emit only cursor moves + changed chars
function createReactiveTerminalRenderer(stream: NodeJS.WriteStream) {
  let prevBuffer: string[][] | null = null
  return (solved: SolvedLayout) => {
    const newBuffer = computeBuffer(solved)
    const patches = diffBuffers(prevBuffer, newBuffer)
    for (const patch of patches) {
      stream.write(`\x1b[${patch.row + 1};${patch.col + 1}H${patch.content}`) // ANSI is 1-indexed
    }
    prevBuffer = newBuffer
  }
}
```

### 5.2 Rust Crates (Phase 2-3, `core/life/interface/crates/`)

#### `ikr-core` — Semantic IR Types (Rust)

```rust
use serde::{Serialize, Deserialize};

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum UINode {
    TextBlock(TextBlockNode),
    InlineRow(InlineRowNode),
    Card(CardNode),
    Column(ColumnNode),
    Chip(ChipNode),
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TextBlockNode {
    pub id: String,
    pub text: String,
    pub role: TextRole,
    pub font_token: String,
    pub constraints: Option<TextConstraints>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SolvedLayout {
    pub valid: bool,
    pub width: f32,
    pub height: f32,
    pub nodes: Vec<SolvedNode>,
    pub violations: Vec<Violation>,
}
```

#### `ikr-layout` — Taffy-backed Layout (Rust, Phase 3)

Replaces Yoga with Taffy (pure Rust, flexbox + grid). Text measurement via `ikr-core` text-mono for terminal, or Pretext via WASM bridge for browser.

#### `ikr-terminal` — Native ANSI Renderer (Rust)

Used by Arcan daemon for direct terminal output. No Node.js dependency.

#### `ikr-wasm` — Browser Bridge (Rust→WASM, Phase 3)

Exposes Rust layout engine and policy validation to browser via WASM. Replaces Yoga dependency with native Taffy.

### 5.3 aiOS Integration

```rust
/// The agent's interface to perceivable surfaces.
pub trait Interface {
    /// Compose a semantic UI spec from intent + data
    fn compose(&self, intent: &Intent, data: &Value) -> UISpec;
    /// Solve layout for a target surface
    fn solve(&self, spec: &UISpec, surface: &Surface) -> SolvedLayout;
    /// Validate constraints
    fn validate(&self, solved: &SolvedLayout) -> Vec<Violation>;
    /// Render to a surface
    fn render(&self, solved: &SolvedLayout, target: &mut dyn RenderTarget) -> Result<()>;
}
```

## 6. Developer API

### High-level (what most consumers use)

```typescript
import { composeSemanticUI, solveLayout, repairLayout, render } from '@life/ikr'

// 1. Compose
const ui = composeSemanticUI({
  intent: 'show procurement risk summary',
  data,
  surface: 'right_panel',
  constraints: { width: 360, height: 640, density: 'compact' }
})

// 2. Solve
const solved = solveLayout(ui)

// 3. Validate + Repair
if (!solved.valid) {
  const repaired = await repairLayout(ui, solved.violations)
  return render(solveLayout(repaired))
}

// 4. Render
return render(solved)
```

### Streaming (AI SDK integration)

```typescript
import { streamSemanticUI } from '@life/ikr/ai'

// In a Next.js route handler or Workflow DevKit step:
const stream = streamSemanticUI({
  model: 'anthropic/claude-sonnet-4.6',
  intent: 'generate dashboard cards',
  data: dashboardData,
  constraints: { width: 360 },
  onViolation: (v) => autoRepair(v), // inline repair during streaming
  writable: getWritable<UIMessageChunk>(), // Workflow DevKit streaming
})
```

### Terminal (CLI agents)

```typescript
import { solveLayout, renderToTerminal } from '@life/ikr'

const spec = composeSemanticUI({ intent: 'deployment status', data, surface: 'terminal' })
const solved = solveLayout(spec, {
  surface: { kind: 'terminal', cols: process.stdout.columns, rows: process.stdout.rows, monoWidth: 1 }
})
process.stdout.write(renderToTerminal(solved))
```

### MCP Tools (for external agents)

```typescript
// Exposed as MCP tools
tools: {
  compute_layout:      (spec, constraints) => solveLayout(spec, constraints),
  validate_constraints: (spec, constraints) => solveLayout(spec, constraints).violations,
  repair_layout:       (spec, violations) => repairLayout(spec, violations),
  suggest_variants:    (spec, constraints) => generateVariants(spec, constraints),
  measure_text:        (text, font, width) => measureText(text, font, width),
}
```

## 7. Key Design Decisions

### 7.1 Fork Textura's Pretext+Yoga integration, don't depend on it

Textura (`@razroo/textura`) already wires Pretext into Yoga's MeasureFunction. We absorb this ~500 LOC integration and evolve it independently. Rationale: Textura solves static snapshots; we need reactive, incremental layout with spatial dependency tracking.

### 7.2 Use @preact/signals-core, draw from SolidJS patterns

`@preact/signals-core` (1.6KB) provides the reactive primitives. We add an ownership tree (SolidJS pattern) for hierarchical disposal and spatial dependency signals for layout-aware reactivity. We do NOT use SolidJS directly — its JSX compilation is opinionated and we need control over the DOM commit phase.

### 7.3 json-render as spec format, not as runtime

json-render catalogs define component schemas. The IKR consumes these schemas as input but owns the layout solving, constraint validation, and rendering pipeline. This means existing json-render users (HealthOS) can adopt IKR incrementally.

### 7.4 Terminal as first-class surface, not afterthought

Terminal rendering uses the same semantic IR and constraint system. Text measurement is trivial (monospace arithmetic with wcwidth). This makes the kernel immediately useful for CLI agents, TUI dashboards, and Arcan daemon output.

### 7.5 Two-tier repair: rules first, LLM second

Deterministic repair (collapse chips, switch density, widen container) handles ~80% of violations with zero latency and zero cost. LLM repair (summarize text, rewrite labels) handles the remaining 20% that require semantic understanding.

## 8. V1 Scope (Phase 1)

### Supported primitives
- `textBlock` (title, subtitle, body, caption, label, code)
- `inlineRow` (wrapping row of mixed content)
- `chip` (atomic inline element)
- `icon` (fixed-size inline element)
- `button` (interactive CTA)
- `card` (container with padding)
- `column` (vertical stack)
- `section` (titled group)

### Supported layouts
- Vertical stack (column)
- Inline wrap row
- Card (padded container)
- Section group
- Nested composition

### Supported constraints
- maxLines, maxWidth, maxHeight
- density (compact/normal/spacious)
- overflow policy (clip/ellipsis/summarize/reflow)
- chip atomicity (keep whole)
- min touch target (44px)

### Supported repair strategies
- Summarize body text (LLM)
- Shrink/rewrite title (LLM)
- Collapse chips (+N overflow)
- Widen container within bounds
- Switch density mode
- Hide low-priority nodes

### Renderers
- `render-dom` (browser)
- `render-terminal` (ANSI)

### Integrations
- AI SDK v6 (streamText, Output.object)
- AI Gateway (OIDC auth)
- json-render (catalog adapter)
- MCP tools (5 tools)

## 9. What This Does NOT Include (Explicit Non-Goals)

- Full CSS reimplementation (we support flexbox-like layout, not all of CSS)
- Font rendering engine (Pretext measures; the surface renders)
- Animation/transitions (future; renderer concern)
- Selection/caret behavior (future; rich text editing)
- Hydration/SSR resumability (future; if we build a web framework)
- Full bidi rendering at renderer level (Pretext handles measurement; visual reordering is renderer's job)
- Replacing React (we complement it; React can be a renderer target)

## 10. Phasing

### Phase 1 — TypeScript Foundation (4-6 weeks)
- `@life/ikr-ir` — Semantic IR types + json-render adapter
- `@life/ikr-layout` — Pretext+Yoga integration (forked from Textura)
- `@life/ikr-policy` — Constraint rules + violation detection
- `@life/ikr-repair` — Two-tier repair (rules + AI SDK)
- `@life/ikr-signals` — @preact/signals-core + ownership tree
- `@life/ikr-render-dom` — Browser DOM renderer
- `@life/ikr-render-terminal` — ANSI terminal renderer
- `@life/ikr` — Umbrella package (re-exports)
- Playground app (Next.js 16, AI Gateway, demo cards)
- MCP server (5 tools)

### Phase 2 — Rust Core + Arcan Integration (after Phase 1)
- `ikr-core` crate — Shared IR types (serde, no_std)
- `ikr-terminal` crate — Native ANSI renderer for Arcan
- Arcan daemon integration via Interface trait
- Text-mono measurement in Rust (wcwidth)

### Phase 3 — Full Native Stack (later)
- `ikr-layout` crate — Taffy-backed (replaces Yoga dependency)
- `ikr-wasm` — Rust layout+policy in browser via WASM
- `render-canvas` — Canvas/WebGL renderer
- `render-pdf` — PDF export renderer
- Full aiOS Interface trait implementation
- Spatial dependency tracking in signal graph

## 11. Success Criteria

1. An AI agent can generate a dashboard card spec, have it layout-validated, and auto-repaired — without a browser
2. The same spec renders correctly to DOM and terminal
3. Layout validation runs in < 1ms for typical card specs (after prepare())
4. AI repair resolves 95%+ of constraint violations
5. Existing json-render catalogs work with IKR via adapter
6. MCP tools enable any external agent to use the layout kernel

## 12. Dependencies and Risks

| Dependency | Risk | Mitigation |
|-----------|------|------------|
| `@chenglou/pretext` (v0.0.3) | Very new (published 2026-03-26), API may change | Pin version; text measurement interface is small enough to adapt |
| `yoga-layout` WASM | Large bundle (~200KB), C++ maintenance | Phase 3 replaces with Taffy (Rust, smaller, native) |
| `@preact/signals-core` | Stable but may diverge from TC39 Signals | Thin wrapper; can swap implementation later |
| Textura fork | May diverge from upstream improvements | The integration is ~500 LOC; maintenance burden is low |
| AI SDK v6 | Rapid evolution | Only used in repair package; isolated dependency |

## 13. Relationship to Existing Ecosystem

| Project | Relationship |
|---------|-------------|
| **json-render** | Spec format compatibility — IKR consumes json-render catalogs via adapter |
| **Arcan Glass** | Design tokens — IKR uses Arcan Glass font/color/spacing tokens |
| **ChatOS** | Consumer — ChatOS state layer can wrap in IKR signals; AI chat output uses IKR for layout |
| **HealthOS** | Consumer — Existing json-render MetricCards gain constraint validation |
| **Arcan daemon** | Consumer — Terminal output via ikr-terminal (Rust) |
| **Praxis** | Peer — Praxis executes tools; IKR presents results |
| **Spaces** | Peer — Agent messages can carry IKR specs for structured rendering |
| **Vigil** | Peer — Observability panels rendered via IKR |
| **Textura** | Forked integration — Pretext+Yoga wiring absorbed, not depended on |

---

*This spec was produced through collaborative brainstorming with parallel research agents analyzing Pretext API, signal-based frameworks (SolidJS, Svelte 5, Preact Signals, Vue 3, Lit), layout engines (Yoga, Taffy, Flutter, Cassowary, Textura), and semantic UI IR formats (A2UI, json-render, Intuit Player, Airbnb SDUI).*
