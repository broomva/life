//! gRPC transport layer for A2A protocol.
//!
//! Implements the A2AService gRPC service, delegating to the SpacesBridge.

use crate::agent_card::generate_agent_card;
use crate::bridge::SpacesBridge;
use crate::types::{MessagePart, TaskMessage};
use std::sync::Arc;
use tonic::{Request, Response, Status};

pub mod pb {
    tonic::include_proto!("a2a.v1");
}

pub struct A2AGrpcService {
    bridge: Arc<SpacesBridge>,
}

impl A2AGrpcService {
    pub fn new(bridge: Arc<SpacesBridge>) -> Self {
        Self { bridge }
    }
}

#[tonic::async_trait]
impl pb::a2a_service_server::A2aService for A2AGrpcService {
    async fn send_message(
        &self,
        request: Request<pb::SendMessageRequest>,
    ) -> Result<Response<pb::SendMessageResponse>, Status> {
        let req = request.into_inner();

        // Convert proto message to internal type
        let message = proto_msg_to_task_message(&req.message);

        // Check for follow-up vs new task
        if let Some(task_id) = req.task_id {
            let task = self
                .bridge
                .send_task_message(&task_id, &message)
                .await
                .map_err(|e| Status::internal(e.to_string()))?;
            let proto_task = task_to_proto(&task);
            return Ok(Response::new(pb::SendMessageResponse {
                task: Some(proto_task),
            }));
        }

        let context_id = req
            .context_id
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

        let task = self
            .bridge
            .create_task(&req.agent_id, &context_id, &message)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        let proto_task = task_to_proto(&task);
        Ok(Response::new(pb::SendMessageResponse {
            task: Some(proto_task),
        }))
    }

    type StreamMessageStream =
        tokio_stream::wrappers::ReceiverStream<Result<pb::TaskEvent, Status>>;

    async fn stream_message(
        &self,
        request: Request<pb::SendMessageRequest>,
    ) -> Result<Response<Self::StreamMessageStream>, Status> {
        let req = request.into_inner();
        let bridge = Arc::clone(&self.bridge);
        let message = proto_msg_to_task_message(&req.message);

        let (tx, rx) = tokio::sync::mpsc::channel(32);

        tokio::spawn(async move {
            let context_id = req
                .context_id
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

            let task = match bridge
                .create_task(&req.agent_id, &context_id, &message)
                .await
            {
                Ok(t) => t,
                Err(e) => {
                    let _ = tx.send(Err(Status::internal(e.to_string()))).await;
                    return;
                }
            };

            // Send initial status
            let _ = tx
                .send(Ok(pb::TaskEvent {
                    event: Some(pb::task_event::Event::StatusUpdate(pb::TaskStatus {
                        state: "submitted".to_string(),
                        message: None,
                        error: None,
                        timestamp: chrono::Utc::now().to_rfc3339(),
                    })),
                }))
                .await;

            // In a real implementation, we'd poll SpacetimeDB for updates.
            // For now, send the task status.
            let _ = tx
                .send(Ok(pb::TaskEvent {
                    event: Some(pb::task_event::Event::StatusUpdate(pb::TaskStatus {
                        state: format!("{:?}", task.status.state).to_lowercase(),
                        message: task.status.message,
                        error: task.status.error.map(|e| pb::TaskError {
                            code: e.code,
                            message: e.message,
                        }),
                        timestamp: task.status.timestamp,
                    })),
                }))
                .await;
        });

        Ok(Response::new(tokio_stream::wrappers::ReceiverStream::new(
            rx,
        )))
    }

    async fn get_task(
        &self,
        request: Request<pb::GetTaskRequest>,
    ) -> Result<Response<pb::GetTaskResponse>, Status> {
        let req = request.into_inner();

        let task = self
            .bridge
            .get_task(&req.task_id, req.include_history)
            .await
            .map_err(|e| Status::internal(e.to_string()))?
            .ok_or_else(|| Status::not_found(format!("Task '{}' not found", req.task_id)))?;

        Ok(Response::new(pb::GetTaskResponse {
            task: Some(task_to_proto(&task)),
        }))
    }

    async fn cancel_task(
        &self,
        request: Request<pb::CancelTaskRequest>,
    ) -> Result<Response<pb::CancelTaskResponse>, Status> {
        let req = request.into_inner();

        let task = self
            .bridge
            .cancel_task(&req.task_id)
            .await
            .map_err(|e| Status::failed_precondition(e.to_string()))?;

        Ok(Response::new(pb::CancelTaskResponse {
            task: Some(task_to_proto(&task)),
        }))
    }

    async fn list_tasks(
        &self,
        _request: Request<pb::ListTasksRequest>,
    ) -> Result<Response<pb::ListTasksResponse>, Status> {
        // TODO: implement filtering when SpacetimeDB queries are available
        Ok(Response::new(pb::ListTasksResponse { tasks: vec![] }))
    }

