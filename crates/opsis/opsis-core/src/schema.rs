//! Schema definitions — metadata describing known event schemas.

use serde::{Deserialize, Serialize};

use crate::feed::SchemaKey;
use crate::state::StateDomain;

/// What kind of entity produces events under this schema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SchemaProducer {
    Feed,
    Agent,
    Gaia,
    System,
}

/// Hint for the expected OpsisEventKind variants under this schema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum OpsisEventKindHint {
    WorldObservation,
    AgentObservation,
    AgentAlert,
    GaiaCorrelation,
    GaiaAnomaly,
    Custom,
}

/// Metadata describing a known event schema.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaDefinition {
    pub key: SchemaKey,
    pub version: u32,
    pub description: String,
    pub producer: SchemaProducer,
    pub event_kind_hint: OpsisEventKindHint,
    pub domain_hint: Option<StateDomain>,
}

/// Built-in schemas shipped with Opsis.
pub fn builtin_schemas() -> Vec<SchemaDefinition> {
    vec![
        SchemaDefinition {
            key: SchemaKey::new("usgs.geojson.v1"),
            version: 1,
            description: "USGS earthquake GeoJSON feed".into(),
            producer: SchemaProducer::Feed,
            event_kind_hint: OpsisEventKindHint::WorldObservation,
            domain_hint: Some(StateDomain::Emergency),
        },
        SchemaDefinition {
            key: SchemaKey::new("openmeteo.current.v1"),
            version: 1,
            description: "Open-Meteo current weather conditions".into(),
            producer: SchemaProducer::Feed,
            event_kind_hint: OpsisEventKindHint::WorldObservation,
            domain_hint: Some(StateDomain::Weather),
        },
        SchemaDefinition {
            key: SchemaKey::new("gaia.v1"),
            version: 1,
            description: "Gaia cross-domain correlations and anomalies".into(),
            producer: SchemaProducer::Gaia,
            event_kind_hint: OpsisEventKindHint::GaiaCorrelation,
            domain_hint: None,
        },
        SchemaDefinition {
            key: SchemaKey::new("arcan.agent.v1"),
            version: 1,
            description: "Arcan agent consciousness stream events".into(),
            producer: SchemaProducer::Agent,
            event_kind_hint: OpsisEventKindHint::AgentObservation,
            domain_hint: None,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_schemas_have_unique_keys() {
        let schemas = builtin_schemas();
        let keys: Vec<_> = schemas.iter().map(|s| &s.key).collect();
        for (i, k) in keys.iter().enumerate() {
            assert!(!keys[i + 1..].contains(k), "duplicate key: {k}");
        }
    }

    #[test]
    fn schema_definition_json_roundtrip() {
        let def = &builtin_schemas()[0];
        let json = serde_json::to_string(def).unwrap();
        let restored: SchemaDefinition = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.key, def.key);
        assert_eq!(restored.version, def.version);
    }

    #[test]
    fn arcan_agent_schema_exists() {
        let schemas = builtin_schemas();
        assert!(
            schemas
                .iter()
                .any(|s| s.key == SchemaKey::new("arcan.agent.v1"))
        );
    }

    #[test]
    fn four_builtin_schemas() {
        assert_eq!(builtin_schemas().len(), 4);
    }
}
