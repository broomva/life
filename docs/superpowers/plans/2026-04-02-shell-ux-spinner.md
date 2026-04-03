# Arcan Shell UX — Spinner, Status Line & Message Queuing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add animated thinking/streaming spinner, live stats, and message queuing to the Arcan shell REPL — matching Claude Code/Noesis UX quality with Life framework flavor.

**Architecture:** A new `spinner.rs` module in the arcan binary crate with a background render thread that overwrites a single stderr line via ANSI escape codes. The REPL loop in `shell.rs` is modified at four points (before provider call, inside streaming callback, after provider call, around tool execution). A stdin reader thread enables message queuing. Non-TTY environments get a no-op spinner so E2E tests pass unchanged.

**Tech Stack:** Rust 2024, `fastrand` 2.x (verb selection), `crossterm` 0.29 (terminal width), raw ANSI escape sequences (stderr rendering).

---

## File Structure

| File | Action | Responsibility |
|------|--------|----------------|
| `crates/arcan/src/spinner.rs` | **Create** | ShellSpinner, SpinnerState, glyph sets, verb list, rendering thread, formatting helpers |
| `crates/arcan/src/shell.rs` | **Modify** | Integrate spinner at 4 points, replace stdin blocking with mpsc channel |
| `crates/arcan/src/main.rs` | **Modify** | Add `mod spinner;` declaration |
| `crates/arcan/Cargo.toml` | **Modify** | Add `fastrand` and `crossterm` dependencies |

---

### Task 1: Add dependencies

**Files:**
- Modify: `crates/arcan/Cargo.toml`

- [ ] **Step 1: Add `fastrand` and `crossterm` to dependencies**

In `crates/arcan/Cargo.toml`, add after the `chrono.workspace = true` line:

```toml
fastrand = "2"
crossterm = "0.29"
```

- [ ] **Step 2: Verify it compiles**

Run: `cd /Users/broomva/broomva/core/life/arcan && cargo check -p arcan`
Expected: compiles with no errors.

- [ ] **Step 3: Commit**

```bash
git add crates/arcan/Cargo.toml
git commit -m "chore(arcan): add fastrand and crossterm deps for shell spinner"
```

---

### Task 2: Create spinner.rs — constants, glyph sets, verb list, formatting helpers

**Files:**
- Create: `crates/arcan/src/spinner.rs`
- Modify: `crates/arcan/src/main.rs` (add `mod spinner;`)

- [ ] **Step 1: Write tests for glyph cycling, verb selection, and formatting helpers**

Create `crates/arcan/src/spinner.rs` with the following test module and stub types:

