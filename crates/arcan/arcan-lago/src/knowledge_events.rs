//! Knowledge lifecycle event helpers and runtime observability middleware.
//!
//! The legacy helpers emit Lago `Custom("knowledge.*")` events. The
//! `KnowledgeEventMiddleware` derives typed `aios-protocol` events from the
//! kernel's canonical tool-completion events so knowledge observability lands
//! on the same event stream as the rest of the turn.

use std::sync::Arc;

use aios_protocol::{
    BranchId as KernelBranchId, EventKind, EventRecord, EventStorePort,
    SessionId as KernelSessionId, SpanStatus,
};
use aios_runtime::{TickOutput, TurnContext, TurnMiddleware, TurnNext};
use anyhow::Result;
use async_trait::async_trait;
use lago_core::event::{EventEnvelope, EventPayload};
use lago_core::id::*;
use serde_json::json;
use tracing::warn;

/// Create an event recording that a knowledge index was built/rebuilt.
pub fn knowledge_indexed_event(
    session_id: &SessionId,
    branch_id: &BranchId,
    seq: u64,
    note_count: usize,
    health_score: f32,
) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId::new(),
        session_id: session_id.clone(),
        branch_id: branch_id.clone(),
        seq,
        timestamp: now_micros(),
        run_id: None,
        parent_id: None,
        metadata: Default::default(),
        schema_version: 1,
        payload: EventPayload::Custom {
            event_type: "knowledge.indexed".to_string(),
            data: json!({
                "note_count": note_count,
                "health_score": health_score,
            }),
        },
    }
}

/// Create an event recording a knowledge search query.
pub fn knowledge_searched_event(
    session_id: &SessionId,
    branch_id: &BranchId,
    seq: u64,
    query: &str,
    result_count: usize,
    top_score: f64,
) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId::new(),
        session_id: session_id.clone(),
        branch_id: branch_id.clone(),
        seq,
        timestamp: now_micros(),
        run_id: None,
        parent_id: None,
        metadata: Default::default(),
        schema_version: 1,
        payload: EventPayload::Custom {
            event_type: "knowledge.searched".to_string(),
            data: json!({
                "query": query,
                "result_count": result_count,
                "top_score": top_score,
            }),
        },
    }
}

fn now_micros() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64
}

/// Runtime middleware that derives typed knowledge events from tool-completion events.
///
/// This preserves the purity of the tool trait while still emitting
/// first-class `Knowledge*` events in the active kernel runtime.
pub struct KnowledgeEventMiddleware {
    event_store: Arc<dyn EventStorePort>,
    read_limit: usize,
}

impl KnowledgeEventMiddleware {
    pub fn new(event_store: Arc<dyn EventStorePort>) -> Self {
        Self {
            event_store,
            read_limit: 512,
        }
    }

    #[allow(dead_code)]
    pub fn with_read_limit(mut self, read_limit: usize) -> Self {
        self.read_limit = read_limit.max(1);
        self
    }

    async fn append_kind(
        &self,
        session_id: &KernelSessionId,
        branch_id: &KernelBranchId,
        kind: EventKind,
    ) -> Result<()> {
        let next_seq = self
            .event_store
            .head(session_id.clone(), branch_id.clone())
            .await?
            .saturating_add(1);
        let record = EventRecord::new(session_id.clone(), branch_id.clone(), next_seq, kind);
        self.event_store.append(record).await?;
        Ok(())
    }
}

#[async_trait]
impl TurnMiddleware for KnowledgeEventMiddleware {
    async fn process(&self, ctx: &mut TurnContext, next: TurnNext<'_>) -> Result<TickOutput> {
        let start_seq = self
            .event_store
            .head(ctx.session_id.clone(), ctx.branch_id.clone())
            .await?;

        let mut output = next.run(ctx).await?;
        let records = self
            .event_store
            .read(
                ctx.session_id.clone(),
                ctx.branch_id.clone(),
                start_seq.saturating_add(1),
                self.read_limit,
            )
            .await?;

        let mut appended = 0_u64;
        for record in records {
            for kind in derive_knowledge_events(&record.kind) {
                if let Err(error) = self
                    .append_kind(&ctx.session_id, &ctx.branch_id, kind)
                    .await
                {
                    warn!(
                        session_id = %ctx.session_id,
                        branch = %ctx.branch_id,
                        error = %error,
                        "failed to append typed knowledge event"
                    );
                } else {
                    appended += 1;
                }
            }
        }

        output.events_emitted += appended;
        output.last_sequence += appended;
        Ok(output)
    }
}

fn derive_knowledge_events(kind: &EventKind) -> Vec<EventKind> {
    let EventKind::ToolCallCompleted {
        tool_name,
        result,
        status,
        ..
    } = kind
    else {
        return Vec::new();
    };

    if *status != SpanStatus::Ok {
        return Vec::new();
    }

    let Some(output) = successful_output(result) else {
        return Vec::new();
    };

    match tool_name.as_str() {
        "wiki_search" => derive_search_events(output),
        "wiki_lint" => derive_lint_events(output),
        _ => Vec::new(),
    }
}

