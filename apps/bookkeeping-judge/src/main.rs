//! CLI binary for the bookkeeping-judge ergon Workflow.
//!
//! ## Surface
//!
//! Mirrors the bellows reference binary: reads `JudgeInput` JSON from
//! stdin, writes `JudgeOutput` JSON to stdout, traces to stderr.
//!
//! ```bash
//! ANTHROPIC_API_KEY=sk-ant-... \
//!   echo '{"extract_path":"/abs/path/to/raw.md","max_items":20}' \
//!     | bookkeeping-judge
//! ```
//!
//! `BOOKKEEPING_AGENTS_DIR` overrides the agent registry directory
//! (default: `<workspace>/agents/`, derived from CARGO_MANIFEST_DIR
//! in this binary).
//!
//! The provider chain is the same one
//! `tests/anthropic_agents_smoke.rs` (BRO-1013) uses:
//! `arcan_provider::AnthropicProvider` → `ArcanProviderAdapter`
//! (`aios_protocol::ModelProviderPort`) → `arcan_ergon::ModelProviderAdapter`
//! (`ergon::Provider`).

use std::io::Read;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use bookkeeping_judge::{BookkeepingJudge, JudgeInput};

fn main() -> ExitCode {
    init_tracing();

    // Step 1 — drain stdin synchronously before entering any tokio
    // runtime (the bellows reference does the same; matches the
    // arcan-ergon anthropic-workflow pattern).
    let mut buf = String::new();
    if std::io::stdin().read_to_string(&mut buf).is_err() {
        eprintln!("[bookkeeping-judge] could not read stdin");
        return ExitCode::from(2);
    }
    if buf.trim().is_empty() {
        eprintln!("[bookkeeping-judge] no JSON input on stdin");
        return ExitCode::from(2);
    }

    let input: JudgeInput = match serde_json::from_str(&buf) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("[bookkeeping-judge] invalid JSON input: {e}");
            return ExitCode::from(2);
        }
    };

    let agents_dir = resolve_agents_dir();
    eprintln!(
        "[bookkeeping-judge] agents dir: {} (override with BOOKKEEPING_AGENTS_DIR)",
        agents_dir.display()
    );

    // Step 2 — Build the provider chain in sync context (avoids
    // dropping reqwest::blocking's inner runtime from inside an outer
    // tokio runtime, which panics).
    let provider_chain = match cli::build_provider_chain() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("[bookkeeping-judge] could not build provider chain: {e}");
            return ExitCode::from(3);
        }
    };

    // Step 3 — Build the judge.
    let judge = match BookkeepingJudge::new(agents_dir.clone()) {
        Ok(j) => Arc::new(j),
        Err(e) => {
            eprintln!(
                "[bookkeeping-judge] could not load agents from {}: {e}",
                agents_dir.display()
            );
            return ExitCode::from(3);
        }
    };

    // Step 4 — Run inside a manually-built tokio runtime.
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[bookkeeping-judge] could not build tokio runtime: {e}");
            return ExitCode::from(3);
        }
    };

    let out = runtime.block_on(cli::run_judge(judge, provider_chain, input));

    match out {
        Ok(output_json) => {
            println!("{output_json}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("[bookkeeping-judge] judge run failed: {e}");
            ExitCode::from(1)
        }
    }
}

fn resolve_agents_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("BOOKKEEPING_AGENTS_DIR") {
        return PathBuf::from(dir);
    }
    // CARGO_MANIFEST_DIR points to apps/bookkeeping-judge/; the
    // workspace agents/ dir is two levels up.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("agents")
}

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                tracing_subscriber::EnvFilter::new("info,bookkeeping_judge=info")
            }),
        )
        .with_writer(std::io::stderr)
        .compact()
        .try_init();
}

mod cli {
    //! Provider-chain assembly + judge-driving glue. Mirrors
    //! `arcan-ergon/tests/anthropic_agents_smoke.rs::build_ergon_provider`
    //! so the live behavior is byte-identical to the smoke test the
    //! authored agents were validated through.
    use std::sync::Arc;