    async fn get_agent_card(
        &self,
        request: Request<pb::GetAgentCardRequest>,
    ) -> Result<Response<pb::AgentCardResponse>, Status> {
        let req = request.into_inner();

        let listing = self
            .bridge
            .get_listing(&req.agent_id)
            .await
            .ok_or_else(|| Status::not_found(format!("Agent '{}' not found", req.agent_id)))?;

        let card = generate_agent_card(&self.bridge.config, &listing);
        let card_json =
            serde_json::to_string(&card).map_err(|e| Status::internal(e.to_string()))?;

        Ok(Response::new(pb::AgentCardResponse { card_json }))
    }

    async fn list_agents(
        &self,
        _request: Request<pb::ListAgentsRequest>,
    ) -> Result<Response<pb::ListAgentsResponse>, Status> {
        let listings = self.bridge.get_all_listings().await;
        let agents = listings
            .iter()
            .map(|l| pb::AgentSummary {
                agent_id: l.agent_id.clone(),
                name: l.name.clone(),
                description: l.description.clone(),
                url: l.url.clone(),
                version: l.version.clone(),
                card_url: format!(
                    "{}/agents/{}/.well-known/agent-card.json",
                    self.bridge.config.base_url, l.agent_id
                ),
            })
            .collect();

        Ok(Response::new(pb::ListAgentsResponse { agents }))
    }
}

// --- Conversion helpers ---

fn proto_msg_to_task_message(msg: &Option<pb::TaskMessage>) -> TaskMessage {
    let msg = msg.as_ref().cloned().unwrap_or(pb::TaskMessage {
        role: "requester".to_string(),
        parts: vec![],
        timestamp: None,
    });

    TaskMessage {
        role: msg.role,
        parts: msg
            .parts
            .into_iter()
            .filter_map(|p| {
                p.content.map(|c| match c {
                    pb::message_part::Content::Text(t) => MessagePart::Text { text: t.text },
                    pb::message_part::Content::Data(d) => MessagePart::Data {
                        data: serde_json::Value::String(base64::Engine::encode(
                            &base64::engine::general_purpose::STANDARD,
                            &d.data,
                        )),
                        mime_type: Some(d.mime_type),
                    },
                })
            })
            .collect(),
        timestamp: msg.timestamp,
    }
}

fn task_to_proto(task: &crate::types::Task) -> pb::Task {
    pb::Task {
        id: task.id.clone(),
        context_id: task.context_id.clone(),
        status: Some(pb::TaskStatus {
            state: format!("{:?}", task.status.state).to_lowercase(),
            message: task.status.message.clone(),
            error: task.status.error.as_ref().map(|e| pb::TaskError {
                code: e.code.clone(),
                message: e.message.clone(),
            }),
            timestamp: task.status.timestamp.clone(),
        }),
        artifacts: task
            .artifacts
            .iter()
            .map(|a| pb::Artifact {
                index: a.index,
                name: a.name.clone(),
                parts: a
                    .parts
                    .iter()
                    .map(|p| match p {
                        crate::types::ArtifactPart::Text { text, mime_type } => pb::ArtifactPart {
                            content: Some(pb::artifact_part::Content::Text(pb::TextArtifact {
                                text: text.clone(),
                                mime_type: mime_type.clone(),
                            })),
                        },
                        crate::types::ArtifactPart::Data { data, mime_type } => pb::ArtifactPart {
                            content: Some(pb::artifact_part::Content::Data(pb::DataArtifact {
                                data: data.as_bytes().to_vec(),
                                mime_type: mime_type.clone(),
                            })),
                        },
                        crate::types::ArtifactPart::File { uri, mime_type } => pb::ArtifactPart {
                            content: Some(pb::artifact_part::Content::File(pb::FileArtifact {
                                uri: uri.clone(),
                                mime_type: mime_type.clone(),
                            })),
                        },
                    })
                    .collect(),
            })
            .collect(),
        history: task
            .history
            .iter()
            .map(|m| pb::TaskMessage {
                role: m.role.clone(),
                parts: m
                    .parts
                    .iter()
                    .map(|p| match p {
                        MessagePart::Text { text } => pb::MessagePart {
                            content: Some(pb::message_part::Content::Text(pb::TextPart {
                                text: text.clone(),
                            })),
                        },
                        MessagePart::Data { data, mime_type } => pb::MessagePart {
                            content: Some(pb::message_part::Content::Data(pb::DataPart {
                                data: data.to_string().into_bytes(),
                                mime_type: mime_type
                                    .clone()
                                    .unwrap_or_else(|| "application/json".to_string()),
                            })),
                        },
                    })
                    .collect(),
                timestamp: m.timestamp.clone(),
            })
            .collect(),
    }
}