fn successful_output(result: &serde_json::Value) -> Option<&serde_json::Value> {
    (result.get("status").and_then(serde_json::Value::as_str) == Some("success"))
        .then(|| result.get("output"))
        .flatten()
}

fn derive_search_events(output: &serde_json::Value) -> Vec<EventKind> {
    let query = output
        .get("query")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let result_count = output
        .get("count")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0) as u32;
    let top_relevance = output
        .get("top_relevance")
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(0.0);
    let duration_ms = output
        .get("duration_ms")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let context_tokens = output
        .get("context_tokens")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0) as u32;

    let mut events = vec![EventKind::KnowledgeSearched {
        query,
        result_count,
        top_relevance,
        duration_ms,
    }];

    if result_count > 0 {
        events.push(EventKind::KnowledgeRetrieved {
            note_count: result_count,
            context_tokens,
            source: "tool_search".to_owned(),
        });
    }

    events
}

fn derive_lint_events(output: &serde_json::Value) -> Vec<EventKind> {
    vec![EventKind::KnowledgeEvaluated {
        health_score: output
            .get("health_score")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.0) as f32,
        note_count: output
            .get("note_count")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0) as u32,
        contradictions: output
            .get("contradictions")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0) as u32,
        missing_pages: output
            .get("missing_pages")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0) as u32,
        orphans: output
            .get("orphans")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0) as u32,
    }]
}

#[cfg(test)]
mod tests {
    use super::*;
    use aios_protocol::ToolRunId;

    #[test]
    fn indexed_event_has_correct_type() {
        let sid = SessionId::new();
        let bid = BranchId::new();
        let event = knowledge_indexed_event(&sid, &bid, 1, 53, 0.95);
        match &event.payload {
            EventPayload::Custom { event_type, data } => {
                assert_eq!(event_type, "knowledge.indexed");
                assert_eq!(data["note_count"], 53);
                let health = data["health_score"].as_f64().unwrap();
                assert!((health - 0.95).abs() < 0.01, "health_score was {health}");
            }
            _ => panic!("expected Custom event"),
        }
    }

    #[test]
    fn searched_event_has_correct_type() {
        let sid = SessionId::new();
        let bid = BranchId::new();
        let event = knowledge_searched_event(&sid, &bid, 2, "lago events", 7, 8.5);
        match &event.payload {
            EventPayload::Custom { event_type, data } => {
                assert_eq!(event_type, "knowledge.searched");
                assert_eq!(data["query"], "lago events");
                assert_eq!(data["result_count"], 7);
            }
            _ => panic!("expected Custom event"),
        }
    }

    #[test]
    fn derive_search_events_emits_typed_search_and_retrieval() {
        let events = derive_knowledge_events(&EventKind::ToolCallCompleted {
            tool_run_id: ToolRunId::default(),
            call_id: Some("call-1".into()),
            tool_name: "wiki_search".into(),
            result: serde_json::json!({
                "status": "success",
                "output": {
                    "query": "temporal validity",
                    "count": 3,
                    "top_relevance": 5.2,
                    "duration_ms": 18,
                    "context_tokens": 44,
                }
            }),
            duration_ms: 18,
            status: SpanStatus::Ok,
        });

        assert_eq!(events.len(), 2);
        assert!(matches!(
            &events[0],
            EventKind::KnowledgeSearched {
                query,
                result_count,
                top_relevance,
                duration_ms
            } if query == "temporal validity"
                && *result_count == 3
                && (*top_relevance - 5.2).abs() < f64::EPSILON
                && *duration_ms == 18
        ));
        assert!(matches!(
            &events[1],
            EventKind::KnowledgeRetrieved {
                note_count,
                context_tokens,
                source
            } if *note_count == 3 && *context_tokens == 44 && source == "tool_search"
        ));
    }

    #[test]
    fn derive_lint_events_emits_typed_knowledge_evaluated() {
        let events = derive_knowledge_events(&EventKind::ToolCallCompleted {
            tool_run_id: ToolRunId::default(),
            call_id: Some("call-2".into()),
            tool_name: "wiki_lint".into(),
            result: serde_json::json!({
                "status": "success",
                "output": {
                    "health_score": 0.82,
                    "note_count": 64,
                    "contradictions": 1,
                    "missing_pages": 2,
                    "orphans": 3,
                }
            }),
            duration_ms: 11,
            status: SpanStatus::Ok,
        });

        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            EventKind::KnowledgeEvaluated {
                health_score,
                note_count,
                contradictions,
                missing_pages,
                orphans
            } if (*health_score - 0.82).abs() < f32::EPSILON
                && *note_count == 64
                && *contradictions == 1
                && *missing_pages == 2
                && *orphans == 3
        ));
    }
}
