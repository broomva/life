//! JSON-RPC 2.0 handler for A2A protocol methods.
//!
//! Dispatches incoming JSON-RPC requests to the appropriate handler
//! and returns JSON-RPC responses.

use crate::bridge::SpacesBridge;
use crate::types::*;
use std::sync::Arc;

/// Process a JSON-RPC request and return a response.
pub async fn handle_jsonrpc(bridge: Arc<SpacesBridge>, request: JsonRpcRequest) -> JsonRpcResponse {
    if request.jsonrpc != "2.0" {
        return JsonRpcResponse::error(
            request.id,
            error_codes::INVALID_REQUEST,
            "Invalid JSON-RPC version".to_string(),
        );
    }

    match request.method.as_str() {
        "message/send" => handle_message_send(bridge, request).await,
        "tasks/get" => handle_task_get(bridge, request).await,
        "tasks/cancel" => handle_task_cancel(bridge, request).await,
        _ => JsonRpcResponse::error(
            request.id,
            error_codes::METHOD_NOT_FOUND,
            format!("Method '{}' not found", request.method),
        ),
    }
}

async fn handle_message_send(
    bridge: Arc<SpacesBridge>,
    request: JsonRpcRequest,
) -> JsonRpcResponse {
    let params: MessageSendParams = match serde_json::from_value(request.params.clone()) {
        Ok(p) => p,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                error_codes::INVALID_PARAMS,
                format!("Invalid params: {}", e),
            );
        }
    };

    // Resolve agent
    let agent_id = match &params.agent_id {
        Some(id) => id.clone(),
        None => {
            return JsonRpcResponse::error(
                request.id,
                error_codes::INVALID_PARAMS,
                "agent_id is required".to_string(),
            );
        }
    };

    // Check if this is a follow-up message to an existing task
    if let Some(task_id) = &params.task_id {
        match bridge.send_task_message(task_id, &params.message).await {
            Ok(task) => {
                return JsonRpcResponse::success(
                    request.id,
                    serde_json::to_value(&task).unwrap_or_default(),
                );
            }
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    error_codes::TASK_STATE_ERROR,
                    e.to_string(),
                );
            }
        }
    }

    // Create a new task
    let context_id = params
        .context_id
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    match bridge
        .create_task(&agent_id, &context_id, &params.message)
        .await
    {
        Ok(task) => {
            JsonRpcResponse::success(request.id, serde_json::to_value(&task).unwrap_or_default())
        }
        Err(e) => JsonRpcResponse::error(request.id, error_codes::INTERNAL_ERROR, e.to_string()),
    }
}

async fn handle_task_get(bridge: Arc<SpacesBridge>, request: JsonRpcRequest) -> JsonRpcResponse {
    let params: TaskGetParams = match serde_json::from_value(request.params.clone()) {
        Ok(p) => p,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                error_codes::INVALID_PARAMS,
                format!("Invalid params: {}", e),
            );
        }
    };

    match bridge.get_task(&params.id, params.include_history).await {
        Ok(Some(task)) => {
            JsonRpcResponse::success(request.id, serde_json::to_value(&task).unwrap_or_default())
        }
        Ok(None) => JsonRpcResponse::error(
            request.id,
            error_codes::TASK_NOT_FOUND,
            format!("Task '{}' not found", params.id),
        ),
        Err(e) => JsonRpcResponse::error(request.id, error_codes::INTERNAL_ERROR, e.to_string()),
    }
}

async fn handle_task_cancel(bridge: Arc<SpacesBridge>, request: JsonRpcRequest) -> JsonRpcResponse {
    let params: TaskCancelParams = match serde_json::from_value(request.params.clone()) {
        Ok(p) => p,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                error_codes::INVALID_PARAMS,
                format!("Invalid params: {}", e),
            );
        }
    };

    match bridge.cancel_task(&params.id).await {
        Ok(task) => {
            JsonRpcResponse::success(request.id, serde_json::to_value(&task).unwrap_or_default())
        }
        Err(e) => JsonRpcResponse::error(request.id, error_codes::TASK_STATE_ERROR, e.to_string()),
    }
}
