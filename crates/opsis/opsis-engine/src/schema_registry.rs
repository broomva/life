//! Schema registry — maps SchemaKey → SchemaDefinition for event validation and discovery.

use std::collections::HashMap;

use opsis_core::feed::SchemaKey;
use opsis_core::schema::{SchemaDefinition, builtin_schemas};

/// Registry of known event schemas.
///
/// Pre-populated with built-in schemas on construction.
/// Feeds register additional schemas at startup.
#[derive(Debug, Clone)]
pub struct SchemaRegistry {
    schemas: HashMap<SchemaKey, SchemaDefinition>,
}

impl SchemaRegistry {
    /// Create a new registry pre-populated with built-in schemas.
    pub fn new() -> Self {
        let mut schemas = HashMap::new();
        for def in builtin_schemas() {
            schemas.insert(def.key.clone(), def);
        }
        Self { schemas }
    }

    /// Register a schema definition. Overwrites if key already exists.
    pub fn register(&mut self, def: SchemaDefinition) {
        self.schemas.insert(def.key.clone(), def);
    }

    /// Look up a schema by key.
    pub fn lookup(&self, key: &SchemaKey) -> Option<&SchemaDefinition> {
        self.schemas.get(key)
    }

    /// All registered schemas.
    pub fn all(&self) -> Vec<&SchemaDefinition> {
        self.schemas.values().collect()
    }

    /// Check if a schema key is known (for advisory validation).
    pub fn is_known(&self, key: &SchemaKey) -> bool {
        self.schemas.contains_key(key)
    }
}

impl Default for SchemaRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opsis_core::schema::{OpsisEventKindHint, SchemaProducer};

    #[test]
    fn new_registry_has_builtins() {
        let reg = SchemaRegistry::new();
        assert!(reg.lookup(&SchemaKey::new("usgs.geojson.v1")).is_some());
        assert!(
            reg.lookup(&SchemaKey::new("openmeteo.current.v1"))
                .is_some()
        );
        assert!(reg.lookup(&SchemaKey::new("gaia.v1")).is_some());
        assert!(reg.lookup(&SchemaKey::new("arcan.agent.v1")).is_some());
    }

    #[test]
    fn register_custom_schema() {
        let mut reg = SchemaRegistry::new();
        let custom = SchemaDefinition {
            key: SchemaKey::new("custom.feed.v1"),
            version: 1,
            description: "Custom feed".into(),
            producer: SchemaProducer::Feed,
            event_kind_hint: OpsisEventKindHint::WorldObservation,
            domain_hint: None,
        };
        reg.register(custom);
        assert!(reg.lookup(&SchemaKey::new("custom.feed.v1")).is_some());
        assert_eq!(reg.all().len(), 7);
    }

    #[test]
    fn all_returns_all_schemas() {
        let reg = SchemaRegistry::new();
        assert_eq!(reg.all().len(), 6);
    }

    #[test]
    fn unknown_key_returns_none() {
        let reg = SchemaRegistry::new();
        assert!(reg.lookup(&SchemaKey::new("nonexistent.v1")).is_none());
        assert!(!reg.is_known(&SchemaKey::new("nonexistent.v1")));
    }
}
