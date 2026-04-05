//! SpacetimeDB bridge — connects the A2A HTTP server to Spaces.
//!
//! This module maintains an in-memory cache of agent listings populated
//! from SpacetimeDB subscriptions. In production, it would use the
//! SpacetimeDB SDK's real-time pub/sub. For now, it uses a simple
//! in-memory store that can be populated via HTTP API or file import.

use crate::agent_card::{AgentCardConfig, AuthSchemeData, ListingData, SkillData};
use crate::types::*;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};

/// Bridge between A2A HTTP endpoints and SpacetimeDB state.
pub struct SpacesBridge {
    /// Agent card generation config
    pub config: AgentCardConfig,
    /// In-memory agent listings (agent_id -> ListingData)
    listings: RwLock<HashMap<String, ListingData>>,
    /// In-memory task store (task_id -> TaskData)
    tasks: RwLock<HashMap<String, TaskData>>,
    /// Broadcast channel for task streaming events
    event_tx: broadcast::Sender<TaskStreamEvent>,
}

/// Internal task representation bridging SpacetimeDB and A2A.
#[derive(Debug, Clone)]
#[expect(dead_code)]
struct TaskData {
    task_id: String,
    context_id: String,
    agent_id: String,
    state: TaskState,
    status_message: Option<String>,
    error_detail: Option<String>,
    artifacts: Vec<ArtifactData>,
    messages: Vec<TaskMessage>,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone)]
struct ArtifactData {
    index: u32,
    name: Option<String>,
    mime_type: String,
    content: String,
}

impl SpacesBridge {
    pub fn new(config: AgentCardConfig) -> Self {
        let (event_tx, _) = broadcast::channel(256);
        Self {
            config,
            listings: RwLock::new(HashMap::new()),
            tasks: RwLock::new(HashMap::new()),
            event_tx,
        }
    }

    /// Subscribe to task streaming events.
    pub fn subscribe_events(&self) -> broadcast::Receiver<TaskStreamEvent> {
        self.event_tx.subscribe()
    }

    /// Emit a task stream event to all subscribers.
    fn emit_event(&self, event: TaskStreamEvent) {
        // Ignore send errors (no subscribers)
        let _ = self.event_tx.send(event);
    }

    // -----------------------------------------------------------------------
    // Agent Card Operations
    // -----------------------------------------------------------------------

    /// Register or update an agent listing.
    pub async fn register_listing(&self, listing: ListingData) {
        let mut listings = self.listings.write().await;
        listings.insert(listing.agent_id.clone(), listing);
    }

    /// Get a specific agent listing.
    pub async fn get_listing(&self, agent_id: &str) -> Option<ListingData> {
        let listings = self.listings.read().await;
        listings.get(agent_id).cloned()
    }

    /// Get all active agent listings.
    pub async fn get_all_listings(&self) -> Vec<ListingData> {
        let listings = self.listings.read().await;
        listings.values().cloned().collect()
    }

    // -----------------------------------------------------------------------
    // Task Operations
    // -----------------------------------------------------------------------

    /// Create a new A2A task.
    pub async fn create_task(
        &self,
        agent_id: &str,
        context_id: &str,
        message: &TaskMessage,
    ) -> Result<Task, anyhow::Error> {
        // Verify agent exists
        let listings = self.listings.read().await;
        if !listings.contains_key(agent_id) {
            anyhow::bail!("Agent '{}' not found", agent_id);
        }
        drop(listings);

        let task_id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now();

        let task_data = TaskData {
            task_id: task_id.clone(),
            context_id: context_id.to_string(),
            agent_id: agent_id.to_string(),
            state: TaskState::Submitted,
            status_message: None,
            error_detail: None,
            artifacts: Vec::new(),
            messages: vec![message.clone()],
            created_at: now,
            updated_at: now,
        };

        let task = task_data_to_task(&task_data, true);
        let mut tasks = self.tasks.write().await;
        tasks.insert(task_id.clone(), task_data);
        drop(tasks);

        // Emit status event
        self.emit_event(TaskStreamEvent::StatusUpdate {
            task_id,
            status: task.status.clone(),
        });

        Ok(task)
    }

