//! Workflow trait + WorkflowExecutor — the user-implementation surface
//! and the executor that wraps lifecycle-hook firing around it.
//!
//! ## Where this fits
//!
//! - [`Workflow`] is **the trait a Broomva developer implements** when
//!   they want to ship an agent. Its `execute` body is plain async Rust
//!   and is the deterministic outer body of an agent run; it dispatches
//!   to the autonomous loop via [`crate::StepCtx::run_inference_streaming`].
//! - [`WorkflowExecutor`] is the **driver** the host runtime (arcan in
//!   production) calls. Its `run` method fires `on_workflow_start` hooks,
//!   calls `Workflow::execute`, then fires `on_workflow_end` hooks
//!   (always — even on error). It does **not** construct the
//!   [`crate::StepCtx`] or the [`crate::HookRegistry`]; those come from
//!   the host runtime, fully assembled, with auto-hooks already
//!   registered before any user hooks.
//!
//! ## Why the executor is so small
//!
//! Spec §3.8 originally proposed that `WorkflowExecutor` would
//! auto-register the four Life-native hooks. We deliberately moved that
//! responsibility out: the executor doesn't know about
//! `PraxisCapabilityHook` / `AutonomicBudgetHook` / `NousScoreHook` /
//! `AnimaAttestHook` (those live in `ergon-life-hooks`, BRO-1000), and
//! it doesn't construct the substrate handles those hooks need (those
//! come from arcan's `TickCtx` in BRO-1001). What's left for the
//! executor is just the workflow boundary: start hook, execute, end hook.
//!
//! Result: `WorkflowExecutor::run` is ~30 lines of business logic.
//! Reviewers can read it in one sitting; bugs have nowhere to hide.
//!
//! ## Spec deviations relative to §3.3 / §3.8
//!
//! 6. **`without_default_hooks` flag dropped** (spec §3.8 / §8 Q3). The
//!    executor doesn't construct any default hooks, so there's nothing to
//!    opt out of. Opt-out lives at the *adapter* level (BRO-1001) — the
//!    arcan adapter is the thing that decides which auto-hooks to add to
//!    the registry it passes into the executor.
//!
//! 7. **`StepCtx` is built by the caller, not the executor**. Spec §3.8's
//!    pseudocode showed the executor building the StepCtx + auto-hook
//!    registry. We invert that: caller passes a fully-built `StepCtx`
//!    (with hooks already in `ctx.hooks`); executor only fires the
//!    workflow boundary events. This separation lets workflows be tested
//!    with a hand-built `StepCtx` and lets the arcan adapter own
//!    substrate-handle assembly.

use crate::error::{ErgonError, Result};
use crate::hook::{HookCtx, HookOutcome};
use crate::role::Role;
use crate::step::StepCtx;
use async_trait::async_trait;
use std::sync::Arc;

// Note: `praxis_skills::SkillSet` is the canonical skill registry trait,
// but ergon today does not depend on `praxis-skills`. We use a tiny
// internal placeholder type that ships in this crate and behaves like
// "no skills". When BRO-1001 (arcan adapter) lands, workflows can return
// a real praxis SkillSet via the adapter — the trait shape is identical.
//
// This keeps ergon's "zero substrate deps" property absolute.

mod praxis_skills_stub {
    /// A minimal skill-set trait used by [`super::Workflow::skills`].
    ///
    /// **Placeholder for v0.1**: ergon doesn't depend on `praxis-skills`,
    /// so this trait lives here as a stub. The shape matches what
    /// `praxis_skills::SkillSet` exposes (read-only iteration). When
    /// BRO-1001 lands, workflows can return a real praxis SkillSet by
    /// implementing this trait against it.
    pub trait SkillSet: Send + Sync {
        /// Number of skills in the set. Default: 0.
        fn len(&self) -> usize {
            0
        }
        /// True iff [`Self::len`] is zero. Default: `true`.
        fn is_empty(&self) -> bool {
            self.len() == 0
        }
    }

    /// The empty skill set — default for workflows that don't consume skills.
    pub struct EmptySkillSet;

    impl SkillSet for EmptySkillSet {}
}

pub use praxis_skills_stub::{EmptySkillSet, SkillSet};