    use aios_protocol::{BranchId, ModelProviderPort, RunId, SessionId};
    use arcan_aios_adapters::ArcanProviderAdapter;
    use arcan_core::runtime::Provider as ArcanProvider;
    use arcan_ergon::ModelProviderAdapter;
    use arcan_provider::anthropic::{AnthropicConfig, AnthropicProvider};
    use async_trait::async_trait;
    use ergon::{
        BufferSink, ErgonError, HookRegistry, Provider, SessionId as ErgonSessionId, StepCtx,
        StreamSink, ToolCall, ToolDefinition, ToolRegistry, ToolResult,
    };

    use bookkeeping_judge::{BookkeepingJudge, JudgeInput};

    /// Mirrors the bellows reference: empty registry (only the
    /// authored agents' synthesized `record_answer` tool is in scope).
    #[derive(Default)]
    pub(crate) struct EmptyTools;

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

    pub(crate) struct ExecuteRuntime;

    impl ergon::RuntimeHandle for ExecuteRuntime {
        fn operating_mode(&self) -> aios_protocol::mode::OperatingMode {
            aios_protocol::mode::OperatingMode::Execute
        }
    }

    /// Returns the assembled `ergon::Provider` chain, built in sync
    /// context (see header comment on reqwest::blocking).
    pub(crate) fn build_provider_chain() -> Result<Arc<dyn Provider>, String> {
        let config = AnthropicConfig::from_env()
            .map_err(|e| format!("AnthropicConfig::from_env failed (ANTHROPIC_API_KEY?): {e}"))?;
        let arcan_provider: Arc<dyn ArcanProvider> = Arc::new(AnthropicProvider::new(config));

        // ArcanProviderAdapter wants a tools list (empty — we only use
        // the per-agent synthesized record_answer tool) and a streaming
        // sender slot (None — no broadcast subscriber here).
        let streaming_sender = Arc::new(std::sync::Mutex::new(None));
        let port: Arc<dyn ModelProviderPort> = Arc::new(ArcanProviderAdapter::new(
            arcan_provider,
            Vec::new(),
            streaming_sender,
        ));

        Ok(Arc::new(ModelProviderAdapter::new(
            port,
            SessionId::from_string("bookkeeping-judge-cli".to_string()),
            BranchId::main(),
            RunId::new_uuid(),
            "anthropic-cli",
        )))
    }

    /// Driver: build the StepCtx, hand it to the workflow, serialize
    /// the typed output to JSON.
    ///
    /// `workflow_name_static` must outlive the StepCtx — we
    /// `Box::leak` the workflow name in the caller (CLI runs once and
    /// exits, so the leak is bounded by process lifetime).
    pub(crate) async fn run_judge(
        judge: Arc<BookkeepingJudge>,
        provider: Arc<dyn Provider>,
        input: JudgeInput,
    ) -> Result<String, ErgonError> {
        use ergon::Workflow;

        // `&'static str` is acceptable as `&'a str` for any `'a`. We
        // leak the workflow name once — the binary runs the judge then
        // exits, so the leak is process-bounded.
        let name: &'static str = Box::leak(judge.name().to_string().into_boxed_str());
        let mut ctx = StepCtx::new(
            ErgonSessionId::default(),
            name,
            provider,
            Arc::new(EmptyTools) as Arc<dyn ToolRegistry>,
            Arc::new(HookRegistry::default()),
            Arc::new(BufferSink::new()) as Arc<dyn StreamSink>,
            Arc::new(ExecuteRuntime) as Arc<dyn ergon::RuntimeHandle>,
            tracing::Span::current(),
        );

        let output = judge.execute(&mut ctx, input).await?;
        serde_json::to_string(&output)
            .map_err(|e| ErgonError::Workflow(format!("could not serialize JudgeOutput: {e}")))
    }
}