```rust
//! Animated status line for the Arcan shell REPL.
//!
//! Renders a spinner with a random verb, elapsed time, token count, and cost
//! to stderr using ANSI escape codes. Disables itself when stderr is not a TTY.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Glyph sets
// ---------------------------------------------------------------------------

/// Neural pulse — primary animation for macOS.
const NEURAL_PULSE: &[char] = &['·', '◦', '○', '◎', '●', '◉', '●', '◎', '○', '◦'];

/// Arcane sigils — alternative animation.
#[allow(dead_code)]
const ARCANE_SIGILS: &[char] = &['✧', '✦', '✶', '✷', '✹', '✷', '✶', '✦'];

/// Fallback for limited terminals.
const FALLBACK_GLYPHS: &[char] = &['·', 'o', 'O', '@', 'O', 'o'];

/// Braille spinner for tool execution.
const TOOL_SPINNER: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// The idle/completion marker glyph.
const IDLE_MARKER: char = '◉';

/// Pick the appropriate glyph set for the current platform.
fn default_glyphs() -> &'static [char] {
    if cfg!(target_os = "macos") {
        NEURAL_PULSE
    } else {
        FALLBACK_GLYPHS
    }
}

// ---------------------------------------------------------------------------
// Spinner verbs (228 total: 188 Noesis base + 40 Life framework)
// ---------------------------------------------------------------------------

const SPINNER_VERBS: &[&str] = &[
    // --- Noesis / Claude Code base set (188) ---
    "Accomplishing", "Actioning", "Actualizing", "Architecting", "Baking",
    "Beaming", "Beboppin'", "Befuddling", "Billowing", "Blanching",
    "Bloviating", "Boogieing", "Boondoggling", "Booping", "Bootstrapping",
    "Brewing", "Bunning", "Burrowing", "Calculating", "Canoodling",
    "Caramelizing", "Cascading", "Catapulting", "Cerebrating", "Channeling",
    "Channelling", "Choreographing", "Churning", "Coalescing", "Cogitating",
    "Combobulating", "Composing", "Computing", "Concocting", "Considering",
    "Contemplating", "Cooking", "Crafting", "Creating", "Crunching",
    "Crystallizing", "Cultivating", "Deciphering", "Deliberating", "Determining",
    "Dilly-dallying", "Discombobulating", "Doing", "Doodling", "Drizzling",
    "Ebbing", "Effecting", "Elucidating", "Embellishing", "Enchanting",
    "Envisioning", "Evaporating", "Fermenting", "Fiddle-faddling", "Finagling",
    "Flambéing", "Flibbertigibbeting", "Flowing", "Flummoxing", "Fluttering",
    "Forging", "Forming", "Frolicking", "Frosting", "Gallivanting",
    "Galloping", "Garnishing", "Generating", "Gesticulating", "Germinating",
    "Grooving", "Gusting", "Harmonizing", "Hashing", "Hatching",
    "Herding", "Honking", "Hullaballooing", "Hyperspacing", "Ideating",
    "Imagining", "Improvising", "Incubating", "Inferring", "Infusing",
    "Ionizing", "Jitterbugging", "Julienning", "Kneading", "Leavening",
    "Levitating", "Lollygagging", "Manifesting", "Marinating", "Meandering",
    "Metamorphosing", "Misting", "Moonwalking", "Moseying", "Mulling",
    "Mustering", "Musing", "Nebulizing", "Nesting", "Newspapering",
    "Noodling", "Nucleating", "Orbiting", "Orchestrating", "Osmosing",
    "Perambulating", "Percolating", "Perusing", "Philosophising",
    "Photosynthesizing", "Pollinating", "Pondering", "Pontificating", "Pouncing",
    "Precipitating", "Prestidigitating", "Processing", "Proofing", "Propagating",
    "Puttering", "Puzzling", "Quantumizing", "Razzle-dazzling", "Razzmatazzing",
    "Recombobulating", "Reticulating", "Roosting", "Ruminating", "Sautéing",
    "Scampering", "Schlepping", "Scurrying", "Seasoning", "Shenaniganing",
    "Shimmying", "Simmering", "Skedaddling", "Sketching", "Slithering",
    "Smooshing", "Sock-hopping", "Spelunking", "Spinning", "Sprouting",
    "Stewing", "Sublimating", "Swirling", "Swooping", "Symbioting",
    "Synthesizing", "Tempering", "Thinking", "Thundering", "Tinkering",
    "Tomfoolering", "Topsy-turvying", "Transfiguring", "Transmuting", "Twisting",
    "Undulating", "Unfurling", "Unravelling", "Vibing", "Waddling",
    "Wandering", "Warping", "Whatchamacalliting", "Whirlpooling", "Whirring",
    "Whisking", "Wibbling", "Working", "Wrangling", "Zesting", "Zigzagging",
    // --- Life framework additions (40) ---
    // Cognition (Arcan)
    "Arcaning", "Cognizing", "Reasoning", "Reconstructing", "Replaying", "Looping",
    // Persistence (Lago)
    "Journaling", "Appending", "Persisting", "Sourcing", "Hydrating", "Projecting",
    // Homeostasis (Autonomic)
    "Regulating", "Balancing", "Stabilizing", "Calibrating", "Adapting", "Homeostating",
    // Tool Execution (Praxis)
    "Sandboxing", "Executing", "Harnessing", "Bridging",
    // Networking (Spaces)
    "Networking", "Broadcasting", "Distributing",
    // Finance (Haima)
    "Circulating", "Settling", "Billing",
    // Observability (Vigil)
    "Observing", "Tracing", "Watching",
    // Biological / organic
    "Pulsing", "Breathing", "Gestating", "Metabolizing", "Synapsing",
    "Evolving", "Mutating", "Differentiating", "Mitosing",
];

/// Pick a random spinner verb.
fn pick_verb() -> &'static str {
    SPINNER_VERBS[fastrand::usize(..SPINNER_VERBS.len())]
}

// ---------------------------------------------------------------------------
// Formatting helpers
// ---------------------------------------------------------------------------

/// Format a duration as a human-readable string: "3.2s", "1m 12s", "1h 5m".
fn format_duration(d: Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        let tenths = d.subsec_millis() / 100;
        format!("{secs}.{tenths}s")
    } else if secs < 3600 {
        let mins = secs / 60;
        let remainder = secs % 60;
        format!("{mins}m {remainder}s")
    } else {
        let hours = secs / 3600;
        let mins = (secs % 3600) / 60;
        format!("{hours}h {mins}m")
    }
}

/// Format a token count with K/M suffix: "847", "1.2k", "2.5M".
fn format_tokens(tokens: u64) -> String {
    if tokens < 1_000 {
        format!("{tokens}")
    } else if tokens < 1_000_000 {
        let k = tokens as f64 / 1_000.0;
        if k < 10.0 {
            format!("{k:.1}k")
        } else {
            format!("{:.0}k", k)
        }
    } else {
        let m = tokens as f64 / 1_000_000.0;
        format!("{m:.1}M")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Glyph tests ---

    #[test]
    fn neural_pulse_has_10_frames() {
        assert_eq!(NEURAL_PULSE.len(), 10);
    }

    #[test]
    fn tool_spinner_has_10_frames() {
        assert_eq!(TOOL_SPINNER.len(), 10);
    }

    #[test]
    fn default_glyphs_returns_non_empty() {
        assert!(!default_glyphs().is_empty());
    }

    #[test]
    fn glyph_frame_wraps_around() {
        let glyphs = NEURAL_PULSE;
        for i in 0..30 {
            let frame = i % glyphs.len();
            let _ = glyphs[frame]; // should not panic
        }
    }

    // --- Verb tests ---

    #[test]
    fn verb_list_has_expected_count() {
        // 188 base + 40 Life = 228
        assert!(
            SPINNER_VERBS.len() >= 220,
            "Expected 220+ verbs, got {}",
            SPINNER_VERBS.len()
        );
    }

    #[test]
    fn pick_verb_returns_valid_verb() {
        let verb = pick_verb();
        assert!(!verb.is_empty());
        assert!(SPINNER_VERBS.contains(&verb));
    }

    #[test]
    fn all_verbs_end_with_ing_or_apostrophe() {
        for verb in SPINNER_VERBS {
            assert!(
                verb.ends_with("ing") || verb.ends_with("ing'") || verb.contains('-'),
                "Verb '{}' doesn't match expected pattern",
                verb
            );
        }
    }

    // --- Duration formatting tests ---

    #[test]
    fn format_duration_sub_minute() {
        assert_eq!(format_duration(Duration::from_millis(3200)), "3.2s");
        assert_eq!(format_duration(Duration::from_millis(500)), "0.5s");
        assert_eq!(format_duration(Duration::from_secs(0)), "0.0s");
        assert_eq!(format_duration(Duration::from_secs(59)), "59.0s");
    }

    #[test]
    fn format_duration_minutes() {
        assert_eq!(format_duration(Duration::from_secs(72)), "1m 12s");
        assert_eq!(format_duration(Duration::from_secs(600)), "10m 0s");
    }

    #[test]
    fn format_duration_hours() {
        assert_eq!(format_duration(Duration::from_secs(3661)), "1h 1m");
    }

    // --- Token formatting tests ---

    #[test]
    fn format_tokens_small() {
        assert_eq!(format_tokens(0), "0");
        assert_eq!(format_tokens(847), "847");
        assert_eq!(format_tokens(999), "999");
    }

    #[test]
    fn format_tokens_thousands() {
        assert_eq!(format_tokens(1_200), "1.2k");
        assert_eq!(format_tokens(5_000), "5.0k");
        assert_eq!(format_tokens(15_000), "15k");
        assert_eq!(format_tokens(999_999), "1000k");
    }

    #[test]
    fn format_tokens_millions() {
        assert_eq!(format_tokens(1_500_000), "1.5M");
        assert_eq!(format_tokens(2_000_000), "2.0M");
    }
}
```

