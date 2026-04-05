//! Agent Card generation from SpacetimeDB data.
//!
//! Converts Life agent listings into valid A2A Agent Cards.
//! Supports ed25519 signing for agent authentication.

use crate::types::{
    AgentCapabilities, AgentCard, AgentProvider, AgentSkill, AuthScheme, SecurityCard,
};
use base64::Engine;
use ed25519_dalek::{Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};

/// Configuration for generating Agent Cards.
pub struct AgentCardConfig {
    /// Base URL where this bridge server is accessible
    pub base_url: String,
    /// A2A protocol version
    pub a2a_version: String,
    /// Optional signing key for Agent Card authentication
    pub signing_key: Option<SigningKey>,
}

impl Default for AgentCardConfig {
    fn default() -> Self {
        Self {
            base_url: "http://localhost:3001".to_string(),
            a2a_version: "1.0".to_string(),
            signing_key: None,
        }
    }
}

impl AgentCardConfig {
    /// Create a config with a new random signing key.
    pub fn with_new_signing_key(base_url: String, a2a_version: String) -> Self {
        let signing_key = SigningKey::generate(&mut rand::rngs::OsRng);
        Self {
            base_url,
            a2a_version,
            signing_key: Some(signing_key),
        }
    }

    /// Create a config from an existing ed25519 secret key (32 bytes, base64-encoded).
    pub fn with_signing_key_b64(
        base_url: String,
        a2a_version: String,
        key_b64: &str,
    ) -> Result<Self, String> {
        let key_bytes = base64::engine::general_purpose::STANDARD
            .decode(key_b64)
            .map_err(|e| format!("Invalid base64 key: {}", e))?;
        let key_array: [u8; 32] = key_bytes
            .try_into()
            .map_err(|_| "Signing key must be 32 bytes".to_string())?;
        Ok(Self {
            base_url,
            a2a_version,
            signing_key: Some(SigningKey::from_bytes(&key_array)),
        })
    }

    /// Get the public key as base64 (for display/sharing).
    pub fn public_key_b64(&self) -> Option<String> {
        self.signing_key.as_ref().map(|sk| {
            base64::engine::general_purpose::STANDARD.encode(sk.verifying_key().as_bytes())
        })
    }
}

/// Sign an Agent Card with the config's signing key.
/// Returns the SecurityCard to embed in the AgentCard.
fn sign_agent_card(config: &AgentCardConfig, card: &AgentCard) -> Option<SecurityCard> {
    let signing_key = config.signing_key.as_ref()?;

    // Create a copy of the card with security_card = None for canonical form
    let mut canonical = card.clone();
    canonical.security_card = None;
    let canonical_json = serde_json::to_string(&canonical).ok()?;

    let signature = signing_key.sign(canonical_json.as_bytes());
    let b64 = base64::engine::general_purpose::STANDARD;

    Some(SecurityCard {
        algorithm: "ed25519".to_string(),
        public_key: b64.encode(signing_key.verifying_key().as_bytes()),
        signature: b64.encode(signature.to_bytes()),
        signed_at: chrono::Utc::now().to_rfc3339(),
    })
}

/// Verify an Agent Card's security card signature.
pub fn verify_agent_card(card: &AgentCard) -> Result<bool, String> {
    let security = card
        .security_card
        .as_ref()
        .ok_or("No security card present")?;

    if security.algorithm != "ed25519" {
        return Err(format!("Unsupported algorithm: {}", security.algorithm));
    }

    let b64 = base64::engine::general_purpose::STANDARD;

    let pk_bytes = b64
        .decode(&security.public_key)
        .map_err(|e| format!("Invalid public key: {}", e))?;
    let pk_array: [u8; 32] = pk_bytes
        .try_into()
        .map_err(|_| "Public key must be 32 bytes".to_string())?;
    let verifying_key = VerifyingKey::from_bytes(&pk_array)
        .map_err(|e| format!("Invalid verifying key: {}", e))?;

    let sig_bytes = b64
        .decode(&security.signature)
        .map_err(|e| format!("Invalid signature: {}", e))?;
    let sig_array: [u8; 64] = sig_bytes
        .try_into()
        .map_err(|_| "Signature must be 64 bytes".to_string())?;
    let signature = ed25519_dalek::Signature::from_bytes(&sig_array);

    // Reconstruct canonical form
    let mut canonical = card.clone();
    canonical.security_card = None;
    let canonical_json =
        serde_json::to_string(&canonical).map_err(|e| format!("Serialization failed: {}", e))?;

    use ed25519_dalek::Verifier;
    Ok(verifying_key
        .verify(canonical_json.as_bytes(), &signature)
        .is_ok())
}