    /// Get a task by ID.
    pub async fn get_task(
        &self,
        task_id: &str,
        include_history: bool,
    ) -> Result<Option<Task>, anyhow::Error> {
        let tasks = self.tasks.read().await;
        Ok(tasks
            .get(task_id)
            .map(|td| task_data_to_task(td, include_history)))
    }

    /// Cancel a task.
    pub async fn cancel_task(&self, task_id: &str) -> Result<Task, anyhow::Error> {
        let mut tasks = self.tasks.write().await;
        let task_data = tasks
            .get_mut(task_id)
            .ok_or_else(|| anyhow::anyhow!("Task '{}' not found", task_id))?;

        // Check if task can be canceled
        if matches!(
            task_data.state,
            TaskState::Completed | TaskState::Failed | TaskState::Canceled | TaskState::Rejected
        ) {
            anyhow::bail!(
                "Cannot cancel task in terminal state: {:?}",
                task_data.state
            );
        }

        task_data.state = TaskState::Canceled;
        task_data.status_message = Some("Canceled by requester".to_string());
        task_data.updated_at = chrono::Utc::now();

        let task = task_data_to_task(task_data, false);
        drop(tasks);

        self.emit_event(TaskStreamEvent::Done {
            task_id: task.id.clone(),
            task: task.clone(),
        });

        Ok(task)
    }

    /// Send a follow-up message to an existing task.
    pub async fn send_task_message(
        &self,
        task_id: &str,
        message: &TaskMessage,
    ) -> Result<Task, anyhow::Error> {
        let mut tasks = self.tasks.write().await;
        let task_data = tasks
            .get_mut(task_id)
            .ok_or_else(|| anyhow::anyhow!("Task '{}' not found", task_id))?;

        // Check if task accepts messages
        if matches!(
            task_data.state,
            TaskState::Completed | TaskState::Failed | TaskState::Canceled | TaskState::Rejected
        ) {
            anyhow::bail!(
                "Cannot send messages to task in terminal state: {:?}",
                task_data.state
            );
        }

        task_data.messages.push(message.clone());
        task_data.updated_at = chrono::Utc::now();

        // If task was waiting for input, transition back to working
        if task_data.state == TaskState::InputRequired || task_data.state == TaskState::AuthRequired
        {
            task_data.state = TaskState::Working;
        }

        Ok(task_data_to_task(task_data, true))
    }

    /// Update task state (called by handler agent).
    pub async fn update_task_state(
        &self,
        task_id: &str,
        new_state: TaskState,
        status_message: Option<String>,
        error_detail: Option<String>,
    ) -> Result<Task, anyhow::Error> {
        let mut tasks = self.tasks.write().await;
        let task_data = tasks
            .get_mut(task_id)
            .ok_or_else(|| anyhow::anyhow!("Task '{}' not found", task_id))?;

        task_data.state = new_state;
        task_data.status_message = status_message;
        task_data.error_detail = error_detail;
        task_data.updated_at = chrono::Utc::now();

        Ok(task_data_to_task(task_data, false))
    }

    /// Add an artifact to a task (called by handler agent).
    pub async fn add_artifact(
        &self,
        task_id: &str,
        name: Option<String>,
        mime_type: String,
        content: String,
    ) -> Result<Task, anyhow::Error> {
        let mut tasks = self.tasks.write().await;
        let task_data = tasks
            .get_mut(task_id)
            .ok_or_else(|| anyhow::anyhow!("Task '{}' not found", task_id))?;

        if task_data.state != TaskState::Working {
            anyhow::bail!("Can only add artifacts to tasks in Working state");
        }

        let index = task_data.artifacts.len() as u32;
        task_data.artifacts.push(ArtifactData {
            index,
            name,
            mime_type,
            content,
        });
        task_data.updated_at = chrono::Utc::now();

        Ok(task_data_to_task(task_data, false))
    }
}

