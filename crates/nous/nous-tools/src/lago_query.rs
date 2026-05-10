//! `lago_query` praxis tool — read-only event-journal queries.
//!
//! Authored agents (e.g. `nous-promoter`) need to walk lago events
//! when reasoning over historical scoring decisions. This tool wraps
//! [`lago_core::Journal`] in the canonical [`Tool`] trait so the same
//! agent code path that calls `nous_aggregate` and `nous_compare` can
//! also call `lago_query`.
//!
//! ## Filters
//!
//! - `event_kind` — exact match on the serialized event payload `type`
//!   discriminant (e.g. `"Message"`, `"ToolCall"`, `"Custom"`). Lago
//!   already supports this through `EventQuery::with_kind`.
//! - `after_ts_ms` / `before_ts_ms` — inclusive timestamp window in
//!   **milliseconds** since epoch. Lago events store microseconds; the
//!   tool converts on the way in and out so callers think in the same
//!   unit as `LineageEvent`.
//! - `limit` — maximum events returned. Default `50`, hard-capped at
//!   `500` to bound memory.
//!
//! ## Output
//!
//! ```jsonc
//! {
//!   "events": [
//!     { "id": "<EventId>", "kind": "<discriminant>", "ts_ms": <u64>,
//!       "payload": <serde_json::Value> },
//!     ...
//!   ]
//! }
//! ```
//!
//! ## Async over a sync trait
//!
//! `aios_protocol::tool::Tool::execute` is **synchronous** but the
//! `Journal` API is async. The tool stashes a `tokio::runtime::Handle`
//! at construction time and uses `block_on` to drive the async call,
//! mirroring the established pattern in `praxis-mcp-bridge::McpTool`.

use aios_protocol::tool::{
    Tool, ToolAnnotations, ToolCall, ToolContext, ToolDefinition, ToolError, ToolResult,
};
use lago_core::{EventEnvelope, EventQuery, Journal};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use tokio::runtime::Handle;

/// Default `limit` returned when the caller does not supply one.
pub const DEFAULT_LIMIT: u32 = 50;
/// Hard cap on `limit` regardless of caller input — bounds memory for
/// untrusted agents.
pub const MAX_LIMIT: u32 = 500;

/// One event row in [`QueryOutput`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct QueriedEvent {
    pub id: String,
    pub kind: String,
    /// Timestamp in milliseconds since epoch (lago stores microseconds;
    /// this tool exposes ms for parity with [`crate::LineageEvent`]).
    pub ts_ms: u64,
    pub payload: serde_json::Value,
}

/// Output payload of [`LagoQueryTool`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct QueryOutput {
    pub events: Vec<QueriedEvent>,
}

/// Praxis tool implementing `lago_query`.
pub struct LagoQueryTool {
    journal: Arc<dyn Journal>,
    runtime: Handle,
}

impl LagoQueryTool {
    /// Construct the tool with the given journal and the **current**
    /// tokio runtime handle. Panics outside a tokio runtime — callers
    /// in test code should wrap construction in `#[tokio::test]` or
    /// `Runtime::new()?.block_on(...)`.
    pub fn new(journal: Arc<dyn Journal>) -> Self {
        Self {
            journal,
            runtime: Handle::current(),
        }
    }

    /// Construct with an explicit runtime handle (for cases where the
    /// caller wants to dispatch from a non-async context).
    pub fn with_runtime(journal: Arc<dyn Journal>, runtime: Handle) -> Self {
        Self { journal, runtime }
    }
}