- [ ] **Step 2: Register the module in main.rs**

In `crates/arcan/src/main.rs`, add after `mod shell;`:

```rust
mod spinner;
```

- [ ] **Step 3: Run tests to verify they pass**

Run: `cd /Users/broomva/broomva/core/life/arcan && cargo test -p arcan -- spinner`
Expected: all spinner tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/arcan/src/spinner.rs crates/arcan/src/main.rs
git commit -m "feat(arcan): add spinner constants, glyph sets, verbs, and formatting helpers"
```

---

### Task 3: Implement ShellSpinner with background render thread

**Files:**
- Modify: `crates/arcan/src/spinner.rs`

- [ ] **Step 1: Write tests for ShellSpinner lifecycle**

Add these tests to the existing `#[cfg(test)] mod tests` block in `spinner.rs`:

```rust
    // --- ShellSpinner lifecycle tests ---

    #[test]
    fn spinner_start_and_finish_does_not_panic() {
        // Non-TTY (test runner) — spinner should be a no-op but not crash.
        let spinner = ShellSpinner::start();
        std::thread::sleep(Duration::from_millis(100));
        spinner.finish(0.001);
    }

    #[test]
    fn spinner_set_streaming_and_add_tokens() {
        let spinner = ShellSpinner::start();
        spinner.set_streaming();
        spinner.add_tokens(100);
        spinner.add_tokens(200);
        assert_eq!(spinner.state.tokens.load(Ordering::Relaxed), 300);
        spinner.finish(0.0);
    }

    #[test]
    fn spinner_finish_tool() {
        let spinner = ShellSpinner::start_tool("bash");
        std::thread::sleep(Duration::from_millis(50));
        spinner.finish_tool(true);
    }

    #[test]
    fn spinner_is_noop_when_not_tty() {
        // In test context, stderr is not a TTY.
        let spinner = ShellSpinner::start();
        // Should not produce any ANSI output; just verify it completes.
        spinner.set_streaming();
        spinner.add_tokens(500);
        spinner.finish(0.05);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd /Users/broomva/broomva/core/life/arcan && cargo test -p arcan -- spinner`
