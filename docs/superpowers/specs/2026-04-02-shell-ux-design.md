# Arcan Shell UX — Spinner, Status Line & Message Queuing

**Date**: 2026-04-02
**Status**: Approved
**Scope**: `arcan/crates/arcan/src/spinner.rs` (new), `arcan/crates/arcan/src/shell.rs` (modified)

## Problem

The Arcan shell REPL has no visual feedback between sending a message and receiving the first token (5-30s gap). No elapsed time, token count, or cost is shown during or after turns. Users cannot type their next message while a response is streaming. This is a poor experience compared to Claude Code/Noesis which show animated spinners, status stats, and support message queuing.

## Design

### Approach: ANSI Status Line

A new `spinner.rs` module in the arcan binary crate. A background thread overwrites a single stderr line using ANSI escape codes. stdout remains exclusively for LLM streaming output.

No new external dependencies. Uses `fastrand` (already in workspace) for verb selection and raw ANSI escape sequences for terminal rendering. Terminal width from `crossterm::terminal::size()` (available via ratatui).

## Status Line Format

### Thinking phase (before first token)

```
◉ Precipitating… (3.2s)
```

### Streaming phase (after first token, shown once elapsed > 5s)

```
◉ Precipitating… (8.1s · ↓ 1.2k tokens)
```

### Tool execution phase

```
  ⠋ Running bash…
```

### Completion line (replaces spinner, stays visible)

```
◉ Done (4.1s · ↓ 847 tokens · $0.0012)
```

## Glyph Animation

### Neural pulse set (primary, macOS)

```
['·', '◦', '○', '◎', '●', '◉', '●', '◎', '○', '◦']
```

A dot that expands outward and contracts — like a neuron firing. Bounce animation at ~120ms per frame.

### Arcane sigils set (alternative, configurable)

```
['✧', '✦', '✶', '✷', '✹', '✷', '✶', '✦']
```

### Fallback (non-macOS / limited terminal)

```
['·', 'o', 'O', '@', 'O', 'o']
```

### Tool execution spinner (braille dots, standard)

```
['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏']
```

### Idle marker

`◉` (filled circle with dot — the "eye" of the agent).

### Stall detection

If no new tokens arrive for 3+ seconds during streaming, the glyph color drifts from the default color toward amber using ANSI 256-color codes. This signals to the user that the connection may be slow.

### Non-TTY detection

When stderr is not a TTY (piped output, E2E tests), the spinner disables itself entirely. No ANSI codes are emitted. This ensures all existing E2E tests continue to pass without modification.

## Spinner Verbs

228 verbs total: 188 from the Noesis/Claude Code base set plus ~40 Life framework additions.

### Life framework additions (by AOS primitive)

**Cognition (Arcan)**: Arcaning, Cognizing, Reasoning, Reconstructing, Replaying, Looping

**Persistence (Lago)**: Journaling, Appending, Persisting, Sourcing, Hydrating, Projecting

**Homeostasis (Autonomic)**: Regulating, Balancing, Stabilizing, Calibrating, Adapting, Homeostating

**Tool Execution (Praxis)**: Sandboxing, Executing, Harnessing, Bridging

**Networking (Spaces)**: Networking, Broadcasting, Distributing

**Finance (Haima)**: Circulating, Settling, Billing

**Observability (Vigil)**: Observing, Tracing, Watching

**Biological / organic**: Pulsing, Breathing, Gestating, Metabolizing, Synapsing, Evolving, Mutating, Differentiating, Mitosing

### Selection

One verb picked randomly per agent turn using `fastrand`. Displayed as `{Verb}…` (unicode ellipsis). Configurable via `arcan config` in the future (append/replace mode).

## Architecture

### New file: `arcan/crates/arcan/src/spinner.rs`

Three components:

#### `ShellSpinner` struct

Owns a background thread that renders the status line to stderr at 50ms tick intervals. Communicates with the main thread via `Arc<SpinnerState>` (atomic fields + mutex for verb string).

**Public API:**

```rust
impl ShellSpinner {
    /// Start the spinner with a random verb. Returns immediately.
    fn start() -> Self;

    /// Signal that streaming tokens have begun arriving.
    fn set_streaming(&self);

    /// Increment the token counter (called from streaming callback).
    fn add_tokens(&self, count: u64);

    /// Stop the spinner and print a completion summary line.
    fn finish(&self, cost: f64);

    /// Stop the spinner and print a tool completion line.
    fn finish_tool(&self, tool_name: &str, success: bool, elapsed: Duration);
}
```

#### `SpinnerState` (internal)

```rust
struct SpinnerState {
    phase: AtomicU8,           // 0=thinking, 1=streaming, 2=done
    tokens: AtomicU64,         // accumulated token count
    started_at: Instant,       // for elapsed time calculation
    first_token_at: Mutex<Option<Instant>>,  // for stall detection
    verb: String,              // the chosen verb for this turn
    stop: AtomicBool,          // signal to stop the render thread
}
```

#### Rendering thread

- Spawned by `ShellSpinner::start()`
- Loops at 50ms intervals (`thread::sleep(Duration::from_millis(50))`)
- Each tick: compute glyph frame, format status string, write to stderr
- Uses `\r\x1b[2K` (carriage return + clear entire line) for in-place updates
- On stop signal: prints final line, exits thread
- Joins on `ShellSpinner::drop()` (or explicit `finish()`)

#### `SPINNER_VERBS` constant

Static array of 228 verb strings. `pick_verb()` selects one using `fastrand::usize()`.

### Integration in `shell.rs`

Four integration points in the existing REPL loop:

1. **Before `provider.complete_streaming()`**: `let spinner = ShellSpinner::start();`

2. **Inside the streaming callback**: On first delta, call `spinner.set_streaming()`. On each delta, estimate token count and call `spinner.add_tokens()`.

3. **After provider call completes**: `spinner.finish(cmd_ctx.session_cost_usd);`

4. **Around tool execution in `run_agent_loop`**: Start a tool-mode spinner before `execute_tool()`, finish with success/error status after.

### Message Queuing

A separate stdin reader thread pushes lines into an `mpsc::channel<String>`. The REPL loop receives from the channel instead of blocking on `stdin.read_line()`.

**Behavior:**
- While a response is streaming, typed lines accumulate in the channel buffer
- Between turns, the next message is popped immediately without re-displaying the prompt (the prompt was already shown)
- If the queue has messages, they are drained one at a time through the normal turn cycle
- Empty lines and EOF are handled the same as today

**Prompt display:**
- `arcan>` prompt is displayed only when the queue is empty and the shell is idle
- If a queued message is being processed, a `arcan> {message}` echo line is printed to show what's being sent

### What doesn't change

- stdout remains exclusively for LLM streaming output
- The `Provider` trait and `complete_streaming` signature stay the same
- Slash command dispatch is unchanged
- All existing E2E tests continue to work (spinner detects non-TTY and disables)
- The `arcan-tui` crate's existing `Spinner` widget is unaffected (it's ratatui-based for the full TUI)

## Testing

- Unit tests for glyph frame cycling, verb selection, elapsed time formatting, token count formatting
- Non-TTY detection test (spinner is no-op when stderr is not a terminal)
- E2E: existing `scripts/e2e-smoke.sh` continues to pass (piped stdin = non-TTY)
- Manual verification of animation smoothness with real Anthropic provider

## Error Handling

- Spinner thread panics are caught by `JoinHandle` on drop — never fatal to the REPL
- Write errors to stderr are silently ignored (same as current `eprintln!` calls)
- If terminal width detection fails, fall back to 80 columns