impl Tool for LagoQueryTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "lago_query".into(),
            description: "Read events from the lago journal. Supports filtering by event kind \
                 discriminant, timestamp window (ms), and a bounded result limit \
                 (default 50, max 500)."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "event_kind": {
                        "type": "string",
                        "description": "Exact match on event payload 'type' discriminant."
                    },
                    "after_ts_ms": {
                        "type": "integer",
                        "minimum": 0,
                        "description": "Inclusive lower bound on event timestamp (milliseconds)."
                    },
                    "before_ts_ms": {
                        "type": "integer",
                        "minimum": 0,
                        "description": "Inclusive upper bound on event timestamp (milliseconds)."
                    },
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": MAX_LIMIT,
                        "description": "Max events returned. Default 50; hard cap 500."
                    }
                }
            }),
            title: Some("Lago Query".into()),
            output_schema: Some(json!({
                "type": "object",
                "properties": {
                    "events": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "id": { "type": "string" },
                                "kind": { "type": "string" },
                                "ts_ms": { "type": "integer", "minimum": 0 },
                                "payload": {}
                            },
                            "required": ["id", "kind", "ts_ms", "payload"]
                        }
                    }
                },
                "required": ["events"]
            })),
            annotations: Some(ToolAnnotations {
                read_only: true,
                idempotent: true,
                ..Default::default()
            }),
            category: Some("nous".into()),
            tags: vec!["nous".into(), "lago".into(), "query".into()],
            timeout_secs: Some(15),
        }
    }

    fn execute(&self, call: &ToolCall, _ctx: &ToolContext) -> Result<ToolResult, ToolError> {
        // Decode + validate input.
        let event_kind = match call.input.get("event_kind") {
            Some(serde_json::Value::Null) | None => None,
            Some(v) => Some(
                v.as_str()
                    .ok_or_else(|| ToolError::InvalidInput {
                        message: "'event_kind' must be a string".into(),
                    })?
                    .to_string(),
            ),
        };
        let after_ts_ms = decode_optional_u64(&call.input, "after_ts_ms")?;
        let before_ts_ms = decode_optional_u64(&call.input, "before_ts_ms")?;
        if let (Some(a), Some(b)) = (after_ts_ms, before_ts_ms)
            && a > b
        {
            return Err(ToolError::InvalidInput {
                message: format!("after_ts_ms ({a}) must be <= before_ts_ms ({b})"),
            });
        }

        let limit = decode_optional_u32(&call.input, "limit")?
            .unwrap_or(DEFAULT_LIMIT)
            .clamp(1, MAX_LIMIT) as usize;

        // Build a Lago query. Kind filtering is delegated to the journal
        // for backends that can index it; the timestamp filter is
        // applied client-side because EventQuery exposes seq filters,
        // not timestamp filters.
        let mut query = EventQuery::new().limit(limit);
        if let Some(ref kind) = event_kind {
            query = query.with_kind(kind.clone());
        }

        let journal = self.journal.clone();
        let runtime = self.runtime.clone();
        // `Tool::execute` is sync but `Journal::read` is async. We
        // detect whether we are already inside a tokio runtime: if so,
        // hop out of the async context with `block_in_place` so we can
        // legally call `Handle::block_on` (this is the same trick
        // `praxis-mcp-bridge::McpTool` uses, generalised for both
        // contexts). If not, plain `block_on` is sufficient.
        let envelopes: Vec<EventEnvelope> = if Handle::try_current().is_ok() {
            tokio::task::block_in_place(|| {
                runtime.block_on(async move { journal.read(query).await })
            })
        } else {
            runtime.block_on(async move { journal.read(query).await })
        }
        .map_err(|e| ToolError::ExecutionFailed {
            tool_name: "lago_query".into(),
            message: format!("journal read failed: {e}"),
        })?;

        // Apply timestamp filter (us → ms) and shape the rows.
        let mut events = Vec::with_capacity(envelopes.len());
        for env in envelopes {
            let ts_ms = env.timestamp / 1_000;
            if let Some(after) = after_ts_ms
                && ts_ms < after
            {
                continue;
            }
            if let Some(before) = before_ts_ms
                && ts_ms > before
            {
                continue;
            }

            let payload_value =
                serde_json::to_value(&env.payload).map_err(|e| ToolError::ExecutionFailed {
                    tool_name: "lago_query".into(),
                    message: format!("failed to serialize event payload: {e}"),
                })?;
            let kind = payload_value
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap_or("Unknown")
                .to_string();

            events.push(QueriedEvent {
                id: env.event_id.as_str().to_string(),
                kind,
                ts_ms,
                payload: payload_value,
            });

            if events.len() >= limit {
                break;
            }
        }

        let output = serde_json::to_value(QueryOutput { events }).map_err(|e| {
            ToolError::ExecutionFailed {
                tool_name: "lago_query".into(),
                message: format!("failed to serialize query output: {e}"),
            }
        })?;

        Ok(ToolResult {
            call_id: call.call_id.clone(),
            tool_name: call.tool_name.clone(),
            output,
            content: None,
            is_error: false,
            usage: None,
        })
    }
}

