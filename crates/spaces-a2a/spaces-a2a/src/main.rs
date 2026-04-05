//! spaces-a2a bridge server.
//!
//! Runs an HTTP + gRPC server that bridges A2A protocol to Life Spaces.
//!
//! HTTP endpoints:
//!   - GET  /.well-known/agent-card.json           — Directory of all agents
//!   - GET  /agents/{id}/.well-known/agent-card.json — Individual agent card
//!   - POST /                                       — JSON-RPC 2.0 endpoint
//!   - POST /agents/{id}                            — Agent-specific JSON-RPC
//!   - GET  /health                                 — Health check
//!   - GET  /agents                                 — List all agents (REST)
//!   - POST /agents                                 — Register agent (REST)
//!
//! gRPC:
//!   - A2AService on port 50051 (configurable)

use axum::{
    extract::{Json, Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Router,
};
use std::sync::Arc;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use spaces_a2a::{
    agent_card::{generate_agent_card, generate_directory, AgentCardConfig, ListingData},
    bridge::SpacesBridge,
    grpc::{pb::a2a_service_server::A2aServiceServer, A2AGrpcService},
    jsonrpc::handle_jsonrpc,
    types::JsonRpcRequest,
};

type AppState = Arc<SpacesBridge>;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "spaces_a2a=info,tower_http=info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let http_port: u16 = std::env::var("A2A_HTTP_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(3001);

    let grpc_port: u16 = std::env::var("A2A_GRPC_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(50051);

    let base_url =
        std::env::var("A2A_BASE_URL").unwrap_or_else(|_| format!("http://localhost:{}", http_port));

    // Create bridge with config
    let config = AgentCardConfig {
        base_url: base_url.clone(),
        a2a_version: "1.0".to_string(),
        signing_key: None,
    };
    let bridge = Arc::new(SpacesBridge::new(config));

    // Register sample agents for demo
    register_sample_agents(&bridge).await;

    // Build HTTP router
    let app = Router::new()
        // A2A well-known discovery
        .route("/.well-known/agent-card.json", get(well_known_directory))
        .route(
            "/agents/{agent_id}/.well-known/agent-card.json",
            get(well_known_agent_card),
        )
        // JSON-RPC endpoint (global)
        .route("/", post(jsonrpc_handler))
        // Agent-specific JSON-RPC endpoint
        .route("/agents/{agent_id}", post(agent_jsonrpc_handler))
        // REST endpoints
        .route("/agents", get(list_agents))
        .route("/agents", post(register_agent))
        .route("/health", get(health_check))
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(bridge.clone());

    // Start gRPC server in background
    let grpc_bridge = bridge.clone();
    tokio::spawn(async move {
        let addr = format!("0.0.0.0:{}", grpc_port).parse().unwrap();
        let service = A2AGrpcService::new(grpc_bridge);
        tracing::info!("gRPC server listening on {}", addr);
        if let Err(e) = tonic::transport::Server::builder()
            .add_service(A2aServiceServer::new(service))
            .serve(addr)
            .await
        {
            tracing::error!("gRPC server error: {}", e);
        }
    });

    // Start HTTP server
    let addr = format!("0.0.0.0:{}", http_port);
    tracing::info!("A2A bridge HTTP server listening on {}", addr);
    tracing::info!(
        "Agent Card directory: {}/.well-known/agent-card.json",
        base_url
    );
    tracing::info!("JSON-RPC endpoint: {}/", base_url);
    tracing::info!("gRPC endpoint: 0.0.0.0:{}", grpc_port);

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

// ---------------------------------------------------------------------------
// HTTP Handlers
// ---------------------------------------------------------------------------

/// GET /.well-known/agent-card.json — directory of all agents.
async fn well_known_directory(State(bridge): State<AppState>) -> impl IntoResponse {
    let listings = bridge.get_all_listings().await;
    let directory = generate_directory(&bridge.config, &listings);
    (
        StatusCode::OK,
        [("content-type", "application/json")],
        serde_json::to_string_pretty(&directory).unwrap_or_default(),
    )
}

/// GET /agents/{id}/.well-known/agent-card.json — individual agent card.
async fn well_known_agent_card(
    State(bridge): State<AppState>,
    Path(agent_id): Path<String>,
) -> impl IntoResponse {
    match bridge.get_listing(&agent_id).await {
        Some(listing) => {
            let card = generate_agent_card(&bridge.config, &listing);
            (
                StatusCode::OK,
                [("content-type", "application/json")],
                serde_json::to_string_pretty(&card).unwrap_or_default(),
            )
        }
        None => (
            StatusCode::NOT_FOUND,
            [("content-type", "application/json")],
            serde_json::json!({"error": "Agent not found"}).to_string(),
        ),
    }
}

/// POST / — JSON-RPC 2.0 handler.
async fn jsonrpc_handler(
    State(bridge): State<AppState>,
    Json(request): Json<JsonRpcRequest>,
) -> Json<spaces_a2a::types::JsonRpcResponse> {
    Json(handle_jsonrpc(bridge, request).await)
}

/// POST /agents/{id} — Agent-specific JSON-RPC handler.
async fn agent_jsonrpc_handler(
    State(bridge): State<AppState>,
    Path(agent_id): Path<String>,
    Json(mut request): Json<JsonRpcRequest>,
) -> Json<spaces_a2a::types::JsonRpcResponse> {
    // Inject agent_id into params if not present
    if let Some(params) = request.params.as_object_mut() {
        if !params.contains_key("agentId") {
            params.insert("agentId".to_string(), serde_json::Value::String(agent_id));
        }
    }
    Json(handle_jsonrpc(bridge, request).await)
}

/// GET /agents — List all registered agents (REST).
async fn list_agents(State(bridge): State<AppState>) -> impl IntoResponse {
    let listings = bridge.get_all_listings().await;
    let agents: Vec<serde_json::Value> = listings
        .iter()
        .map(|l| {
            serde_json::json!({
                "agentId": l.agent_id,
                "name": l.name,
                "description": l.description,
                "version": l.version,
                "url": format!("{}/agents/{}", bridge.config.base_url, l.agent_id),
            })
        })
        .collect();
    Json(serde_json::json!({"agents": agents}))
}

/// POST /agents — Register a new agent (REST).
async fn register_agent(
    State(bridge): State<AppState>,
    Json(listing): Json<ListingData>,
) -> impl IntoResponse {
    let agent_id = listing.agent_id.clone();
    bridge.register_listing(listing).await;
    (
        StatusCode::CREATED,
        Json(serde_json::json!({"status": "registered", "agentId": agent_id})),
    )
}

/// GET /health — Health check.
async fn health_check(State(bridge): State<AppState>) -> impl IntoResponse {
    let count = bridge.get_all_listings().await.len();
    Json(serde_json::json!({
        "status": "ok",
        "agents": count,
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

// ---------------------------------------------------------------------------
// Sample Data
// ---------------------------------------------------------------------------

async fn register_sample_agents(bridge: &SpacesBridge) {
    use spaces_a2a::agent_card::{AuthSchemeData, SkillData};

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
            description: "Event-sourced persistence — journal, blob store, knowledge index"
                .to_string(),
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
                examples: Some("Find docs about auth|Search for API patterns".to_string()),
                input_modes: None,
                output_modes: None,
            }],
            auth_schemes: vec![AuthSchemeData {
                scheme_type: "Bearer".to_string(),
                config: None,
            }],
        })
        .await;

    bridge
        .register_listing(ListingData {
            agent_id: "life-autonomic".to_string(),
            name: "Autonomic Controller".to_string(),
            description: "Homeostasis controller — operational, cognitive, and economic regulation for agents".to_string(),
            version: "0.1.0".to_string(),
            url: "http://localhost:8082".to_string(),
            provider_name: "BroomVA".to_string(),
            provider_url: Some("https://broomva.tech".to_string()),
            input_modes: "application/json".to_string(),
            output_modes: "application/json".to_string(),
            supports_streaming: false,
            supports_push_notifications: false,
            documentation_url: None,
            skills: vec![SkillData {
                skill_id: "health-assessment".to_string(),
                name: "Health Assessment".to_string(),
                description: "Evaluate agent operational health and recommend mode transitions".to_string(),
                tags: "health,monitoring,homeostasis".to_string(),
                examples: None,
                input_modes: None,
                output_modes: None,
            }],
            auth_schemes: vec![],
        })
        .await;

    tracing::info!("Registered 3 sample Life agents");
}