Expected: FAIL — `ShellSpinner` not defined.

- [ ] **Step 3: Implement ShellSpinner**

Add the following above the `#[cfg(test)]` module in `spinner.rs`:

```rust
// ---------------------------------------------------------------------------
// Spinner phases
// ---------------------------------------------------------------------------

const PHASE_THINKING: u8 = 0;
const PHASE_STREAMING: u8 = 1;
const PHASE_DONE: u8 = 2;

// ---------------------------------------------------------------------------
// SpinnerState — shared between main thread and render thread
// ---------------------------------------------------------------------------

struct SpinnerState {
    phase: AtomicU8,
    tokens: AtomicU64,
    stop: AtomicBool,
    started_at: Instant,
    first_token_at: Mutex<Option<Instant>>,
    verb: String,
    tool_name: Option<String>,
}

// ---------------------------------------------------------------------------
// ShellSpinner — public API
// ---------------------------------------------------------------------------

/// Animated status line for the Arcan shell.
///
/// Start with `ShellSpinner::start()` before a provider call, then call
/// `set_streaming()` when the first token arrives, `add_tokens()` as deltas
/// stream in, and `finish()` when the turn is complete.
///
/// When stderr is not a TTY (piped, test runner), the spinner is a no-op.
pub struct ShellSpinner {
    pub(crate) state: Arc<SpinnerState>,
    handle: Option<std::thread::JoinHandle<()>>,
    is_tty: bool,
}

impl ShellSpinner {
    /// Start the spinner for an LLM provider call. Picks a random verb.
    pub fn start() -> Self {
        let is_tty = std::io::IsTerminal::is_terminal(&std::io::stderr());
        let state = Arc::new(SpinnerState {
            phase: AtomicU8::new(PHASE_THINKING),
            tokens: AtomicU64::new(0),
            stop: AtomicBool::new(false),
            started_at: Instant::now(),
            first_token_at: Mutex::new(None),
            verb: pick_verb().to_string(),
            tool_name: None,
        });

        let handle = if is_tty {
            let s = Arc::clone(&state);
            Some(std::thread::spawn(move || render_loop(s, false)))
        } else {
            None
        };

        Self {
            state,
            handle,
            is_tty,
        }
    }

    /// Start a tool-execution spinner.
    pub fn start_tool(tool_name: &str) -> Self {
        let is_tty = std::io::IsTerminal::is_terminal(&std::io::stderr());
        let state = Arc::new(SpinnerState {
            phase: AtomicU8::new(PHASE_THINKING),
            tokens: AtomicU64::new(0),
            stop: AtomicBool::new(false),
            started_at: Instant::now(),
            first_token_at: Mutex::new(None),
            verb: "Running".to_string(),
            tool_name: Some(tool_name.to_string()),
        });

        let handle = if is_tty {
            let s = Arc::clone(&state);
            Some(std::thread::spawn(move || render_loop(s, true)))
        } else {
            None
        };

        Self {
            state,
            handle,
            is_tty,
        }
    }

    /// Signal that streaming tokens have begun.
    pub fn set_streaming(&self) {
        self.state.phase.store(PHASE_STREAMING, Ordering::Relaxed);
        let mut ft = self.state.first_token_at.lock().unwrap();
        if ft.is_none() {
            *ft = Some(Instant::now());
        }
    }

    /// Increment the accumulated token counter.
    pub fn add_tokens(&self, count: u64) {
        self.state.tokens.fetch_add(count, Ordering::Relaxed);
    }

    /// Stop the spinner and print a completion summary.
    pub fn finish(self, cost: f64) {
        self.state.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle {
            let _ = h.join();
        }
        if self.is_tty {
            let elapsed = self.state.started_at.elapsed();
            let tokens = self.state.tokens.load(Ordering::Relaxed);
            let mut parts = vec![format_duration(elapsed)];
            if tokens > 0 {
                parts.push(format!("\u{2193} {} tokens", format_tokens(tokens)));
            }
            if cost > 0.0001 {
                parts.push(format!("${cost:.4}"));
            }
            eprint!("\r\x1b[2K");
            eprintln!("{IDLE_MARKER} Done ({})", parts.join(" \u{00b7} "));
        }
    }

    /// Stop a tool-execution spinner and print result.
    pub fn finish_tool(self, success: bool) {
        self.state.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle {
            let _ = h.join();
        }
        if self.is_tty {
            let elapsed = self.state.started_at.elapsed();
            let name = self
                .state
                .tool_name
                .as_deref()
                .unwrap_or("tool");
            let marker = if success { "\x1b[32m\u{2713}\x1b[0m" } else { "\x1b[31m\u{2717}\x1b[0m" };
            eprint!("\r\x1b[2K");
            eprintln!("  {marker} {name} ({})", format_duration(elapsed));
        }
    }
}

impl Drop for ShellSpinner {
    fn drop(&mut self) {
        self.state.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

// ---------------------------------------------------------------------------
// Render loop (runs on background thread)
// ---------------------------------------------------------------------------

fn render_loop(state: Arc<SpinnerState>, is_tool: bool) {
    use std::io::Write;

    let glyphs = if is_tool { TOOL_SPINNER } else { default_glyphs() };
    let mut frame: usize = 0;
    let tick = Duration::from_millis(50);
    // Advance glyph every ~120ms (roughly every 2-3 ticks).
    let frames_per_glyph: usize = 3; // 50ms * 3 = 150ms, close to 120ms target
    let mut tick_count: usize = 0;

    while !state.stop.load(Ordering::Relaxed) {
        tick_count += 1;
        if tick_count % frames_per_glyph == 0 {
            frame = (frame + 1) % glyphs.len();
        }

        let glyph = glyphs[frame];
        let elapsed = state.started_at.elapsed();
        let phase = state.phase.load(Ordering::Relaxed);
        let tokens = state.tokens.load(Ordering::Relaxed);

        // Build the status line
        let line = if is_tool {
            let name = state.tool_name.as_deref().unwrap_or("tool");
            format!("  {glyph} {} {name}\u{2026}", state.verb)
        } else {
            let mut status = format!("{glyph} {}\u{2026} ({}",
                state.verb, format_duration(elapsed));
            if phase == PHASE_STREAMING && tokens > 0 {
                status.push_str(&format!(" \u{00b7} \u{2193} {} tokens", format_tokens(tokens)));
            }
            // Stall detection: if streaming but no new tokens for 3s
            if phase == PHASE_STREAMING {
                let ft = state.first_token_at.lock().unwrap();
                if let Some(first) = *ft {
                    if first.elapsed() > Duration::from_secs(3) && tokens == 0 {
                        // Would add amber color here, but keep simple for now
                    }
                }
            }
            status.push(')');
            status
        };

        // Truncate to terminal width
        let width = crossterm::terminal::size().map(|(w, _)| w as usize).unwrap_or(80);
        let display = if line.len() > width {
            format!("{}\u{2026}", &line[..width.saturating_sub(1)])
        } else {
            line
        };

        let mut stderr = std::io::stderr().lock();
        let _ = write!(stderr, "\r\x1b[2K{display}");
        let _ = stderr.flush();

        std::thread::sleep(tick);
    }

    // Clear the spinner line on exit
    let mut stderr = std::io::stderr().lock();
    let _ = write!(stderr, "\r\x1b[2K");
    let _ = stderr.flush();
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd /Users/broomva/broomva/core/life/arcan && cargo test -p arcan -- spinner`
Expected: all spinner tests pass.

