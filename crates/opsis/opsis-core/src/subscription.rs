use serde::{Deserialize, Serialize};
use ulid::Ulid;

use crate::event::OpsisEvent;
use crate::spatial::Bbox;
use crate::state::StateDomain;

/// Unique client identifier (ULID by default).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ClientId(pub String);

impl Default for ClientId {
    fn default() -> Self {
        Self(Ulid::new().to_string())
    }
}

/// A subscription filter — decides which [`OpsisEvent`]s a client receives.
///
/// All non-empty filter fields are combined with AND logic.  An empty filter
/// (e.g. `Subscription::all()`) matches everything.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Subscription {
    /// Only match events in these domains.  Empty = any domain.
    pub domains: Vec<StateDomain>,
    /// Only match events whose location falls within this box.
    pub bbox: Option<Bbox>,
    /// Minimum severity threshold (events below this are skipped).
    pub severity_threshold: f32,
    /// Only match events whose tags contain any of these keywords.
    /// Empty = no keyword filter.
    pub keywords: Vec<String>,
}

impl Subscription {
    /// A subscription that matches everything.
    pub fn all() -> Self {
        Self {
            domains: Vec::new(),
            bbox: None,
            severity_threshold: 0.0,
            keywords: Vec::new(),
        }
    }

    /// Returns `true` if the given event matches all active filters.
    pub fn matches(&self, event: &OpsisEvent) -> bool {
        // Domain filter — skip if event has no domain and we're filtering by domain.
        if !self.domains.is_empty() {
            match &event.domain {
                Some(domain) if self.domains.contains(domain) => {}
                _ => return false,
            }
        }

        // Severity filter — events without severity are treated as 0.0.
        let severity = event.severity.unwrap_or(0.0);
        if severity < self.severity_threshold {
            return false;
        }

        // Bbox filter
        if let Some(ref bbox) = self.bbox {
            match event.location {
                Some(ref loc) => {
                    if !bbox.contains(loc) {
                        return false;
                    }
                }
                None => return false,
            }
        }

        // Keyword filter — match against tags.
        if !self.keywords.is_empty() {
            let has_keyword = self.keywords.iter().any(|kw| {
                let kw_lower = kw.to_lowercase();
                event
                    .tags
                    .iter()
                    .any(|t| t.to_lowercase().contains(&kw_lower))
            });
            if !has_keyword {
                return false;
            }
        }

        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::WorldTick;
    use crate::event::{EventId, EventSource, OpsisEventKind};
    use crate::feed::{FeedSource, SchemaKey};
    use crate::spatial::GeoPoint;
    use chrono::Utc;

    fn sample_event() -> OpsisEvent {
        OpsisEvent {
            id: EventId::default(),
            tick: WorldTick(1),
            timestamp: Utc::now(),
            source: EventSource::Feed(FeedSource::new("test")),
            kind: OpsisEventKind::WorldObservation {
                summary: "Stock market surge".into(),
            },
            location: Some(GeoPoint::new(5.0, 5.0)),
            domain: Some(StateDomain::Finance),
            severity: Some(0.7),
            schema_key: SchemaKey::new("test.v1"),
            tags: vec!["finance".into(), "market".into()],
        }
    }

    #[test]
    fn all_matches_everything() {
        let sub = Subscription::all();
        assert!(sub.matches(&sample_event()));
    }

    #[test]
    fn domain_filter() {
        let sub = Subscription {
            domains: vec![StateDomain::Weather],
            ..Subscription::all()
        };
        assert!(!sub.matches(&sample_event()));

        let sub = Subscription {
            domains: vec![StateDomain::Finance],
            ..Subscription::all()
        };
        assert!(sub.matches(&sample_event()));
    }

    #[test]
    fn domain_filter_no_domain_event() {
        let mut evt = sample_event();
        evt.domain = None;
        let sub = Subscription {
            domains: vec![StateDomain::Finance],
            ..Subscription::all()
        };
        assert!(!sub.matches(&evt));
    }

    #[test]
    fn severity_filter() {
        let sub = Subscription {
            severity_threshold: 0.9,
            ..Subscription::all()
        };
        assert!(!sub.matches(&sample_event()));

        let sub = Subscription {
            severity_threshold: 0.5,
            ..Subscription::all()
        };
        assert!(sub.matches(&sample_event()));
    }

    #[test]
    fn severity_filter_none_severity() {
        let mut evt = sample_event();
        evt.severity = None;
        let sub = Subscription {
            severity_threshold: 0.1,
            ..Subscription::all()
        };
        assert!(!sub.matches(&evt));
    }

    #[test]
    fn bbox_filter() {
        let bbox = Bbox::new(GeoPoint::new(0.0, 0.0), GeoPoint::new(10.0, 10.0));
        let sub = Subscription {
            bbox: Some(bbox),
            ..Subscription::all()
        };
        assert!(sub.matches(&sample_event()));

        let bbox_far = Bbox::new(GeoPoint::new(20.0, 20.0), GeoPoint::new(30.0, 30.0));
        let sub = Subscription {
            bbox: Some(bbox_far),
            ..Subscription::all()
        };
        assert!(!sub.matches(&sample_event()));
    }

    #[test]
    fn keyword_filter() {
        let sub = Subscription {
            keywords: vec!["finance".into()],
            ..Subscription::all()
        };
        assert!(sub.matches(&sample_event()));

        let sub = Subscription {
            keywords: vec!["earthquake".into()],
            ..Subscription::all()
        };
        assert!(!sub.matches(&sample_event()));

        // Match via tag
        let sub = Subscription {
            keywords: vec!["market".into()],
            ..Subscription::all()
        };
        assert!(sub.matches(&sample_event()));
    }
}
