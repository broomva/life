use serde::{Deserialize, Serialize};
use ulid::Ulid;

use crate::event::StateEvent;
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

/// A subscription filter — decides which [`StateEvent`]s a client receives.
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
    /// Only match events whose summary or tags contain any of these keywords.
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
    pub fn matches(&self, event: &StateEvent) -> bool {
        // Domain filter
        if !self.domains.is_empty() && !self.domains.contains(&event.domain) {
            return false;
        }

        // Severity filter
        if event.severity < self.severity_threshold {
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

        // Keyword filter
        if !self.keywords.is_empty() {
            let has_keyword = self.keywords.iter().any(|kw| {
                let kw_lower = kw.to_lowercase();
                event.summary.to_lowercase().contains(&kw_lower)
                    || event
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
    use crate::event::EventId;
    use crate::feed::FeedSource;
    use crate::spatial::GeoPoint;

    fn sample_event() -> StateEvent {
        StateEvent {
            id: EventId::default(),
            tick: WorldTick(1),
            domain: StateDomain::Finance,
            location: Some(GeoPoint::new(5.0, 5.0)),
            severity: 0.7,
            summary: "Stock market surge".into(),
            source: FeedSource::new("test"),
            tags: vec!["finance".into(), "market".into()],
            raw_ref: EventId::default(),
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
            keywords: vec!["surge".into()],
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