/// The user-implementation surface for an agent.
///
/// A `Workflow` is the *deterministic body* of an agent run. The host
/// runtime (arcan in production) calls `execute` once per session;
/// inside `execute`, the workflow author runs `Step`s via `ctx.step(...)`
/// and drives the autonomous loop via
/// `ctx.run_inference_streaming(...)`.
///
/// ## Typed input + output
///
/// Each workflow declares typed `Input` and `Output` associated types.
/// The host runtime serializes / deserializes via `serde_json` at the
/// boundary; inside `execute`, the workflow body works with strongly
/// typed values.
///
/// ## Provided defaults
///
/// - [`Self::role`] returns an empty [`Role`] by default (workflow has
///   no system-prompt overlay of its own).
/// - [`Self::skills`] returns an [`EmptySkillSet`] by default. Workflows
///   that consume skills override this.
/// - The workflow does NOT supply tools, sandbox, or providers — those
///   come from the [`StepCtx`] the host runtime hands to `execute`.
#[async_trait]
pub trait Workflow: Send + Sync + 'static {
    /// Typed input. Must round-trip through JSON (the host runtime
    /// deserializes from `serde_json::Value` at session start).
    type Input: for<'de> serde::Deserialize<'de> + Send;

    /// Typed output. Must serialize to JSON (the host runtime serializes
    /// to `serde_json::Value` at session end).
    type Output: serde::Serialize + Send;

    /// Stable, human-readable name used in lifecycle hooks, tracing,
    /// session events, and the lifed agent registry. Convention:
    /// dotted-path, e.g. `"bookkeeping.promotion-judge"`.
    fn name(&self) -> &str;

    /// Workflow-default [`Role`] overlay. Lowest precedence (agent
    /// scope); merged with session and call roles via [`Role::merge`].
    /// Defaults to an empty role.
    fn role(&self) -> Role {
        Role::default()
    }

    /// Skill set the workflow may consult. Defaults to empty.
    fn skills(&self) -> &dyn SkillSet {
        &EmptySkillSet
    }

    /// The deterministic orchestration body.
    ///
    /// Inside, the workflow author calls `ctx.step(...)` to dispatch
    /// sub-steps and `ctx.run_inference_streaming(...)` to drive the
    /// autonomous model+tool loop. Plain async Rust — branching, looping,
    /// and error handling are all available.
    async fn execute(&self, ctx: &mut StepCtx<'_>, input: Self::Input) -> Result<Self::Output>;
}

/// The driver that fires lifecycle hooks around a [`Workflow::execute`]
/// call.
///
/// Construction is trivial: `WorkflowExecutor::new(Arc::new(my_workflow))`.
/// The host runtime (arcan in production) builds the [`StepCtx`] —
/// including the [`crate::HookRegistry`] with auto-hooks pre-registered —
/// and passes both to [`Self::run`].
///
/// ## Lifecycle (per spec §5)
///
/// ```text
/// for hook in ctx.hooks {
///     match hook.on_workflow_start(...).await {
///         Continue => {},
///         Deny(r)  => return Err(ErgonError::Hook(...)),
///     }
/// }
/// let result = workflow.execute(ctx, input).await;
/// for hook in ctx.hooks {
///     // best-effort, even on error
///     let _ = hook.on_workflow_end(..., result.is_ok()).await;
/// }
/// result
/// ```
pub struct WorkflowExecutor<W: Workflow> {
    workflow: Arc<W>,
}

impl<W: Workflow> WorkflowExecutor<W> {
    /// Construct an executor wrapping the given workflow.
    pub fn new(workflow: Arc<W>) -> Self {
        Self { workflow }
    }

    /// Read-only handle to the wrapped workflow (mostly useful for
    /// inspecting `name()` from the host runtime).
    pub fn workflow(&self) -> &Arc<W> {
        &self.workflow
    }

    /// Run the workflow against the given context + input.
    ///
    /// Fires `on_workflow_start` hooks (Deny short-circuits with
    /// [`ErgonError::Hook`]), calls `Workflow::execute`, then fires
    /// `on_workflow_end` hooks (always — even on error; `ok` reflects
    /// whether `execute` returned `Ok`).
    ///
    /// `on_workflow_end` hook errors are best-effort: they're logged via
    /// `tracing` but do not change the executor's return value.
    pub async fn run(&self, ctx: &mut StepCtx<'_>, input: W::Input) -> Result<W::Output> {
        // Snapshot the registry up front. We need to release the borrow
        // on `ctx` before calling `workflow.execute(ctx, _)`, and Arc
        // keeps the registry alive across that handoff.
        let hooks = Arc::clone(&ctx.hooks);

        // Fire on_workflow_start. First Deny short-circuits the run.
        {
            let hook_ctx = HookCtx::new(ctx.session_id.clone(), ctx.workflow_name, &ctx.trace);
            for hook in hooks.iter() {
                match hook.on_workflow_start(&hook_ctx).await? {
                    HookOutcome::Continue => {}
                    HookOutcome::Deny(reason) => {
                        return Err(ErgonError::Hook(format!(
                            "workflow `{}` denied by {}: {reason}",
                            ctx.workflow_name,
                            hook.name()
                        )));
                    }
                }
            }
        }

        // Execute the workflow body.
        let result = self.workflow.execute(ctx, input).await;
        let ok = result.is_ok();

        // Fire on_workflow_end ALWAYS — even on error. Hook errors are
        // logged but do not override the workflow result.
        {
            let hook_ctx = HookCtx::new(ctx.session_id.clone(), ctx.workflow_name, &ctx.trace);
            for hook in hooks.iter() {
                if let Err(err) = hook.on_workflow_end(&hook_ctx, ok).await {
                    tracing::warn!(
                        parent: &ctx.trace,
                        workflow = ctx.workflow_name,
                        hook = hook.name(),
                        error = %err,
                        "on_workflow_end hook failed (non-fatal)",
                    );
                }
            }
        }

        result
    }
}

