//! Startup replay — rebuilds WorldState from persisted Lago events.
//!
//! On daemon startup, reads all OpsisEvents from the journal and replays
//! them to reconstruct the last known world state, restoring hotspots,
//! domain activity levels, and recent event history.

use std::sync::Arc;

use lago_core::id::{BranchId, SessionId};
use lago_core::journal::{EventQuery, Journal};
use opsis_core::event::OpsisEvent;

use crate::event_map;

/// Replays persisted events to rebuild engine state.
pub struct OpsisReplay {
    journal: Arc<dyn Journal>,
    session_id: SessionId,
    branch_id: BranchId,
}

impl OpsisReplay {
    pub fn new(journal: Arc<dyn Journal>, session_id: SessionId, branch_id: BranchId) -> Self {
        Self {
            journal,
            session_id,
            branch_id,
        }
    }

    /// Read all persisted OpsisEvents from the journal.
    ///
    /// Returns events in sequence order (oldest first). The caller can
    /// feed these into the engine's tick aggregator to rebuild state.
    pub async fn load_events(&self) -> Result<Vec<OpsisEvent>, ReplayError> {
        let query = EventQuery::new()
            .session(self.session_id.clone())
            .branch(self.branch_id.clone());

        let envelopes = self
            .journal
            .read(query)
            .await
            .map_err(|e| ReplayError::JournalRead(e.to_string()))?;

        let mut events = Vec::with_capacity(envelopes.len());
        for envelope in &envelopes {
            if let Some(event) = event_map::lago_to_opsis(envelope) {
                events.push(event);
            }
        }

        tracing::info!(
            total_envelopes = envelopes.len(),
            opsis_events = events.len(),
            "opsis-lago: replay loaded events from journal"
        );

        Ok(events)
    }

    /// Load events and return the count (for startup logging).
    pub async fn event_count(&self) -> Result<usize, ReplayError> {
        let events = self.load_events().await?;
        Ok(events.len())
    }
}

/// Errors during replay.
#[derive(Debug, thiserror::Error)]
pub enum ReplayError {
    #[error("journal read failed: {0}")]
    JournalRead(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use lago_core::event::{EventEnvelope, EventPayload};
    use lago_core::id::{EventId as LagoEventId, SeqNo};
    use opsis_core::clock::WorldTick;
    use opsis_core::event::{EventSource, OpsisEventKind};
    use opsis_core::feed::{FeedSource, SchemaKey};
    use opsis_core::state::StateDomain;
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// In-memory journal that stores pre-loaded envelopes.
    struct PreloadedJournal {
        events: Mutex<Vec<EventEnvelope>>,
    }

    impl PreloadedJournal {
        fn with_events(events: Vec<EventEnvelope>) -> Self {
            Self {
                events: Mutex::new(events),
            }
        }
    }

    impl Journal for PreloadedJournal {
        fn append(
            &self,
            _event: EventEnvelope,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = lago_core::error::LagoResult<SeqNo>> + Send + '_>,
        > {
            Box::pin(async { Ok(0) })
        }

        fn append_batch(
            &self,
            _events: Vec<EventEnvelope>,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = lago_core::error::LagoResult<SeqNo>> + Send + '_>,
        > {
            Box::pin(async { Ok(0) })
        }

        fn read(
            &self,
            _query: EventQuery,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<Output = lago_core::error::LagoResult<Vec<EventEnvelope>>>
                    + Send
                    + '_,
            >,
        > {
            Box::pin(async { Ok(self.events.lock().unwrap().clone()) })
        }

        fn get_event(
            &self,
            _event_id: &lago_core::id::EventId,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = lago_core::error::LagoResult<Option<EventEnvelope>>,
                    > + Send
                    + '_,
            >,
        > {
            Box::pin(async { Ok(None) })
        }

        fn head_seq(
            &self,
            _session_id: &SessionId,
            _branch_id: &BranchId,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = lago_core::error::LagoResult<SeqNo>> + Send + '_>,
        > {
            Box::pin(async { Ok(0) })
        }

        fn stream(
            &self,
            _session_id: SessionId,
            _branch_id: BranchId,
            _after_seq: SeqNo,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = lago_core::error::LagoResult<lago_core::journal::EventStream>,
                    > + Send
                    + '_,
            >,
        > {
            Box::pin(async {
                Err(lago_core::error::LagoError::Journal(
                    "stream not supported".into(),
                ))
            })
        }

        fn put_session(
            &self,
            _session: lago_core::session::Session,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = lago_core::error::LagoResult<()>> + Send + '_>,
        > {
            Box::pin(async { Ok(()) })
        }

        fn get_session(
            &self,
            _session_id: &SessionId,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = lago_core::error::LagoResult<Option<lago_core::session::Session>>,
                    > + Send
                    + '_,
            >,
        > {
            Box::pin(async { Ok(None) })
        }

        fn list_sessions(
            &self,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = lago_core::error::LagoResult<Vec<lago_core::session::Session>>,
                    > + Send
                    + '_,
            >,
        > {
            Box::pin(async { Ok(vec![]) })
        }
    }

    fn make_opsis_envelope(seq: u64, summary: &str) -> EventEnvelope {
        let event = OpsisEvent {
            id: opsis_core::event::EventId::default(),
            tick: WorldTick(seq),
            timestamp: Utc::now(),
            source: EventSource::Feed(FeedSource::new("usgs")),
            kind: OpsisEventKind::WorldObservation {
                summary: summary.into(),
            },
            location: None,
            domain: Some(StateDomain::Emergency),
            severity: Some(0.5),
            schema_key: SchemaKey::new("usgs.v1"),
            tags: vec![],
        };

        let session_id = SessionId::from_string("opsis-world");
        let branch_id = BranchId::from("main");
        let mut env = crate::event_map::opsis_to_lago(&event, &session_id, &branch_id);
        env.seq = seq;
        env
    }

    #[tokio::test]
    async fn replay_loads_opsis_events() {
        let envelopes = vec![
            make_opsis_envelope(1, "earthquake 1"),
            make_opsis_envelope(2, "earthquake 2"),
            make_opsis_envelope(3, "storm 1"),
        ];

        let journal = Arc::new(PreloadedJournal::with_events(envelopes));
        let replay = OpsisReplay::new(
            journal,
            SessionId::from_string("opsis-world"),
            BranchId::from("main"),
        );

        let events = replay.load_events().await.unwrap();
        assert_eq!(events.len(), 3);

        match &events[0].kind {
            OpsisEventKind::WorldObservation { summary } => {
                assert_eq!(summary, "earthquake 1");
            }
            _ => panic!("expected WorldObservation"),
        }
    }

    #[tokio::test]
    async fn replay_skips_non_opsis_envelopes() {
        let mut envelopes = vec![make_opsis_envelope(1, "earthquake")];

        // Add a non-opsis envelope.
        envelopes.push(EventEnvelope {
            event_id: LagoEventId::new(),
            session_id: SessionId::from_string("opsis-world"),
            branch_id: BranchId::from("main"),
            run_id: None,
            seq: 2,
            timestamp: EventEnvelope::now_micros(),
            parent_id: None,
            payload: EventPayload::Custom {
                event_type: "arcan.something".into(),
                data: serde_json::json!({}),
            },
            metadata: HashMap::new(),
            schema_version: 1,
        });

        let journal = Arc::new(PreloadedJournal::with_events(envelopes));
        let replay = OpsisReplay::new(
            journal,
            SessionId::from_string("opsis-world"),
            BranchId::from("main"),
        );

        let events = replay.load_events().await.unwrap();
        assert_eq!(events.len(), 1); // Only the opsis event
    }
}
