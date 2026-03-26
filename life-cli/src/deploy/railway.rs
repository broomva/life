//! Railway deployment backend — provisions Life agent stacks via Railway GraphQL API.
//!
//! Port of the TypeScript client at broomva.tech/apps/chat/lib/railway.ts
//! to native Rust for the `life deploy` CLI.

use std::collections::HashMap;

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use tracing::{debug, info, warn};

use super::backend::{DeployBackend, DeployedService, DeploymentResult};
use crate::template::AgentTemplate;

const RAILWAY_API_URL: &str = "https://backboard.railway.app/graphql/v2";

pub struct RailwayBackend {
    token: String,
    client: reqwest::Client,
}

impl RailwayBackend {
    pub fn new(token: String) -> Self {
        Self {
            token,
            client: reqwest::Client::new(),
        }
    }

    /// Execute a GraphQL query/mutation against the Railway API.
    async fn graphql<T: for<'de> Deserialize<'de>>(
        &self,
        query: &str,
        variables: Value,
    ) -> Result<T> {
        let body = json!({
            "query": query,
            "variables": variables,
        });

        let resp = self
            .client
            .post(RAILWAY_API_URL)
            .header("Content-Type", "application/json")
            .header("Authorization", format!("Bearer {}", self.token))
            .json(&body)
            .send()
            .await
            .context("failed to reach Railway API")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("Railway API returned HTTP {status}: {text}");
        }

        let json: Value = resp.json().await.context("failed to parse Railway response")?;

        if let Some(errors) = json.get("errors") {
            if let Some(arr) = errors.as_array() {
                if !arr.is_empty() {
                    let messages: Vec<&str> = arr
                        .iter()
                        .filter_map(|e| e.get("message").and_then(Value::as_str))
                        .collect();
                    anyhow::bail!("Railway GraphQL error: {}", messages.join("; "));
                }
            }
        }

        let data = json
            .get("data")
            .context("Railway response missing 'data' field")?
            .clone();

        serde_json::from_value(data).context("failed to deserialize Railway response data")
    }
}

#[async_trait]
impl DeployBackend for RailwayBackend {
    async fn deploy(
        &self,
        project_name: &str,
        template: &AgentTemplate,
        extra_env: &HashMap<String, String>,
    ) -> Result<DeploymentResult> {
        // ── 1. Create Railway project ────────────────────────────────────────
        info!(project = project_name, "creating Railway project");

        #[derive(Deserialize)]
        struct ProjectCreate {
            #[serde(rename = "projectCreate")]
            project_create: IdNode,
        }
        #[derive(Deserialize)]
        struct IdNode {
            id: String,
        }

        let project: ProjectCreate = self
            .graphql(
                r#"mutation ($input: ProjectCreateInput!) {
                    projectCreate(input: $input) { id }
                }"#,
                json!({ "input": { "name": project_name } }),
            )
            .await
            .context("failed to create Railway project")?;

        let project_id = project.project_create.id;
        info!(project_id = %project_id, "project created");

        // ── 2. Get default environment ───────────────────────────────────────
        #[derive(Deserialize)]
        struct ProjectEnvs {
            project: ProjectEnvsInner,
        }
        #[derive(Deserialize)]
        struct ProjectEnvsInner {
            environments: Edges<EnvNode>,
        }
        #[derive(Deserialize)]
        struct Edges<T> {
            edges: Vec<Edge<T>>,
        }
        #[derive(Deserialize)]
        struct Edge<T> {
            node: T,
        }
        #[derive(Deserialize)]
        struct EnvNode {
            id: String,
            name: String,
        }