- [ ] **Step 5: Run `cargo fmt` and `cargo clippy`**

```bash
cd /Users/broomva/broomva/core/life/arcan && cargo fmt && cargo clippy -p arcan -- -D warnings
```

Expected: no warnings or errors.

- [ ] **Step 6: Commit**

```bash
git add crates/arcan/src/spinner.rs
git commit -m "feat(arcan): implement ShellSpinner with background render thread"
```

---

### Task 4: Integrate spinner into the provider call path in shell.rs

**Files:**
- Modify: `crates/arcan/src/shell.rs`

- [ ] **Step 1: Add spinner to run_agent_loop — before provider call**

In `shell.rs`, inside `run_agent_loop`, find the line (around line 1402):

```rust
        let turn = provider.complete_streaming(&request, &|delta| {
            let mut out = std::io::stdout().lock();
            let _ = write!(out, "{delta}");
            let _ = out.flush();
        })?;
```

Replace with:

```rust
        let spinner = crate::spinner::ShellSpinner::start();
        let spinner_state = Arc::clone(&spinner.state);
        let first_delta = std::sync::atomic::AtomicBool::new(true);

        let turn = provider.complete_streaming(&request, &|delta| {
            // On first delta, clear the spinner line and switch to streaming mode.
            if first_delta.swap(false, std::sync::atomic::Ordering::Relaxed) {
                spinner_state.phase.store(1, std::sync::atomic::Ordering::Relaxed);
                let mut ft = spinner_state.first_token_at.lock().unwrap();
                if ft.is_none() {
                    *ft = Some(std::time::Instant::now());
                }
                // Clear spinner line before first output
                eprint!("\r\x1b[2K");
            }
            // Estimate tokens: ~4 chars per token
            let estimated = (delta.len() as u64 + 3) / 4;
            spinner_state.tokens.fetch_add(estimated, std::sync::atomic::Ordering::Relaxed);
            let mut out = std::io::stdout().lock();
            let _ = write!(out, "{delta}");
            let _ = out.flush();
        })?;
```

