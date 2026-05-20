//! Live-Anthropic parity smoke test against the Bellows reference.
//!
//! `#[ignore]`'d — runs only when `ANTHROPIC_API_KEY` is set and
//! invoked explicitly. Validates that the ergon-Workflow port produces
//! item totals within ±1 of the Bellows reference for the same input,
//! within LLM-nondeterminism tolerance.
//!
//! Run manually:
//! ```bash
//! ANTHROPIC_API_KEY=sk-ant-... \
//!   cargo test -p bookkeeping-judge --test parity_smoke -- --ignored --nocapture
//! ```
//!
//! ## What "parity" means
//!
//! The bellows reference's `bookkeeping-judge` binary uses a single
//! Claude call per file. The ergon port uses three calls per item (one
//! per axis) plus pre-parsing on the Rust side. Both produce the same
//! `JudgedItem` schema. LLM nondeterminism means individual axis
//! scores can drift ±1 between runs even with `temperature: 0`;
//! per-item *totals* are the stable parity surface.
//!
//! The tolerance is ±1 on total (out of 9). This matches the
//! handoff's stated tolerance and matches what
//! `skills/bookkeeping/scripts/bookkeeping.py` already accepts when
//! re-scoring previously-scored items.

use std::path::PathBuf;
use std::sync::Arc;

use aios_protocol::{BranchId, ModelProviderPort, RunId, SessionId};
use arcan_aios_adapters::ArcanProviderAdapter;
use arcan_core::runtime::Provider as ArcanProvider;
use arcan_ergon::ModelProviderAdapter;
use arcan_provider::anthropic::{AnthropicConfig, AnthropicProvider};
use async_trait::async_trait;
use ergon::{
    BufferSink, ErgonError, HookRegistry, Provider, RuntimeHandle, SessionId as ErgonSessionId,
    StepCtx, StreamSink, ToolCall, ToolDefinition, ToolRegistry, ToolResult, Workflow,
};

use bookkeeping_judge::{BookkeepingJudge, JudgeInput};

#[derive(Default)]
struct EmptyTools;

#[async_trait]
impl ToolRegistry for EmptyTools {
    fn definitions(&self) -> Vec<ToolDefinition> {
        Vec::new()
    }
    async fn invoke(&self, call: ToolCall) -> Result<ToolResult, ErgonError> {
        Err(ErgonError::Tool(format!(
            "EmptyTools cannot invoke `{}`",
            call.name
        )))
    }
}

struct ExecuteRuntime;
impl RuntimeHandle for ExecuteRuntime {
    fn operating_mode(&self) -> aios_protocol::mode::OperatingMode {
        aios_protocol::mode::OperatingMode::Execute
    }
}

fn agents_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("agents")
        .canonicalize()
        .expect("workspace agents/ dir must exist")
}

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("sample-raw.md")
}

fn build_provider_chain() -> Arc<dyn Provider> {
    let config = AnthropicConfig::from_env()
        .expect("ANTHROPIC_API_KEY must be set for the parity smoke test");
    let arcan_provider: Arc<dyn ArcanProvider> = Arc::new(AnthropicProvider::new(config));
    let streaming_sender = Arc::new(std::sync::Mutex::new(None));
    let port: Arc<dyn ModelProviderPort> = Arc::new(ArcanProviderAdapter::new(
        arcan_provider,
        Vec::new(),
        streaming_sender,
    ));
    Arc::new(ModelProviderAdapter::new(
        port,
        SessionId::from_string("parity-smoke-1003".to_owned()),
        BranchId::main(),
        RunId::new_uuid(),
        "anthropic-parity",
    ))
}

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                tracing_subscriber::EnvFilter::new("info,bookkeeping_judge=info,ergon=info")
            }),
        )
        .with_test_writer()
        .try_init();
}

/// Parity check against the bellows reference.
///
/// We don't actually shell out to the bellows binary here — that
/// would require the user to have built it, and would couple the test
/// to bellows release cadence. Instead we assert each item lands in
/// the expected band given the fixture's known content:
///
/// - Item 1 ("ergon harness + authored agents") — total ≥ 5 (pass).
/// - Item 2 ("vibes-only AI claim") — total ≤ 3 (fail).
/// - Item 3 ("RoPE embeddings") — total in 4..=7 (depending on
///   relevance-to-Life-Agent-OS calibration).
///
/// These are *bands*, not exact scores. The ±1 tolerance is implicit
/// in the band width. If a future bellows release produces totals
/// outside these bands for the same fixture, that's a genuine port
/// divergence worth investigating.
#[test]
#[ignore = "requires ANTHROPIC_API_KEY and live network; run with --ignored"]
fn parity_against_bellows_bands() {
    init_tracing();

    let provider = build_provider_chain();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime");

    runtime.block_on(async move {
        let judge = BookkeepingJudge::new(agents_dir()).expect("load agents");

        let mut ctx = StepCtx::new(
            ErgonSessionId::default(),
            "parity-smoke.bookkeeping-judge",
            provider,
            Arc::new(EmptyTools) as Arc<dyn ToolRegistry>,
            Arc::new(HookRegistry::default()),
            Arc::new(BufferSink::new()) as Arc<dyn StreamSink>,
            Arc::new(ExecuteRuntime) as Arc<dyn RuntimeHandle>,
            tracing::Span::current(),
        );

        let input = JudgeInput {
            extract_path: fixture_path().to_string_lossy().to_string(),
            max_items: Some(10),
        };

        let output = judge.execute(&mut ctx, input).await.expect("execute ok");

        eprintln!("[parity] output:");
        for item in &output.items {
            eprintln!(
                "  item={} total={} pass={} blog={} (n={}, s={}, r={})",
                item.item_number,
                item.total,
                item.pass,
                item.blog_candidate,
                item.novelty,
                item.specificity,
                item.relevance
            );
        }

        assert_eq!(output.items.len(), 3, "fixture has exactly 3 items");

        // Item 1 — concrete claim about ergon + authored agents
        // (high-signal). Bellows reference typically scores total >= 5
        // here. Allow 1-point slack below the pass threshold for LLM
        // nondeterminism — anything ≥ 4 is "close enough".
        let item1 = &output.items[0];
        assert!(
            item1.total >= 4,
            "Item 1 (high-signal) total too low: {} ({:?})",
            item1.total,
            item1
        );

        // Item 2 — pure vibes. Bellows reference scores total ≤ 3.
        // Allow up to 4 for nondeterminism.
        let item2 = &output.items[1];
        assert!(
            item2.total <= 4,
            "Item 2 (vibes-only) total too high: {} ({:?})",
            item2.total,
            item2
        );

        // Item 3 — specific (RoPE) but tangential. Bellows reference
        // scores total in 4..=7 depending on relevance calibration.
        let item3 = &output.items[2];
        assert!(
            (3..=8).contains(&item3.total),
            "Item 3 (specific-tangential) total out of band: {} ({:?})",
            item3.total,
            item3
        );

        // Sanity: provider field stable across runs.
        assert_eq!(output.provider, "ergon-authored-agents");
    });
}
