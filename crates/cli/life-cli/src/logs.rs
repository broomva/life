//! Log streaming for deployed agents.
//!
//! Queries the Railway deployment logs API and streams them to stdout.
//! Falls back to querying service health endpoints if the API is unreachable.

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::cli::LogsArgs;
use crate::deploy::DeploymentState;

const RAILWAY_API_URL: &str = "https://backboard.railway.app/graphql/v2";

/// Fetch deployment logs from Railway via GraphQL.
async fn fetch_railway_logs(token: &str, deployment_id: &str, limit: u32) -> Result<Vec<LogEntry>> {
    let client = reqwest::Client::new();

    let query = r#"query ($deploymentId: String!, $limit: Int) {
        deploymentLogs(deploymentId: $deploymentId, limit: $limit) {
            timestamp
            message
            severity
        }
    }"#;

    let body = json!({
        "query": query,
        "variables": {
            "deploymentId": deployment_id,
            "limit": limit,
        },
    });

    let resp = client
        .post(RAILWAY_API_URL)
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {token}"))
        .json(&body)
        .send()
        .await
        .context("failed to reach Railway API")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        anyhow::bail!("Railway API returned HTTP {status}: {text}");
    }

    let json: Value = resp
        .json()
        .await
        .context("failed to parse Railway response")?;

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

    let logs = json
        .get("data")
        .and_then(|d| d.get("deploymentLogs"))
        .cloned()
        .unwrap_or(Value::Array(vec![]));

    let entries: Vec<LogEntry> = serde_json::from_value(logs).unwrap_or_default();
    Ok(entries)
}

/// Fetch the latest deployment ID for a service from Railway.
async fn fetch_latest_deployment_id(
    token: &str,
    project_id: &str,
    service_name: &str,
) -> Result<Option<String>> {
    let client = reqwest::Client::new();

    #[derive(Deserialize)]
    struct ProjectData {
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
        name: String,
        #[serde(rename = "serviceInstances")]
        service_instances: Edges<InstanceNode>,
    }
    #[derive(Deserialize)]
    struct InstanceNode {
        #[serde(rename = "latestDeployment")]
        latest_deployment: Option<DeploymentRef>,
    }
    #[derive(Deserialize)]
    struct DeploymentRef {
        id: String,
    }

    let query = r#"query ($projectId: String!) {
        project(id: $projectId) {
            services {
                edges {
                    node {
                        name
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
    }"#;

    let body = json!({
        "query": query,
        "variables": { "projectId": project_id },
    });

    let resp = client
        .post(RAILWAY_API_URL)
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {token}"))
        .json(&body)
        .send()
        .await
        .context("failed to reach Railway API")?;

    let json: Value = resp.json().await?;
    let data: ProjectData = serde_json::from_value(json.get("data").cloned().unwrap_or_default())?;

    for edge in data.project.services.edges {
        if edge.node.name == service_name {
            return Ok(edge
                .node
                .service_instances
                .edges
                .first()
                .and_then(|i| i.node.latest_deployment.as_ref())
                .map(|d| d.id.clone()));
        }
    }

    Ok(None)
}

#[derive(Debug, Deserialize)]
struct LogEntry {
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(default)]
    message: String,
    #[serde(default)]
    severity: Option<String>,
}

pub async fn run(args: LogsArgs) -> Result<()> {
    let state = DeploymentState::load(&args.agent)
        .with_context(|| format!("no deployment found for agent '{}'", args.agent))?;

    let token = std::env::var("RAILWAY_API_TOKEN")
        .context("RAILWAY_API_TOKEN required for log streaming")?;

    // Determine which services to fetch logs for
    let service_names: Vec<String> = if let Some(ref svc) = args.service {
        if !state.services.contains_key(svc) {
            let available: Vec<&str> = state.services.keys().map(String::as_str).collect();
            anyhow::bail!(
                "service '{svc}' not found. Available: {}",
                available.join(", ")
            );
        }
        vec![svc.clone()]
    } else {
        state.services.keys().cloned().collect()
    };

    println!(
        "Logs for agent '{}' (project: {})",
        state.agent_name, state.project_name
    );
    println!("═══════════════════════════════════════════");

    let mut found_any = false;

    for svc_name in &service_names {
        // Get the latest deployment ID for this service
        let deployment_id = fetch_latest_deployment_id(&token, &state.project_id, svc_name).await?;

        let Some(deployment_id) = deployment_id else {
            println!("\n[{svc_name}] No active deployment found.");
            continue;
        };

        match fetch_railway_logs(&token, &deployment_id, args.lines).await {
            Ok(entries) => {
                if entries.is_empty() {
                    println!("\n[{svc_name}] No log entries.");
                    continue;
                }

                found_any = true;
                println!("\n── {svc_name} ({} entries) ──", entries.len());

                for entry in &entries {
                    let ts = entry.timestamp.as_deref().unwrap_or("                   ");
                    let severity = entry.severity.as_deref().unwrap_or("INFO");
                    let msg = &entry.message;

                    // Truncate long timestamps to readable form
                    let ts_display = if ts.len() > 19 { &ts[..19] } else { ts };

                    println!("{ts_display} [{severity:>5}] {msg}");
                }
            }
            Err(e) => {
                eprintln!("\n[{svc_name}] Failed to fetch logs: {e}");
            }
        }
    }

    if !found_any && args.service.is_none() {
        println!();
        println!("No logs found for any service.");
        println!(
            "Services may still be deploying. Run `life status --agent {}` to check.",
            args.agent
        );
    }

    Ok(())
}
