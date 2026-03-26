//! A2A protocol types following the a2a-protocol.org specification.
//!
//! These types map 1:1 to the A2A JSON schema for Agent Cards,
//! tasks, messages, and JSON-RPC envelopes.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Agent Card (Discovery)
// ---------------------------------------------------------------------------

/// A2A Agent Card — the discovery document for an agent.
/// Published at `/.well-known/agent-card.json` (RFC 8615).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentCard {
    /// Schema version (always "1.0")
    pub schema_version: String,
    /// Human-readable agent name
    pub name: String,
    /// Detailed description of the agent's purpose
    pub description: String,
    /// Primary A2A endpoint URL
    pub url: String,
    /// Semantic version (e.g., "1.0.0")
    pub version: String,
    /// Provider information
    pub provider: AgentProvider,
    /// Protocol capabilities
    pub capabilities: AgentCapabilities,
    /// Supported authentication schemes (at least one required)
    pub auth_schemes: Vec<AuthScheme>,
    /// Agent skills/capabilities
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<AgentSkill>,
    /// Default input MIME types
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub default_input_modes: Vec<String>,
    /// Default output MIME types
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub default_output_modes: Vec<String>,
    /// Documentation URL
    #[serde(skip_serializing_if = "Option::is_none")]
    pub documentation_url: Option<String>,
    /// Whether the agent supports streaming (SSE)
    #[serde(default)]
    pub supports_streaming: bool,
    /// Whether the agent supports push notifications
    #[serde(default)]
    pub supports_push_notifications: bool,
    /// Security card — ed25519 public key and signature for card verification
    #[serde(skip_serializing_if = "Option::is_none")]
    pub security_card: Option<SecurityCard>,
}

/// Signed security card for agent authentication.
/// Contains the ed25519 public key and a signature over the card payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecurityCard {
    /// Algorithm (always "ed25519")
    pub algorithm: String,
    /// Base64-encoded ed25519 public key (32 bytes)
    pub public_key: String,
    /// Base64-encoded ed25519 signature over the canonical JSON of the card
    /// (with security_card field set to null before signing)
    pub signature: String,
    /// ISO 8601 timestamp of when this card was signed
    pub signed_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentProvider {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentCapabilities {
    /// A2A protocol version supported
    pub a2a_version: String,
    /// Whether the agent supports streaming responses
    #[serde(default)]
    pub streaming: bool,
    /// Whether the agent supports push notifications
    #[serde(default)]
    pub push_notifications: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum AuthScheme {
    #[serde(rename = "apiKey")]
    ApiKey,
    #[serde(rename = "httpBasic")]
    HttpBasic,
    #[serde(rename = "bearer")]
    Bearer,
    #[serde(rename = "oauth2")]
    OAuth2 {
        #[serde(skip_serializing_if = "Option::is_none")]
        authorization_url: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        token_url: Option<String>,
    },
    #[serde(rename = "oidc")]
    Oidc {
        #[serde(skip_serializing_if = "Option::is_none")]
        issuer: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSkill {
    /// Unique skill identifier
    pub id: String,
    /// Human-readable skill name
    pub name: String,
    /// Skill description
    pub description: String,
    /// Tags for search/categorization
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Example use cases
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub examples: Vec<String>,
    /// Supported input MIME types
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub input_modes: Vec<String>,
    /// Supported output MIME types
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub output_modes: Vec<String>,
}

// ---------------------------------------------------------------------------
// Task Lifecycle
// ---------------------------------------------------------------------------

/// A2A task state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Submitted,
    Working,
    InputRequired,
    AuthRequired,
    Completed,
    Failed,
    Canceled,
    Rejected,
    Unknown,
}

/// A2A Task object returned in JSON-RPC responses.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    /// Unique task identifier
    pub id: String,
    /// Context ID for conversational continuity
    pub context_id: String,
    /// Current task status
    pub status: TaskStatus,
    /// Artifacts produced by the task
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<Artifact>,
    /// Message history
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub history: Vec<TaskMessage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskStatus {
    pub state: TaskState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<TaskError>,
    /// ISO 8601 timestamp
    pub timestamp: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskError {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Artifact {
    pub index: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub parts: Vec<ArtifactPart>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ArtifactPart {
    #[serde(rename = "text")]
    Text {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        mime_type: Option<String>,
    },
    #[serde(rename = "data")]
    Data {
        data: String,
        mime_type: String,
    },
    #[serde(rename = "file")]
    File {
        uri: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        mime_type: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskMessage {
    pub role: String,
    pub parts: Vec<MessagePart>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum MessagePart {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "data")]
    Data {
        data: serde_json::Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        mime_type: Option<String>,
    },
}

// ---------------------------------------------------------------------------
// JSON-RPC 2.0
// ---------------------------------------------------------------------------

/// JSON-RPC 2.0 request envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub method: String,
    #[serde(default)]
    pub params: serde_json::Value,
    pub id: serde_json::Value,
}

/// JSON-RPC 2.0 response envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
    pub id: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl JsonRpcResponse {
    pub fn success(id: serde_json::Value, result: serde_json::Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            result: Some(result),
            error: None,
            id,
        }
    }

    pub fn error(id: serde_json::Value, code: i32, message: String) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            result: None,
            error: Some(JsonRpcError {
                code,
                message,
                data: None,
            }),
            id,
        }
    }
}

// ---------------------------------------------------------------------------
// JSON-RPC method params
// ---------------------------------------------------------------------------

/// Params for message/send and message/stream
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageSendParams {
    /// Target agent URL or ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    /// Message to send
    pub message: TaskMessage,
    /// Existing task ID (for follow-up messages)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    /// Context ID for grouping related tasks
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_id: Option<String>,
}

/// Params for tasks/get
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskGetParams {
    pub id: String,
    /// Whether to include message history
    #[serde(default)]
    pub include_history: bool,
}

/// Params for tasks/cancel
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskCancelParams {
    pub id: String,
}

/// Params for message/stream (SSE)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageStreamParams {
    /// Target agent URL or ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    /// Message to send
    pub message: TaskMessage,
    /// Existing task ID (for follow-up messages)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    /// Context ID for grouping related tasks
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_id: Option<String>,
}

/// SSE event for task streaming updates.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum TaskStreamEvent {
    /// Task status changed
    #[serde(rename = "status")]
    StatusUpdate { task_id: String, status: TaskStatus },
    /// New artifact produced
    #[serde(rename = "artifact")]
    ArtifactUpdate {
        task_id: String,
        artifact: Artifact,
    },
    /// New message in task conversation
    #[serde(rename = "message")]
    MessageUpdate {
        task_id: String,
        message: TaskMessage,
    },
    /// Task completed (terminal event)
    #[serde(rename = "done")]
    Done { task_id: String, task: Task },
}

// ---------------------------------------------------------------------------
// JSON-RPC error codes
// ---------------------------------------------------------------------------

pub mod error_codes {
    pub const PARSE_ERROR: i32 = -32700;
    pub const INVALID_REQUEST: i32 = -32600;
    pub const METHOD_NOT_FOUND: i32 = -32601;
    pub const INVALID_PARAMS: i32 = -32602;
    pub const INTERNAL_ERROR: i32 = -32603;
    pub const TASK_NOT_FOUND: i32 = -32001;
    pub const AGENT_NOT_FOUND: i32 = -32002;
    pub const TASK_STATE_ERROR: i32 = -32003;
}