- [ ] **Step 2: Add spinner finish after provider call**

Right after the `provider.complete_streaming` call and the `drop(_provider_span);` line, add:

```rust
        // Finish the spinner with cost info
        let turn_cost = if let Some(usage) = &turn.usage {
            estimate_cost(usage.input_tokens, usage.output_tokens, &cmd_ctx.model_name)
        } else {
            0.0
        };
        spinner.finish(turn_cost);
```

- [ ] **Step 3: Add `use std::time::Instant;` if not already imported**

Check the imports at the top of `shell.rs`. If `std::time::Instant` is not imported, the streaming callback references it. Since `spinner_state` is used directly in the closure, verify the import is available. The `std::sync::atomic::Ordering` and `std::time::Instant` are used inline-qualified in the closure so no new imports are needed.

- [ ] **Step 4: Build and run quick test**

```bash
cd /Users/broomva/broomva/core/life/arcan && cargo build -p arcan
```

Expected: compiles with no errors.

- [ ] **Step 5: Run E2E smoke to verify no regression**

```bash
cd /Users/broomva/broomva/core/life/arcan && bash scripts/e2e-smoke.sh
```

Expected: all tests pass (spinner is no-op in piped mode).

- [ ] **Step 6: Commit**

```bash
git add crates/arcan/src/shell.rs
git commit -m "feat(arcan): integrate spinner into provider call path"
```

