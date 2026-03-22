//! Transport configuration and startup for the Praxis MCP server.
//!
//! Supports two transport modes:
//! - **stdio** — for CLI integration (stdin/stdout JSON-RPC)
//! - **Streamable HTTP** — for network access (HTTP POST + SSE)

use crate::server::PraxisMcpServer;
use rmcp::ServiceExt;
use rmcp::transport::io::stdio;
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use std::net::SocketAddr;
use std::sync::Arc;
use thiserror::Error;
use tokio_util::sync::CancellationToken;
use tracing::info;

/// Errors from MCP transport operations.
#[derive(Debug, Error)]
pub enum TransportError {
    #[error("MCP stdio transport failed: {0}")]
    Stdio(String),
    #[error("MCP HTTP transport failed: {0}")]
    Http(String),
    #[error("Server error: {0}")]
    Service(String),
}

/// Configuration for the Streamable HTTP transport.
#[derive(Debug, Clone)]
pub struct HttpTransportConfig {
    /// Address to bind (default: 127.0.0.1:3100).
    pub bind_addr: SocketAddr,
    /// MCP endpoint path (default: "/mcp").
    pub path: String,
    /// Whether to enable stateful sessions (default: true).
    pub stateful: bool,
    /// Cancellation token for graceful shutdown.
    pub cancel: CancellationToken,
}

impl Default for HttpTransportConfig {
    fn default() -> Self {
        Self {
            bind_addr: SocketAddr::from(([127, 0, 0, 1], 3100)),
            path: "/mcp".to_string(),
            stateful: true,
            cancel: CancellationToken::new(),
        }
    }
}

/// Run the MCP server over stdio transport.
///
/// Blocks until the client disconnects or the process is interrupted.
/// This is the standard way to integrate with MCP clients like Claude Desktop.
pub async fn serve_stdio(server: PraxisMcpServer) -> Result<(), TransportError> {
    info!(
        name = %server.get_info().server_info.name,
        "Starting MCP server on stdio"
    );

    let peer = server
        .serve(stdio())
        .await
        .map_err(|e| TransportError::Stdio(e.to_string()))?;

    peer.waiting()
        .await
        .map_err(|e| TransportError::Stdio(e.to_string()))?;

    Ok(())
}

/// Build an axum [`Router`](axum::Router) that serves MCP over Streamable HTTP.
///
/// This returns the router without starting a listener, so callers can compose
/// it with other routes or customize the server configuration.
///
/// The `factory` closure is called to create a fresh [`PraxisMcpServer`] for
/// each MCP session.
pub fn mcp_axum_router<F>(factory: F, config: HttpTransportConfig) -> axum::Router
where
    F: Fn() -> Result<PraxisMcpServer, std::io::Error> + Send + Sync + 'static,
{
    let session_manager = Arc::new(LocalSessionManager::default());

    let mcp_config = StreamableHttpServerConfig {
        stateful_mode: config.stateful,
        cancellation_token: config.cancel,
        ..Default::default()
    };

    let service = StreamableHttpService::new(factory, session_manager, mcp_config);

    axum::Router::new().route(&config.path, axum::routing::any_service(service))
}

/// Run the MCP server over Streamable HTTP transport.
///
/// Binds to the configured address and serves until cancelled.
pub async fn serve_http<F>(factory: F, config: HttpTransportConfig) -> Result<(), TransportError>
where
    F: Fn() -> Result<PraxisMcpServer, std::io::Error> + Send + Sync + 'static,
{
    let bind_addr = config.bind_addr;
    let router = mcp_axum_router(factory, config);

    info!(%bind_addr, "Starting MCP server on HTTP");

    let listener = tokio::net::TcpListener::bind(bind_addr)
        .await
        .map_err(|e| TransportError::Http(e.to_string()))?;

    axum::serve(listener, router)
        .await
        .map_err(|e| TransportError::Http(e.to_string()))?;

    Ok(())
}

/// Convenience: create a server info summary for `get_info()` without constructing a full server.
///
/// Useful for trait implementations that need `ServerHandler::get_info()`.
impl PraxisMcpServer {
    /// Returns the server info struct.
    pub fn get_info(&self) -> rmcp::model::ServerInfo {
        <Self as rmcp::handler::server::ServerHandler>::get_info(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aios_protocol::tool::ToolRegistry;

    #[test]
    fn http_config_default() {
        let config = HttpTransportConfig::default();
        assert_eq!(config.bind_addr.port(), 3100);
        assert_eq!(config.path, "/mcp");
        assert!(config.stateful);
    }

    #[test]
    fn mcp_router_builds() {
        let config = HttpTransportConfig::default();
        let _router = mcp_axum_router(|| Ok(PraxisMcpServer::new(ToolRegistry::default())), config);
        // Router construction should not panic
    }
}
