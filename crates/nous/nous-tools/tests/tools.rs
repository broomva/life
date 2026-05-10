//! End-to-end integration test exercising all three nous-tools through
//! the `aios_protocol::tool::Tool` dispatch surface.
//!
//! Mirrors the path an authored agent takes:
//!   1. spawn a session
//!   2. record several lineage events
//!   3. query lago for context
//!   4. aggregate the scores
//!   5. compare two aggregates and confirm the verdict.

use aios_protocol::tool::{Tool, ToolCall, ToolContext};
use lago_core::{
    BranchId, EventEnvelope, EventId, EventPayload, EventQuery, EventStream, Journal, LagoResult,
    SeqNo, Session, SessionId,
};
use nous_tools::{
    AggregateOutput, CompareOutput, CompareVerdict, InMemoryNousLineage, LagoQueryTool,
    LineageEvent, LineageFilter, NousAggregateTool, NousCompareTool, NousLineage, QueryOutput,
};
use serde_json::json;
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::{Arc, RwLock};

/// Stub journal — same shape as the unit-test fixture but lifted into
/// the integration crate so the round-trip exercises the real dispatch
/// surface.
struct VecJournal {
    events: RwLock<Vec<EventEnvelope>>,
}

impl VecJournal {
    fn new(events: Vec<EventEnvelope>) -> Self {
        Self {
            events: RwLock::new(events),
        }
    }
}

impl Journal for VecJournal {
    fn append(
        &self,
        event: EventEnvelope,
    ) -> Pin<Box<dyn std::future::Future<Output = LagoResult<SeqNo>> + Send + '_>> {
        let seq = event.seq;
        self.events.write().unwrap().push(event);
        Box::pin(async move { Ok(seq) })
    }

    fn append_batch(
        &self,
        events: Vec<EventEnvelope>,
    ) -> Pin<Box<dyn std::future::Future<Output = LagoResult<SeqNo>> + Send + '_>> {
        let last = events.last().map(|e| e.seq).unwrap_or(0);
        self.events.write().unwrap().extend(events);
        Box::pin(async move { Ok(last) })
    }

    fn read(
        &self,
        query: EventQuery,
    ) -> Pin<Box<dyn std::future::Future<Output = LagoResult<Vec<EventEnvelope>>> + Send + '_>>
    {
        let snapshot = self.events.read().unwrap().clone();
        Box::pin(async move {
            let mut out: Vec<EventEnvelope> = snapshot
                .into_iter()
                .filter(|env| query.matches_filters(env))
                .collect();
            if let Some(limit) = query.limit {
                out.truncate(limit);
            }
            Ok(out)
        })
    }

    fn get_event(
        &self,
        event_id: &EventId,
    ) -> Pin<Box<dyn std::future::Future<Output = LagoResult<Option<EventEnvelope>>> + Send + '_>>
    {
        let target = event_id.clone();
        let snapshot = self.events.read().unwrap().clone();
        Box::pin(async move { Ok(snapshot.into_iter().find(|e| e.event_id == target)) })
    }

    fn head_seq(
        &self,
        _session_id: &SessionId,
        _branch_id: &BranchId,
    ) -> Pin<Box<dyn std::future::Future<Output = LagoResult<SeqNo>> + Send + '_>> {
        Box::pin(async move { Ok(0) })
    }

    fn stream(
        &self,
        _session_id: SessionId,
        _branch_id: BranchId,
        _after_seq: SeqNo,
    ) -> Pin<Box<dyn std::future::Future<Output = LagoResult<EventStream>> + Send + '_>> {
        Box::pin(async move {
            let stream = futures::stream::empty::<LagoResult<EventEnvelope>>();
            Ok(Box::pin(stream) as EventStream)
        })
    }

    fn put_session(
        &self,
        _session: Session,
    ) -> Pin<Box<dyn std::future::Future<Output = LagoResult<()>> + Send + '_>> {
        Box::pin(async move { Ok(()) })
    }

    fn get_session(
        &self,
        _session_id: &SessionId,
    ) -> Pin<Box<dyn std::future::Future<Output = LagoResult<Option<Session>>> + Send + '_>> {
        Box::pin(async move { Ok(None) })
    }

    fn list_sessions(
        &self,
    ) -> Pin<Box<dyn std::future::Future<Output = LagoResult<Vec<Session>>> + Send + '_>> {
        Box::pin(async move { Ok(Vec::new()) })
    }
}

fn ctx() -> ToolContext {
    ToolContext {
        run_id: "run-1".into(),
        session_id: "S1".into(),
        iteration: 0,
        ..Default::default()
    }
}

fn call(name: &str, input: serde_json::Value) -> ToolCall {
    ToolCall {
        call_id: format!("call-{name}"),
        tool_name: name.into(),
        input,
        requested_capabilities: vec![],
    }
}

