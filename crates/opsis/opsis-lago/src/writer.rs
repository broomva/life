//! Background event writer — persists OpsisEvents to Lago journal via MPSC channel.
//!
//! The writer runs as a background tokio task. The engine sends events through
//! a bounded channel; the writer wraps them in Lago envelopes and appends to
//! the journal. Failures are logged but never block the tick loop.

use std::sync::Arc;

use lago_core::id::{BranchId, SessionId};
use lago_core::journal::Journal;
use opsis_core::event::OpsisEvent;
use tokio::sync::mpsc;

use crate::event_map;

/// Handle for sending OpsisEvents to the background writer.
///
/// Created by [`OpsisEventWriter::spawn`]. Clone-friendly.
#[derive(Clone)]
pub struct OpsisEventWriter {
    tx: mpsc::Sender<OpsisEvent>,
}

impl OpsisEventWriter {
    /// Spawn the background writer task and return a handle.
    ///
    /// `buffer_size` controls the MPSC channel capacity. If the channel is full,
    /// events are dropped with a warning — the tick loop is never blocked.
    pub fn spawn(
        journal: Arc<dyn Journal>,
        session_id: SessionId,
        branch_id: BranchId,
        buffer_size: usize,
    ) -> Self {
        let (tx, rx) = mpsc::channel(buffer_size);
        tokio::spawn(run_event_writer(rx, journal, session_id, branch_id));
        Self { tx }
    }

    /// Send an event to the background writer.
    ///
    /// Non-blocking: drops the event with a tracing warning if the channel is full.
    pub fn send(&self, event: OpsisEvent) {
        if let Err(e) = self.tx.try_send(event) {
            tracing::warn!(
                error = %e,
                "opsis-lago: event writer channel full or closed, dropping event"
            );
        }
    }

    /// Send a batch of events.
    pub fn send_batch(&self, events: impl IntoIterator<Item = OpsisEvent>) {
        for event in events {
            self.send(event);
        }
    }
}

/// Background task that drains the MPSC channel and appends events to the journal.
async fn run_event_writer(
    mut rx: mpsc::Receiver<OpsisEvent>,
    journal: Arc<dyn Journal>,
    session_id: SessionId,
    branch_id: BranchId,
) {
    tracing::info!(
        session = %session_id,
        branch = %branch_id,
        "opsis-lago: event writer started"
    );

    let mut written: u64 = 0;

    while let Some(event) = rx.recv().await {
        let envelope = event_map::opsis_to_lago(&event, &session_id, &branch_id);
        match journal.append(envelope).await {
            Ok(seq) => {
                written += 1;
                if written.is_multiple_of(100) {
                    tracing::debug!(written, seq, "opsis-lago: events persisted");
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "opsis-lago: failed to persist event");
            }
        }
    }

    tracing::info!(written, "opsis-lago: event writer shut down");
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use lago_core::event::EventEnvelope;
    use lago_core::id::SeqNo;
    use opsis_core::clock::WorldTick;
    use opsis_core::event::{EventSource, OpsisEventKind};
    use opsis_core::feed::{FeedSource, SchemaKey};
    use opsis_core::state::StateDomain;
    use std::sync::Mutex;

    /// In-memory journal for testing.
    struct MemJournal {
        events: Mutex<Vec<EventEnvelope>>,
    }

    impl MemJournal {
        fn new() -> Self {
            Self {
                events: Mutex::new(Vec::new()),
            }
        }
    }

    impl Journal for MemJournal {
        fn append(
            &self,
            mut event: EventEnvelope,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = lago_core::error::LagoResult<SeqNo>> + Send + '_>,
        > {
            Box::pin(async move {
                let mut events = self.events.lock().unwrap();
                let seq = events.len() as u64 + 1;
                event.seq = seq;
                events.push(event);
                Ok(seq)
            })
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
            _query: lago_core::journal::EventQuery,
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

    fn sample_event(domain: StateDomain, summary: &str) -> OpsisEvent {
        OpsisEvent {
            id: opsis_core::event::EventId::default(),
            tick: WorldTick(1),
            timestamp: Utc::now(),
            source: EventSource::Feed(FeedSource::new("test")),
            kind: OpsisEventKind::WorldObservation {
                summary: summary.into(),
            },
            location: None,
            domain: Some(domain),
            severity: Some(0.5),
            schema_key: SchemaKey::new("test.v1"),
            tags: vec![],
        }
    }

    #[tokio::test]
    async fn writer_persists_events() {
        let journal = Arc::new(MemJournal::new());
        let session = SessionId::from_string("opsis-test");
        let branch = BranchId::from("main");

        let writer = OpsisEventWriter::spawn(journal.clone(), session, branch, 64);

        writer.send(sample_event(StateDomain::Emergency, "earthquake 1"));
        writer.send(sample_event(StateDomain::Weather, "storm 1"));
        writer.send(sample_event(StateDomain::Emergency, "earthquake 2"));

        // Give the background task time to process.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let events = journal.events.lock().unwrap();
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].seq, 1);
        assert_eq!(events[2].seq, 3);

        // Verify opsis event can be extracted back.
        let restored = crate::event_map::lago_to_opsis(&events[0]).unwrap();
        match &restored.kind {
            OpsisEventKind::WorldObservation { summary } => {
                assert_eq!(summary, "earthquake 1");
            }
            _ => panic!("expected WorldObservation"),
        }
    }
}