/// Data from SpacetimeDB needed to build an Agent Card.
/// Populated by the bridge module from SpacetimeDB subscriptions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListingData {
    pub agent_id: String,
    pub name: String,
    pub description: String,
    pub version: String,
    pub url: String,
    pub provider_name: String,
    pub provider_url: Option<String>,
    pub input_modes: String,
    pub output_modes: String,
    pub supports_streaming: bool,
    pub supports_push_notifications: bool,
    pub documentation_url: Option<String>,
    pub skills: Vec<SkillData>,
    pub auth_schemes: Vec<AuthSchemeData>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillData {
    pub skill_id: String,
    pub name: String,
    pub description: String,
    pub tags: String,
    pub examples: Option<String>,
    pub input_modes: Option<String>,
    pub output_modes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthSchemeData {
    pub scheme_type: String,
    pub config: Option<String>,
}

/// Generate an A2A Agent Card from a SpacetimeDB listing.
pub fn generate_agent_card(config: &AgentCardConfig, listing: &ListingData) -> AgentCard {
    let skills = listing
        .skills
        .iter()
        .map(|s| AgentSkill {
            id: s.skill_id.clone(),
            name: s.name.clone(),
            description: s.description.clone(),
            tags: parse_comma_separated(&s.tags),
            examples: s
                .examples
                .as_ref()
                .map(|e| e.split('|').map(|s| s.trim().to_string()).collect())
                .unwrap_or_default(),
            input_modes: s
                .input_modes
                .as_ref()
                .map(|m| parse_comma_separated(m))
                .unwrap_or_default(),
            output_modes: s
                .output_modes
                .as_ref()
                .map(|m| parse_comma_separated(m))
                .unwrap_or_default(),
        })
        .collect();

    let auth_schemes = if listing.auth_schemes.is_empty() {
        // Default to Bearer token if no schemes configured
        vec![AuthScheme::Bearer]
    } else {
        listing
            .auth_schemes
            .iter()
            .map(|a| match a.scheme_type.as_str() {
                "ApiKey" => AuthScheme::ApiKey,
                "HttpBasic" => AuthScheme::HttpBasic,
                "Bearer" => AuthScheme::Bearer,
                "OAuth2" => {
                    let (auth_url, token_url) = parse_oauth2_config(a.config.as_deref());
                    AuthScheme::OAuth2 {
                        authorization_url: auth_url,
                        token_url,
                    }
                }
                "Oidc" => AuthScheme::Oidc {
                    issuer: a.config.clone(),
                },
                _ => AuthScheme::Bearer,
            })
            .collect()
    };

    let mut card = AgentCard {
        schema_version: "1.0".to_string(),
        name: listing.name.clone(),
        description: listing.description.clone(),
        url: format!("{}/agents/{}", config.base_url, listing.agent_id),
        version: listing.version.clone(),
        provider: AgentProvider {
            name: listing.provider_name.clone(),
            url: listing.provider_url.clone(),
        },
        capabilities: AgentCapabilities {
            a2a_version: config.a2a_version.clone(),
            streaming: listing.supports_streaming,
            push_notifications: listing.supports_push_notifications,
        },
        auth_schemes,
        skills,
        default_input_modes: parse_comma_separated(&listing.input_modes),
        default_output_modes: parse_comma_separated(&listing.output_modes),
        documentation_url: listing.documentation_url.clone(),
        supports_streaming: listing.supports_streaming,
        supports_push_notifications: listing.supports_push_notifications,
        security_card: None,
    };

    // Sign the card if a signing key is configured
    card.security_card = sign_agent_card(config, &card);

    card
}

/// Generate the well-known directory listing all available agents.
pub fn generate_directory(config: &AgentCardConfig, listings: &[ListingData]) -> serde_json::Value {
    let agents: Vec<serde_json::Value> = listings
        .iter()
        .map(|l| {
            serde_json::json!({
                "agentId": l.agent_id,
                "name": l.name,
                "description": l.description,
                "url": format!("{}/agents/{}", config.base_url, l.agent_id),
                "version": l.version,
                "cardUrl": format!("{}/agents/{}/.well-known/agent-card.json", config.base_url, l.agent_id),
            })
        })
        .collect();

    serde_json::json!({
        "schemaVersion": "1.0",
        "agents": agents,
    })
}

fn parse_comma_separated(s: &str) -> Vec<String> {
    s.split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn parse_oauth2_config(config: Option<&str>) -> (Option<String>, Option<String>) {
    let Some(config) = config else {
        return (None, None);
    };
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(config) else {
        return (None, None);
    };
    let auth_url = parsed
        .get("authorization_url")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let token_url = parsed
        .get("token_url")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    (auth_url, token_url)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_listing() -> ListingData {
        ListingData {
            agent_id: "life-code-review".to_string(),
            name: "Life Code Review Agent".to_string(),
            description: "Automated code review agent".to_string(),
            version: "1.0.0".to_string(),
            url: "http://localhost:8080".to_string(),
            provider_name: "BroomVA".to_string(),
            provider_url: Some("https://broomva.tech".to_string()),
            input_modes: "text/plain,application/json".to_string(),
            output_modes: "text/plain,application/json".to_string(),
            supports_streaming: true,
            supports_push_notifications: false,
            documentation_url: None,
            skills: vec![SkillData {
                skill_id: "code-review".to_string(),
                name: "Code Review".to_string(),
                description: "Reviews pull requests".to_string(),
                tags: "code,review,pr".to_string(),
                examples: Some("Review this PR|Check for security issues".to_string()),
                input_modes: None,
                output_modes: None,
            }],
            auth_schemes: vec![],
        }
    }

    #[test]
    fn test_generate_agent_card() {
        let config = AgentCardConfig::default();
        let listing = sample_listing();

        let card = generate_agent_card(&config, &listing);
        assert_eq!(card.schema_version, "1.0");
        assert_eq!(card.name, "Life Code Review Agent");
        assert_eq!(card.skills.len(), 1);
        assert_eq!(card.skills[0].tags, vec!["code", "review", "pr"]);
        assert!(card.supports_streaming);
        // Default auth scheme when none configured
        assert_eq!(card.auth_schemes.len(), 1);
        // No signing key → no security card
        assert!(card.security_card.is_none());
    }

    #[test]
    fn test_signed_agent_card() {
        let config = AgentCardConfig::with_new_signing_key(
            "http://localhost:3001".to_string(),
            "1.0".to_string(),
        );
        let listing = sample_listing();

        let card = generate_agent_card(&config, &listing);

        // Security card should be present
        let security = card.security_card.as_ref().expect("security card missing");
        assert_eq!(security.algorithm, "ed25519");
        assert!(!security.public_key.is_empty());
        assert!(!security.signature.is_empty());
        assert!(!security.signed_at.is_empty());

        // Verify the signature
        assert!(verify_agent_card(&card).unwrap());
    }

    #[test]
    fn test_tampered_card_fails_verification() {
        let config = AgentCardConfig::with_new_signing_key(
            "http://localhost:3001".to_string(),
            "1.0".to_string(),
        );
        let listing = sample_listing();

        let mut card = generate_agent_card(&config, &listing);
        assert!(verify_agent_card(&card).unwrap());

        // Tamper with the card
        card.name = "TAMPERED".to_string();
        assert!(!verify_agent_card(&card).unwrap());
    }

    #[test]
    fn test_verify_unsigned_card_errors() {
        let config = AgentCardConfig::default();
        let listing = sample_listing();

        let card = generate_agent_card(&config, &listing);
        assert!(verify_agent_card(&card).is_err());
    }

    #[test]
    fn test_signing_key_roundtrip() {
        let config = AgentCardConfig::with_new_signing_key(
            "http://localhost:3001".to_string(),
            "1.0".to_string(),
        );

        // Export the private key
        let sk_b64 = base64::engine::general_purpose::STANDARD
            .encode(config.signing_key.as_ref().unwrap().to_bytes());

        // Recreate from exported key
        let config2 = AgentCardConfig::with_signing_key_b64(
            "http://localhost:3001".to_string(),
            "1.0".to_string(),
            &sk_b64,
        )
        .unwrap();

        assert_eq!(config.public_key_b64(), config2.public_key_b64());
    }

    #[test]
    fn test_parse_comma_separated() {
        let result = parse_comma_separated("text/plain, application/json, image/png");
        assert_eq!(result, vec!["text/plain", "application/json", "image/png"]);
    }

    #[test]
    fn test_parse_empty() {
        let result = parse_comma_separated("");
        assert!(result.is_empty());
    }

    #[test]
    fn test_agent_card_json_roundtrip() {
        let config = AgentCardConfig::with_new_signing_key(
            "http://localhost:3001".to_string(),
            "1.0".to_string(),
        );
        let listing = sample_listing();
        let card = generate_agent_card(&config, &listing);

        // Serialize and deserialize
        let json = serde_json::to_string_pretty(&card).unwrap();
        let deserialized: AgentCard = serde_json::from_str(&json).unwrap();

        assert_eq!(card.name, deserialized.name);
        assert_eq!(card.url, deserialized.url);
        assert_eq!(card.skills.len(), deserialized.skills.len());
        assert!(deserialized.security_card.is_some());

        // Verify the deserialized card
        assert!(verify_agent_card(&deserialized).unwrap());
    }

    #[test]
    fn test_directory_generation() {
        let config = AgentCardConfig::default();
        let listings = vec![sample_listing()];
        let directory = generate_directory(&config, &listings);

        let agents = directory["agents"].as_array().unwrap();
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0]["agentId"], "life-code-review");
        assert!(agents[0]["cardUrl"]
            .as_str()
            .unwrap()
            .contains("well-known"));
    }

    #[test]
    fn test_oauth2_auth_scheme() {
        let config = AgentCardConfig::default();
        let mut listing = sample_listing();
        listing.auth_schemes = vec![AuthSchemeData {
            scheme_type: "OAuth2".to_string(),
            config: Some(
                r#"{"authorization_url":"https://auth.example.com/authorize","token_url":"https://auth.example.com/token"}"#.to_string(),
            ),
        }];

        let card = generate_agent_card(&config, &listing);
        assert_eq!(card.auth_schemes.len(), 1);
        match &card.auth_schemes[0] {
            AuthScheme::OAuth2 {
                authorization_url,
                token_url,
            } => {
                assert_eq!(
                    authorization_url.as_deref(),
                    Some("https://auth.example.com/authorize")
                );
                assert_eq!(
                    token_url.as_deref(),
                    Some("https://auth.example.com/token")
                );
            }
            _ => panic!("Expected OAuth2 scheme"),
        }
    }
}