/// Convert internal TaskData to A2A Task response.
fn task_data_to_task(data: &TaskData, include_history: bool) -> Task {
    let artifacts = data
        .artifacts
        .iter()
        .map(|a| Artifact {
            index: a.index,
            name: a.name.clone(),
            parts: vec![ArtifactPart::Text {
                text: a.content.clone(),
                mime_type: Some(a.mime_type.clone()),
            }],
        })
        .collect();

    let history = if include_history {
        data.messages.clone()
    } else {
        Vec::new()
    };

    let error = data.error_detail.as_ref().map(|e| TaskError {
        code: "EXECUTION_ERROR".to_string(),
        message: e.clone(),
    });

    Task {
        id: data.task_id.clone(),
        context_id: data.context_id.clone(),
        status: TaskStatus {
            state: data.state,
            message: data.status_message.clone(),
            error,
            timestamp: data.updated_at.to_rfc3339(),
        },
        artifacts,
        history,
    }
}

/// Create a pre-populated bridge with sample Life agents for testing.
pub fn create_sample_bridge() -> Arc<SpacesBridge> {
    let config = AgentCardConfig::default();
    let bridge = Arc::new(SpacesBridge::new(config));

    // Register sample agents synchronously for startup
    let rt = tokio::runtime::Handle::current();
    rt.block_on(async {
        bridge
            .register_listing(ListingData {
                agent_id: "life-arcan".to_string(),
                name: "Arcan Agent Runtime".to_string(),
                description: "Core agent runtime daemon — executes agent loops, manages LLM providers, and coordinates tool execution".to_string(),
                version: "0.1.0".to_string(),
                url: "http://localhost:8080".to_string(),
                provider_name: "BroomVA".to_string(),
                provider_url: Some("https://broomva.tech".to_string()),
                input_modes: "text/plain,application/json".to_string(),
                output_modes: "text/plain,application/json".to_string(),
                supports_streaming: true,
                supports_push_notifications: false,
                documentation_url: Some("https://broomva.tech/docs/arcan".to_string()),
                skills: vec![
                    SkillData {
                        skill_id: "code-generation".to_string(),
                        name: "Code Generation".to_string(),
                        description: "Generate code from natural language descriptions".to_string(),
                        tags: "code,generation,llm".to_string(),
                        examples: Some("Write a Rust function|Generate a REST API".to_string()),
                        input_modes: None,
                        output_modes: None,
                    },
                    SkillData {
                        skill_id: "code-review".to_string(),
                        name: "Code Review".to_string(),
                        description: "Review code for bugs, security issues, and best practices".to_string(),
                        tags: "code,review,security".to_string(),
                        examples: Some("Review this PR|Check for vulnerabilities".to_string()),
                        input_modes: None,
                        output_modes: None,
                    },
                ],
                auth_schemes: vec![AuthSchemeData {
                    scheme_type: "Bearer".to_string(),
                    config: None,
                }],
            })
            .await;

        bridge
            .register_listing(ListingData {
                agent_id: "life-lago".to_string(),
                name: "Lago Persistence Engine".to_string(),
                description: "Event-sourced persistence — append-only journal, content-addressed blob store, and knowledge index".to_string(),
                version: "0.1.0".to_string(),
                url: "http://localhost:8081".to_string(),
                provider_name: "BroomVA".to_string(),
                provider_url: Some("https://broomva.tech".to_string()),
                input_modes: "application/json".to_string(),
                output_modes: "application/json,text/event-stream".to_string(),
                supports_streaming: true,
                supports_push_notifications: false,
                documentation_url: None,
                skills: vec![SkillData {
                    skill_id: "knowledge-search".to_string(),
                    name: "Knowledge Search".to_string(),
                    description: "Search the knowledge graph for relevant information".to_string(),
                    tags: "search,knowledge,rag".to_string(),
                    examples: Some("Find docs about authentication|Search for API patterns".to_string()),
                    input_modes: None,
                    output_modes: None,
                }],
                auth_schemes: vec![AuthSchemeData {
                    scheme_type: "Bearer".to_string(),
                    config: None,
                }],
            })
            .await;
    });

    bridge
}