        let envs: ProjectEnvs = self
            .graphql(
                r#"query ($projectId: String!) {
                    project(id: $projectId) {
                        environments { edges { node { id name } } }
                    }
                }"#,
                json!({ "projectId": &project_id }),
            )
            .await
            .context("failed to fetch Railway environments")?;

        let env_id = envs
            .project
            .environments
            .edges
            .iter()
            .find(|e| e.node.name == "production")
            .or_else(|| envs.project.environments.edges.first())
            .map(|e| e.node.id.clone())
            .context("no environments found in Railway project")?;

        debug!(environment_id = %env_id, "using environment");

        // ── 3. Create services ───────────────────────────────────────────────
        let mut deployed_services = HashMap::new();

        for (svc_name, svc_def) in &template.services {
            info!(service = svc_name, image = %svc_def.image, "creating service");

            // 3a. Create service
            #[derive(Deserialize)]
            struct ServiceCreate {
                #[serde(rename = "serviceCreate")]
                service_create: IdNode,
            }

            let svc: ServiceCreate = self
                .graphql(
                    r#"mutation ($input: ServiceCreateInput!) {
                        serviceCreate(input: $input) { id }
                    }"#,
                    json!({
                        "input": {
                            "name": svc_name,
                            "projectId": &project_id,
                        }
                    }),
                )
                .await
                .with_context(|| format!("failed to create service '{svc_name}'"))?;

            let service_id = svc.service_create.id;

            // 3b. Set environment variables
            let mut vars: HashMap<String, String> = HashMap::new();
            vars.insert("PORT".to_string(), svc_def.port.to_string());

            // Shared env from template
            for (k, v) in &template.shared_env {
                vars.insert(k.clone(), v.clone());
            }

            // Service-specific env
            for (k, v) in &svc_def.env {
                vars.insert(k.clone(), v.clone());
            }

            // Extra env from CLI --env flags
            for (k, v) in extra_env {
                vars.insert(k.clone(), v.clone());
            }

            if let Err(e) = self
                .graphql::<Value>(
                    r#"mutation ($input: VariableCollectionUpsertInput!) {
                        variableCollectionUpsert(input: $input)
                    }"#,
                    json!({
                        "input": {
                            "projectId": &project_id,
                            "environmentId": &env_id,
                            "serviceId": &service_id,
                            "variables": vars,
                        }
                    }),
                )
                .await
            {
                warn!(service = svc_name, error = %e, "failed to set env vars (non-fatal)");
            }

            // 3c. Deploy from Docker image
            if let Err(e) = self
                .graphql::<Value>(
                    r#"mutation ($input: ServiceInstanceDeployInput!) {
                        serviceInstanceDeploy(input: $input) { id }
                    }"#,
                    json!({
                        "input": {
                            "serviceId": &service_id,
                            "environmentId": &env_id,
                            "source": { "image": &svc_def.image },
                        }
                    }),
                )
                .await
            {
                warn!(service = svc_name, error = %e, "failed to trigger deploy (non-fatal)");
            }

            // 3d. Create public domain if needed
            let mut url: Option<String> = None;
            if svc_def.public {
                match self
                    .graphql::<Value>(
                        r#"mutation ($input: ServiceInstanceDomainCreateInput!) {
                            serviceInstanceDomainCreate(input: $input) { domain }
                        }"#,
                        json!({
                            "input": {
                                "serviceId": &service_id,
                                "environmentId": &env_id,
                            }
                        }),
                    )
                    .await
                {
                    Ok(domain_data) => {
                        if let Some(domain) = domain_data
                            .get("serviceInstanceDomainCreate")
                            .and_then(|d| d.get("domain"))
                            .and_then(Value::as_str)
                        {
                            url = Some(format!("https://{domain}"));
                        }
                    }
                    Err(e) => {
                        warn!(service = svc_name, error = %e, "failed to create domain");
                        url = Some(format!(
                            "https://{svc_name}-{project_name}.up.railway.app"
                        ));
                    }
                }
            }

            deployed_services.insert(
                svc_name.clone(),
                DeployedService {
                    service_id,
                    url,
                    status: "DEPLOYING".to_string(),
                },
            );
        }

        Ok(DeploymentResult {
            project_id,
            environment_id: env_id,
            services: deployed_services,
        })
    }

    async fn status(
        &self,
        project_id: &str,
    ) -> Result<HashMap<String, DeployedService>> {
        #[derive(Deserialize)]
        struct ProjectStatus {
            project: ProjectServices,
        }
        #[derive(Deserialize)]
        struct ProjectServices {
            services: Edges<ServiceNode>,
        }
        #[derive(Deserialize)]
        struct Edges<T> {
            edges: Vec<Edge<T>>,
        }
        #[derive(Deserialize)]
        struct Edge<T> {
            node: T,
        }
        #[derive(Deserialize)]
        struct ServiceNode {
            id: String,
            name: String,
            #[serde(rename = "serviceInstances")]
            service_instances: Edges<InstanceNode>,
        }
        #[derive(Deserialize)]
        struct InstanceNode {
            domains: Option<DomainsNode>,
            #[serde(rename = "latestDeployment")]
            latest_deployment: Option<DeploymentNode>,
        }
        #[derive(Deserialize)]
        struct DomainsNode {
            #[serde(rename = "serviceDomains")]
            service_domains: Vec<DomainEntry>,
        }
        #[derive(Deserialize)]
        struct DomainEntry {
            domain: String,
        }
        #[derive(Deserialize)]
        struct DeploymentNode {
            status: String,
        }

        let data: ProjectStatus = self
            .graphql(
                r#"query ($projectId: String!) {
                    project(id: $projectId) {
                        services {
                            edges {
                                node {
                                    id
                                    name
                                    serviceInstances {
                                        edges {
                                            node {
                                                domains {
                                                    serviceDomains { domain }
                                                }
                                                latestDeployment { status }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }"#,
                json!({ "projectId": project_id }),
            )
            .await
            .context("failed to query Railway project status")?;

        let mut services = HashMap::new();

        for edge in data.project.services.edges {
            let svc = edge.node;
            let instance = svc.service_instances.edges.first();

            let status = instance
                .and_then(|i| i.node.latest_deployment.as_ref())
                .map(|d| d.status.clone())
                .unwrap_or_else(|| "UNKNOWN".to_string());

            let url = instance
                .and_then(|i| i.node.domains.as_ref())
                .and_then(|d| d.service_domains.first())
                .map(|d| format!("https://{}", d.domain));

            services.insert(
                svc.name.clone(),
                DeployedService {
                    service_id: svc.id,
                    url,
                    status,
                },
            );
        }

        Ok(services)
    }

    async fn destroy(&self, project_id: &str) -> Result<()> {
        info!(project_id = project_id, "destroying Railway project");

        self.graphql::<Value>(
            r#"mutation ($id: String!) {
                projectDelete(id: $id)
            }"#,
            json!({ "id": project_id }),
        )
        .await
        .context("failed to delete Railway project")?;

        info!("project destroyed");
        Ok(())
    }

    async fn restart(&self, project_id: &str) -> Result<()> {
        // Fetch all services and their latest deployment IDs
        #[derive(Deserialize)]
        struct ProjectDeploys {
            project: ProjectSvcs,
        }
        #[derive(Deserialize)]
        struct ProjectSvcs {
            services: Edges<SvcDeploy>,
        }
        #[derive(Deserialize)]
        struct Edges<T> {
            edges: Vec<Edge<T>>,
        }
        #[derive(Deserialize)]
        struct Edge<T> {
            node: T,
        }
        #[derive(Deserialize)]
        struct SvcDeploy {
            #[serde(rename = "serviceInstances")]
            service_instances: Edges<InstanceDeploy>,
        }
        #[derive(Deserialize)]
        struct InstanceDeploy {
            #[serde(rename = "latestDeployment")]
            latest_deployment: Option<DeployId>,
        }
        #[derive(Deserialize)]
        struct DeployId {
            id: String,
        }

        let data: ProjectDeploys = self
            .graphql(
                r#"query ($projectId: String!) {
                    project(id: $projectId) {
                        services {
                            edges {
                                node {
                                    serviceInstances {
                                        edges {
                                            node {
                                                latestDeployment { id }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }"#,
                json!({ "projectId": project_id }),
            )
            .await?;

        for edge in data.project.services.edges {
            if let Some(instance) = edge.node.service_instances.edges.first() {
                if let Some(deploy) = &instance.node.latest_deployment {
                    let _ = self
                        .graphql::<Value>(
                            r#"mutation ($id: String!) {
                                deploymentRestart(id: $id) { id }
                            }"#,
                            json!({ "id": &deploy.id }),
                        )
                        .await;
                }
            }
        }

        Ok(())
    }

    async fn scale(
        &self,
        _project_id: &str,
        _service_name: &str,
        _replicas: u32,
    ) -> Result<()> {
        // Railway handles scaling via their replica configuration.
        // This would need the serviceInstanceUpdate mutation with numReplicas.
        anyhow::bail!("Railway scaling via API requires Enterprise plan. Use Railway dashboard to configure replicas.")
    }
}