fn decode_optional_u64(input: &serde_json::Value, field: &str) -> Result<Option<u64>, ToolError> {
    match input.get(field) {
        Some(serde_json::Value::Null) | None => Ok(None),
        Some(v) => v.as_u64().map(Some).ok_or_else(|| ToolError::InvalidInput {
            message: format!("'{field}' must be a non-negative integer"),
        }),
    }
}

fn decode_optional_u32(input: &serde_json::Value, field: &str) -> Result<Option<u32>, ToolError> {
    match input.get(field) {
        Some(serde_json::Value::Null) | None => Ok(None),
        Some(v) => {
            let n = v.as_u64().ok_or_else(|| ToolError::InvalidInput {
                message: format!("'{field}' must be a non-negative integer"),
            })?;
            if n > u32::MAX as u64 {
                return Err(ToolError::InvalidInput {
                    message: format!("'{field}' exceeds u32::MAX"),
                });
            }
            Ok(Some(n as u32))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lago_core::{
        BranchId, EventEnvelope, EventId, EventPayload, EventStream, LagoResult, SeqNo, Session,
        SessionId,
    };
    use std::collections::HashMap;
    use std::pin::Pin;
    use std::sync::RwLock;

    /// Minimal test journal: stores a fixed `Vec<EventEnvelope>` and
    /// honours kind / limit filters via [`EventQuery::matches_filters`].
    /// Other journal methods are stubs (we only test `read` here).
    pub struct VecJournal {
        events: RwLock<Vec<EventEnvelope>>,
    }

    impl VecJournal {
        pub fn new(events: Vec<EventEnvelope>) -> Self {
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
            let head = self
                .events
                .read()
                .unwrap()
                .iter()
                .map(|e| e.seq)
                .max()
                .unwrap_or(0);
            Box::pin(async move { Ok(head) })
        }

        fn stream(
            &self,
            _session_id: SessionId,
            _branch_id: BranchId,
            _after_seq: SeqNo,
        ) -> Pin<Box<dyn std::future::Future<Output = LagoResult<EventStream>> + Send + '_>>
        {
            // Not exercised in these tests.
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
        ) -> Pin<Box<dyn std::future::Future<Output = LagoResult<Option<Session>>> + Send + '_>>
        {
            Box::pin(async move { Ok(None) })
        }

        fn list_sessions(
            &self,
        ) -> Pin<Box<dyn std::future::Future<Output = LagoResult<Vec<Session>>> + Send + '_>>
        {
            Box::pin(async move { Ok(Vec::new()) })
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

    fn ctx() -> ToolContext {
        ToolContext {
            run_id: "test-run".into(),
            session_id: "S1".into(),
            iteration: 0,
            ..Default::default()
        }
    }

    fn call(input: serde_json::Value) -> ToolCall {
        ToolCall {
            call_id: "call-1".into(),
            tool_name: "lago_query".into(),
            input,
            requested_capabilities: vec![],
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn lago_query_filters_by_kind() {
        let events = vec![
            make_event(
                "E1",
                1,
                1_000_000,
                EventPayload::Message {
                    role: "user".into(),
                    content: "hi".into(),
                    model: None,
                    token_usage: None,
                },
            ),
            make_event(
                "E2",
                2,
                2_000_000,
                EventPayload::ErrorRaised {
                    message: "boom".into(),
                },
            ),
            make_event(
                "E3",
                3,
                3_000_000,
                EventPayload::Message {
                    role: "assistant".into(),
                    content: "ok".into(),
                    model: None,
                    token_usage: None,
                },
            ),
        ];
        let journal: Arc<dyn Journal> = Arc::new(VecJournal::new(events));
        let tool = LagoQueryTool::new(journal);

        let result = tool
            .execute(&call(json!({"event_kind": "Message"})), &ctx())
            .unwrap();
        let out: QueryOutput = serde_json::from_value(result.output).unwrap();
        assert_eq!(out.events.len(), 2);
        assert_eq!(out.events[0].id, "E1");
        assert_eq!(out.events[0].kind, "Message");
        assert_eq!(out.events[0].ts_ms, 1_000); // 1_000_000 us → 1000 ms.
        assert_eq!(out.events[1].id, "E3");

        // Filter that matches nothing.
        let result = tool
            .execute(&call(json!({"event_kind": "DoesNotExist"})), &ctx())
            .unwrap();
        let out: QueryOutput = serde_json::from_value(result.output).unwrap();
        assert!(out.events.is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn lago_query_respects_limit_and_hard_cap() {
        // 600 events; the hard cap should clamp the output at 500.
        let events: Vec<EventEnvelope> = (0..600)
            .map(|i| {
                make_event(
                    &format!("E{i}"),
                    i + 1,
                    (i + 1) * 1_000,
                    EventPayload::Message {
                        role: "user".into(),
                        content: format!("msg-{i}"),
                        model: None,
                        token_usage: None,
                    },
                )
            })
            .collect();
        let journal: Arc<dyn Journal> = Arc::new(VecJournal::new(events));
        let tool = LagoQueryTool::new(journal);

        let result = tool.execute(&call(json!({"limit": 9999})), &ctx()).unwrap();
        let out: QueryOutput = serde_json::from_value(result.output).unwrap();
        assert_eq!(out.events.len(), MAX_LIMIT as usize);

        // Default limit = 50.
        let result = tool.execute(&call(json!({})), &ctx()).unwrap();
        let out: QueryOutput = serde_json::from_value(result.output).unwrap();
        assert_eq!(out.events.len(), DEFAULT_LIMIT as usize);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn lago_query_filters_by_timestamp_window() {
        let events = vec![
            make_event(
                "E1",
                1,
                1_000_000, // 1000 ms
                EventPayload::Message {
                    role: "user".into(),
                    content: "early".into(),
                    model: None,
                    token_usage: None,
                },
            ),
            make_event(
                "E2",
                2,
                5_000_000, // 5000 ms
                EventPayload::Message {
                    role: "user".into(),
                    content: "middle".into(),
                    model: None,
                    token_usage: None,
                },
            ),
            make_event(
                "E3",
                3,
                9_000_000, // 9000 ms
                EventPayload::Message {
                    role: "user".into(),
                    content: "late".into(),
                    model: None,
                    token_usage: None,
                },
            ),
        ];
        let journal: Arc<dyn Journal> = Arc::new(VecJournal::new(events));
        let tool = LagoQueryTool::new(journal);

        let result = tool
            .execute(
                &call(json!({"after_ts_ms": 2000, "before_ts_ms": 7000})),
                &ctx(),
            )
            .unwrap();
        let out: QueryOutput = serde_json::from_value(result.output).unwrap();
        assert_eq!(out.events.len(), 1);
        assert_eq!(out.events[0].id, "E2");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn lago_query_rejects_inverted_window() {
        let journal: Arc<dyn Journal> = Arc::new(VecJournal::new(vec![]));
        let tool = LagoQueryTool::new(journal);
        let err = tool
            .execute(
                &call(json!({"after_ts_ms": 9000, "before_ts_ms": 1000})),
                &ctx(),
            )
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidInput { .. }));
    }
}