---

### Task 5: Add tool execution spinner

**Files:**
- Modify: `crates/arcan/src/shell.rs`

- [ ] **Step 1: Add tool spinner around execute_tool calls**

In `shell.rs`, in the Phase 2 parallel tool execution section, find the single-tool path (around line 1581):

```rust
        let parallel_results: Vec<(usize, String, bool)> = if approved_indices.len() <= 1 {
            // Single tool — no threading overhead needed.
            approved_indices
                .iter()
                .map(|&i| {
                    let call = &tool_calls[i];
                    let (content, is_error) = execute_tool(registry, call, &ctx);
                    (i, content, is_error)
                })
                .collect()
```

Replace with:

```rust
        let parallel_results: Vec<(usize, String, bool)> = if approved_indices.len() <= 1 {
            // Single tool — no threading overhead needed.
            approved_indices
                .iter()
                .map(|&i| {
                    let call = &tool_calls[i];
                    let tool_spinner = crate::spinner::ShellSpinner::start_tool(&call.tool_name);
                    let (content, is_error) = execute_tool(registry, call, &ctx);
                    tool_spinner.finish_tool(!is_error);
                    (i, content, is_error)
                })
                .collect()
```

- [ ] **Step 2: Add tool spinner for parallel execution path**

In the parallel execution branch (the `std::thread::scope` block), find:

```rust
                    s.spawn(move || {
                        let (content, is_error) = execute_tool(registry, call, ctx_ref);
                        (i, content, is_error)
                    })
```

Replace with:

```rust
                    s.spawn(move || {
                        let tool_spinner = crate::spinner::ShellSpinner::start_tool(&call.tool_name);
                        let (content, is_error) = execute_tool(registry, call, ctx_ref);
                        tool_spinner.finish_tool(!is_error);
                        (i, content, is_error)
                    })
```

- [ ] **Step 3: Build and test**

```bash
cd /Users/broomva/broomva/core/life/arcan && cargo build -p arcan && bash scripts/e2e-smoke.sh
```

