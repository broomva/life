//! Deploy backend trait — abstracts over Railway, Fly.io, AWS ECS.

use std::collections::HashMap;

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::template::AgentTemplate;

/// A deployed service instance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeployedService {
    /// Cloud-specific service identifier.
    pub service_id: String,
    /// Public URL if the service is exposed.
    pub url: Option<String>,
    /// Deployment status (BUILDING, DEPLOYING, SUCCESS, FAILED, etc.).
    pub status: String,
}

/// Result of a full deployment operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeploymentResult {
    /// Cloud-specific project identifier.
    pub project_id: String,
    /// Cloud-specific environment identifier.
    pub environment_id: String,
    /// Deployed services by name.
    pub services: HashMap<String, DeployedService>,
}

/// Trait for cloud deployment backends.
///
/// Implementations handle the specifics of provisioning, monitoring, and
/// tearing down agent stacks on different cloud providers.
#[async_trait]
pub trait DeployBackend: Send + Sync {
    /// Deploy an agent template to the cloud target.
    async fn deploy(
        &self,
        project_name: &str,
        template: &AgentTemplate,
        extra_env: &HashMap<String, String>,
    ) -> Result<DeploymentResult>;

    /// Get the deployment status for all services in a project.
    async fn status(
        &self,
        project_id: &str,
    ) -> Result<HashMap<String, DeployedService>>;

    /// Destroy a project and all its services.
    async fn destroy(&self, project_id: &str) -> Result<()>;

    /// Restart all services in a project.
    #[allow(dead_code)]
    async fn restart(&self, project_id: &str) -> Result<()>;

    /// Scale a specific service to the given number of replicas.
    #[allow(dead_code)]
    async fn scale(
        &self,
        project_id: &str,
        service_name: &str,
        replicas: u32,
    ) -> Result<()>;
}