impl<W: Workflow> std::fmt::Debug for WorkflowExecutor<W> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkflowExecutor")
            .field("workflow", &self.workflow.name())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SessionId;
    use crate::hook::{Hook, HookRegistry, InferenceHookOutcome, ToolHookOutcome};
    use crate::model::{ModelRequest, ModelResponse, ToolCall, ToolResult};
    use crate::runtime::{Provider, RuntimeHandle, ToolRegistry};
    use crate::stream::{BufferSink, StreamSink};
    use aios_protocol::mode::OperatingMode;
    use serde::{Deserialize, Serialize};
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};

    // ── Trivial mocks (the executor itself doesn't drive the loop, so
    //    these can be no-ops) ────────────────────────────────────────

    struct NoopProvider;
    #[async_trait]
    impl Provider for NoopProvider {
        fn name(&self) -> &str {
            "noop"
        }
        async fn stream(
            &self,
            _req: ModelRequest,
            _sink: Arc<dyn StreamSink>,
        ) -> Result<ModelResponse> {
            unreachable!("workflow body for executor tests does not call provider")
        }
    }

    struct NoopTools;
    #[async_trait]
    impl ToolRegistry for NoopTools {
        fn definitions(&self) -> Vec<crate::model::ToolDefinition> {
            Vec::new()
        }
        async fn invoke(&self, _call: ToolCall) -> Result<ToolResult> {
            unreachable!("workflow body for executor tests does not invoke tools")
        }
    }

    struct NoopRuntime;
    impl RuntimeHandle for NoopRuntime {
        fn operating_mode(&self) -> OperatingMode {
            OperatingMode::Execute
        }
    }

    // ── Workflow + Hook helpers ─────────────────────────────────────

    #[derive(Deserialize)]
    struct EchoInput {
        text: String,
    }

    #[derive(Serialize, PartialEq, Debug)]
    struct EchoOutput {
        text: String,
    }

    /// Workflow whose body just upper-cases the input text.
    struct EchoWorkflow {
        name: &'static str,
    }

    #[async_trait]
    impl Workflow for EchoWorkflow {
        type Input = EchoInput;
        type Output = EchoOutput;

        fn name(&self) -> &str {
            self.name
        }

        async fn execute(&self, _ctx: &mut StepCtx<'_>, input: EchoInput) -> Result<EchoOutput> {
            Ok(EchoOutput {
                text: input.text.to_uppercase(),
            })
        }
    }

    /// Workflow that always errors.
    struct FailingWorkflow;

    #[async_trait]
    impl Workflow for FailingWorkflow {
        type Input = EchoInput;
        type Output = EchoOutput;

        fn name(&self) -> &str {
            "failing"
        }

        async fn execute(&self, _ctx: &mut StepCtx<'_>, _input: EchoInput) -> Result<EchoOutput> {
            Err(ErgonError::workflow("intentional failure"))
        }
    }

    /// Hook recording start/end calls and supporting deny-on-start.
    struct LifecycleRecorder {
        name: &'static str,
        deny_start: AtomicBool,
        events: Mutex<Vec<String>>,
    }

    impl LifecycleRecorder {
        fn new(name: &'static str) -> Self {
            Self {
                name,
                deny_start: AtomicBool::new(false),
                events: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl Hook for LifecycleRecorder {
        fn name(&self) -> &str {
            self.name
        }

        async fn on_workflow_start(&self, _ctx: &HookCtx<'_>) -> Result<HookOutcome> {
            self.events
                .lock()
                .expect("lock")
                .push(format!("start:{}", self.name));
            if self.deny_start.load(Ordering::Relaxed) {
                Ok(HookOutcome::Deny(format!("{} blocked", self.name)))
            } else {
                Ok(HookOutcome::Continue)
            }
        }

        async fn on_workflow_end(&self, _ctx: &HookCtx<'_>, ok: bool) -> Result<HookOutcome> {
            self.events
                .lock()
                .expect("lock")
                .push(format!("end:{}:{ok}", self.name));
            Ok(HookOutcome::Continue)
        }

        async fn on_step_start(&self, _: &HookCtx<'_>, _: &str) -> Result<HookOutcome> {
            Ok(HookOutcome::Continue)
        }
        async fn on_step_end(&self, _: &HookCtx<'_>, _: &str, _: bool) -> Result<HookOutcome> {
            Ok(HookOutcome::Continue)
        }
        async fn on_pre_inference(
            &self,
            _: &HookCtx<'_>,
            _: &mut ModelRequest,
        ) -> Result<InferenceHookOutcome> {
            Ok(InferenceHookOutcome::Continue)
        }
        async fn on_post_inference(
            &self,
            _: &HookCtx<'_>,
            _: &ModelResponse,
        ) -> Result<HookOutcome> {
            Ok(HookOutcome::Continue)
        }
        async fn on_pre_tool_use(
            &self,
            _: &HookCtx<'_>,
            _: &mut ToolCall,
        ) -> Result<ToolHookOutcome> {
            Ok(ToolHookOutcome::Continue)
        }
        async fn on_post_tool_use(
            &self,
            _: &HookCtx<'_>,
            _: &ToolCall,
            _: &mut ToolResult,
        ) -> Result<HookOutcome> {
            Ok(HookOutcome::Continue)
        }
    }

    fn build_ctx<'a>(hooks: HookRegistry) -> StepCtx<'a> {
        StepCtx::new(
            SessionId::from_string("01ARZ3NDEKTSV4RRFFQ69G5FAV"),
            "test-wf",
            Arc::new(NoopProvider),
            Arc::new(NoopTools),
            Arc::new(hooks),
            Arc::new(BufferSink::new()),
            Arc::new(NoopRuntime),
            tracing::Span::current(),
        )
    }

    // ── Tests ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn run_with_no_hooks_executes_and_returns_output() {
        let exec = WorkflowExecutor::new(Arc::new(EchoWorkflow { name: "echo" }));
        let mut ctx = build_ctx(HookRegistry::new());
        let out = exec
            .run(
                &mut ctx,
                EchoInput {
                    text: "hello".into(),
                },
            )
            .await
            .expect("ok");
        assert_eq!(
            out,
            EchoOutput {
                text: "HELLO".into()
            }
        );
    }

    #[tokio::test]
    async fn workflow_start_hook_fires_before_execute() {
        let recorder = Arc::new(LifecycleRecorder::new("rec"));
        let hooks = HookRegistry::new().with_arc(recorder.clone() as Arc<dyn Hook>);
        let exec = WorkflowExecutor::new(Arc::new(EchoWorkflow { name: "echo" }));
        let mut ctx = build_ctx(hooks);
        let _ = exec
            .run(&mut ctx, EchoInput { text: "hi".into() })
            .await
            .expect("ok");
        let events = recorder.events.lock().expect("lock").clone();
        assert_eq!(events, vec!["start:rec".to_string(), "end:rec:true".into()]);
    }

    #[tokio::test]
    async fn workflow_start_deny_short_circuits() {
        let recorder = Arc::new(LifecycleRecorder::new("strict"));
        recorder.deny_start.store(true, Ordering::Relaxed);
        let hooks = HookRegistry::new().with_arc(recorder.clone() as Arc<dyn Hook>);
        let exec = WorkflowExecutor::new(Arc::new(EchoWorkflow { name: "echo" }));
        let mut ctx = build_ctx(hooks);
        let err = exec
            .run(&mut ctx, EchoInput { text: "hi".into() })
            .await
            .expect_err("denied");
        match err {
            ErgonError::Hook(msg) => {
                assert!(msg.contains("denied"));
                assert!(msg.contains("strict"));
            }
            other => panic!("expected Hook, got {other:?}"),
        }
        // Only `start` event recorded — execute was not reached, but the
        // executor MUST still fire on_workflow_end? No — on Deny, the
        // workflow never started, so end does NOT fire.
        let events = recorder.events.lock().expect("lock").clone();
        assert_eq!(events, vec!["start:strict".to_string()]);
    }

    #[tokio::test]
    async fn workflow_end_fires_even_on_execute_error() {
        let recorder = Arc::new(LifecycleRecorder::new("rec"));
        let hooks = HookRegistry::new().with_arc(recorder.clone() as Arc<dyn Hook>);
        let exec = WorkflowExecutor::new(Arc::new(FailingWorkflow));
        let mut ctx = build_ctx(hooks);
        let err = exec
            .run(&mut ctx, EchoInput { text: "x".into() })
            .await
            .expect_err("execute fails");
        assert!(matches!(err, ErgonError::Workflow(_)));
        // Both start and end MUST fire; end carries ok=false.
        let events = recorder.events.lock().expect("lock").clone();
        assert_eq!(
            events,
            vec!["start:rec".to_string(), "end:rec:false".into()]
        );
    }

    #[tokio::test]
    async fn first_deny_short_circuits_subsequent_hooks() {
        let a = Arc::new(LifecycleRecorder::new("a"));
        a.deny_start.store(true, Ordering::Relaxed);
        let b = Arc::new(LifecycleRecorder::new("b"));
        let hooks = HookRegistry::new()
            .with_arc(a.clone() as Arc<dyn Hook>)
            .with_arc(b.clone() as Arc<dyn Hook>);
        let exec = WorkflowExecutor::new(Arc::new(EchoWorkflow { name: "echo" }));
        let mut ctx = build_ctx(hooks);
        let _ = exec
            .run(&mut ctx, EchoInput { text: "x".into() })
            .await
            .expect_err("denied");
        // a fired (and denied); b never saw on_workflow_start.
        assert_eq!(
            a.events.lock().expect("lock").clone(),
            vec!["start:a".to_string()]
        );
        assert!(b.events.lock().expect("lock").is_empty());
    }

    #[tokio::test]
    async fn end_hook_failure_does_not_override_workflow_result() {
        struct FailingEnd;
        #[async_trait]
        impl Hook for FailingEnd {
            fn name(&self) -> &str {
                "failing-end"
            }
            async fn on_workflow_end(&self, _: &HookCtx<'_>, _: bool) -> Result<HookOutcome> {
                Err(ErgonError::internal("end hook boom"))
            }
            async fn on_workflow_start(&self, _: &HookCtx<'_>) -> Result<HookOutcome> {
                Ok(HookOutcome::Continue)
            }
            async fn on_step_start(&self, _: &HookCtx<'_>, _: &str) -> Result<HookOutcome> {
                Ok(HookOutcome::Continue)
            }
            async fn on_step_end(&self, _: &HookCtx<'_>, _: &str, _: bool) -> Result<HookOutcome> {
                Ok(HookOutcome::Continue)
            }
            async fn on_pre_inference(
                &self,
                _: &HookCtx<'_>,
                _: &mut ModelRequest,
            ) -> Result<InferenceHookOutcome> {
                Ok(InferenceHookOutcome::Continue)
            }
            async fn on_post_inference(
                &self,
                _: &HookCtx<'_>,
                _: &ModelResponse,
            ) -> Result<HookOutcome> {
                Ok(HookOutcome::Continue)
            }
            async fn on_pre_tool_use(
                &self,
                _: &HookCtx<'_>,
                _: &mut ToolCall,
            ) -> Result<ToolHookOutcome> {
                Ok(ToolHookOutcome::Continue)
            }
            async fn on_post_tool_use(
                &self,
                _: &HookCtx<'_>,
                _: &ToolCall,
                _: &mut ToolResult,
            ) -> Result<HookOutcome> {
                Ok(HookOutcome::Continue)
            }
        }

        let hooks = HookRegistry::new().with(FailingEnd);
        let exec = WorkflowExecutor::new(Arc::new(EchoWorkflow { name: "echo" }));
        let mut ctx = build_ctx(hooks);
        // Despite the end hook erroring, the workflow's Ok result should
        // surface to the caller.
        let out = exec
            .run(&mut ctx, EchoInput { text: "ok".into() })
            .await
            .expect("workflow result wins");
        assert_eq!(out.text, "OK");
    }

    #[test]
    fn empty_skill_set_default_is_empty_and_zero_len() {
        let s = EmptySkillSet;
        assert_eq!(s.len(), 0);
        assert!(s.is_empty());
    }

    #[test]
    fn workflow_default_skills_returns_empty() {
        struct W;
        #[async_trait]
        impl Workflow for W {
            type Input = ();
            type Output = ();
            fn name(&self) -> &str {
                "w"
            }
            async fn execute(&self, _: &mut StepCtx<'_>, _: ()) -> Result<()> {
                Ok(())
            }
        }
        let w = W;
        assert_eq!(w.skills().len(), 0);
        assert!(w.skills().is_empty());
    }
}