Expected: compiles, all E2E tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/arcan/src/shell.rs
git commit -m "feat(arcan): add tool execution spinner"
```

---

### Task 6: Implement message queuing via stdin reader thread

**Files:**
- Modify: `crates/arcan/src/shell.rs`

- [ ] **Step 1: Replace blocking stdin with mpsc channel**

In `shell.rs`, in the `run_shell` function, find the REPL loop section (around line 1015):

```rust
    // --- REPL loop ---
    let stdin = std::io::stdin();
    loop {
        eprint!("arcan> ");
        std::io::stderr().flush().ok();

        let mut line = String::new();
        match stdin.read_line(&mut line) {
            Ok(0) => break, // EOF
            Ok(_) => {}
            Err(e) => {
                eprintln!("read error: {e}");
                break;
            }
        }

        let input = line.trim();
```

Replace with:

```rust
    // --- Message queue: stdin reader thread (BRO-430) ---
    let (input_tx, input_rx) = std::sync::mpsc::channel::<Option<String>>();
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        loop {
            let mut line = String::new();
            match stdin.read_line(&mut line) {
                Ok(0) => {
                    let _ = input_tx.send(None); // EOF
                    break;
                }
                Ok(_) => {
                    let _ = input_tx.send(Some(line));
                }
                Err(_) => {
                    let _ = input_tx.send(None);
                    break;
                }
            }
        }
    });

    // --- REPL loop ---
    loop {
        eprint!("arcan> ");
        std::io::stderr().flush().ok();

        let line = match input_rx.recv() {
            Ok(Some(line)) => line,
            Ok(None) | Err(_) => break, // EOF or channel closed
        };

        let input = line.trim();
```

- [ ] **Step 2: Add queued message echo between turns**

After the response is processed (after the `match response_text { ... }` block and before the auto-compact section), add a drain loop. Find the closing brace of the auto-compact block and the end of the main loop. Inside the loop, after the auto-compact check, before the next iteration starts, the `eprint!("arcan> ")` at the top of the loop handles prompting naturally. When a queued message is ready, `input_rx.recv()` returns it immediately.

No additional drain code is needed — the mpsc channel buffers messages naturally and the loop consumes them one at a time. The `arcan>` prompt still prints each iteration, and the user sees their queued input echoed by the terminal itself (stdin echo).

- [ ] **Step 3: Build and test**

```bash
cd /Users/broomva/broomva/core/life/arcan && cargo build -p arcan && bash scripts/e2e-smoke.sh
```

Expected: compiles, all E2E tests pass (piped stdin works the same way through the channel).

- [ ] **Step 4: Commit**

```bash
git add crates/arcan/src/shell.rs
git commit -m "feat(arcan): add message queuing via stdin reader thread"
```

---

### Task 7: Full workspace verification and cleanup

**Files:**
- All modified files

- [ ] **Step 1: Run full workspace checks**

```bash
cd /Users/broomva/broomva/core/life/arcan && cargo fmt && cargo clippy --workspace -- -D warnings && cargo test --workspace
```

Expected: format clean, zero warnings, all tests pass.

- [ ] **Step 2: Run E2E smoke test**

```bash
cd /Users/broomva/broomva/core/life/arcan && bash scripts/e2e-smoke.sh
```

Expected: 45/45 (or more) passed, 0 failed.

- [ ] **Step 3: Manual verification with real provider**

```bash
cd /Users/broomva/broomva/core/life/arcan && printf 'What is 2+2? Answer in one word.\nUse bash to run: echo hello\n/cost\n' | cargo run --bin arcan -- shell --provider anthropic --budget 1.0 -y 2>&1 | head -40
```

Note: in piped mode the spinner is a no-op (expected). For full visual verification, run interactively:

```bash
cargo run --bin arcan -- shell --provider anthropic --budget 1.0
```

Then type a message and observe: spinning glyph, verb, elapsed time, then streaming output, then completion line with stats.

- [ ] **Step 4: Commit any final cleanup**

```bash
git add -A && git status
# Only commit if there are changes
git commit -m "chore(arcan): shell UX cleanup and formatting"
```