fn make_event(id: &str, seq: u64, ts_us: u64, payload: EventPayload) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId::from_string(id),
        session_id: SessionId::from_string("S1"),
        branch_id: BranchId::from_string("main"),
        run_id: None,
        seq,
        timestamp: ts_us,
        parent_id: None,
        payload,
        metadata: HashMap::new(),
        schema_version: 1,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn end_to_end_promoter_flow() {
    // 1. Lineage substrate populated with two scoring rounds.
    let lineage = InMemoryNousLineage::new();
    let scores_round_a: Vec<f64> = vec![6.0, 7.0, 8.0, 7.0, 6.0];
    let scores_round_b: Vec<f64> = vec![7.5, 8.0, 9.0, 8.5, 7.0];

    for (i, s) in scores_round_a.iter().enumerate() {
        lineage
            .record(LineageEvent {
                session_id: "S1".into(),
                agent_name: "bookkeeping.score-novelty".into(),
                target_ref: Some(format!("notes/round-a-item-{i}")),
                input: json!({"text": format!("a{i}")}),
                output: json!({"score": s}),
                timestamp_ms: 1_000 + i as u64,
                lago_event_id: None,
            })
            .await
            .unwrap();
    }
    for (i, s) in scores_round_b.iter().enumerate() {
        lineage
            .record(LineageEvent {
                session_id: "S2".into(),
                agent_name: "bookkeeping.score-novelty".into(),
                target_ref: Some(format!("notes/round-b-item-{i}")),
                input: json!({"text": format!("b{i}")}),
                output: json!({"score": s}),
                timestamp_ms: 2_000 + i as u64,
                lago_event_id: None,
            })
            .await
            .unwrap();
    }

    let recorded_a = lineage
        .query(LineageFilter {
            session_id: Some("S1".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    let recorded_b = lineage
        .query(LineageFilter {
            session_id: Some("S2".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(recorded_a.len(), 5);
    assert_eq!(recorded_b.len(), 5);

    let extract_scores = |events: &[LineageEvent]| -> Vec<f64> {
        events
            .iter()
            .filter_map(|e| e.output.get("score").and_then(|s| s.as_f64()))
            .collect()
    };

    // 2. Aggregate via the `nous_aggregate` tool.
    let aggregate = NousAggregateTool::new();
    let agg_a_result = aggregate
        .execute(
            &call(
                "nous_aggregate",
                json!({"scores": extract_scores(&recorded_a)}),
            ),
            &ctx(),
        )
        .unwrap();
    let agg_b_result = aggregate
        .execute(
            &call(
                "nous_aggregate",
                json!({"scores": extract_scores(&recorded_b)}),
            ),
            &ctx(),
        )
        .unwrap();
    let agg_a: AggregateOutput = serde_json::from_value(agg_a_result.output).unwrap();
    let agg_b: AggregateOutput = serde_json::from_value(agg_b_result.output).unwrap();
    assert_eq!(agg_a.count, 5);
    assert_eq!(agg_b.count, 5);
    assert!((agg_a.mean - 6.8).abs() < 1e-9);
    assert!((agg_b.mean - 8.0).abs() < 1e-9);

    // 3. Compare via `nous_compare` — round B should beat round A.
    let compare = NousCompareTool::new();
    let cmp_result = compare
        .execute(
            &call(
                "nous_compare",
                json!({"a": agg_b.mean, "b": agg_a.mean, "threshold": 0.05}),
            ),
            &ctx(),
        )
        .unwrap();
    let cmp: CompareOutput = serde_json::from_value(cmp_result.output).unwrap();
    assert_eq!(cmp.verdict, CompareVerdict::Greater);
    assert!((cmp.delta - 1.2).abs() < 1e-9);

    // 4. Query lago for an event referenced by the lineage row.
    let journal: Arc<dyn Journal> = Arc::new(VecJournal::new(vec![
        make_event(
            "E1",
            1,
            1_500_000,
            EventPayload::Message {
                role: "user".into(),
                content: "round-a sample".into(),
                model: None,
                token_usage: None,
            },
        ),
        make_event(
            "E2",
            2,
            2_500_000,
            EventPayload::Message {
                role: "assistant".into(),
                content: "round-b sample".into(),
                model: None,
                token_usage: None,
            },
        ),
    ]));
    let lago = LagoQueryTool::new(journal);
    let lago_result = lago
        .execute(
            &call("lago_query", json!({"event_kind": "Message", "limit": 50})),
            &ctx(),
        )
        .unwrap();
    let lago_out: QueryOutput = serde_json::from_value(lago_result.output).unwrap();
    assert_eq!(lago_out.events.len(), 2);
    assert_eq!(lago_out.events[0].id, "E1");
    assert_eq!(lago_out.events[1].id, "E2");
}
