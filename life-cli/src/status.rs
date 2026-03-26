//! Status checking for deployed agents.

use anyhow::{Context, Result};
use tabled::{Table, Tabled};

use crate::cli::StatusArgs;
use crate::deploy::DeploymentState;

#[derive(Tabled)]
struct ServiceRow {
    #[tabled(rename = "Service")]
    name: String,
    #[tabled(rename = "Status")]
    status: String,
    #[tabled(rename = "URL")]
    url: String,
    #[tabled(rename = "Service ID")]
    service_id: String,
}

pub async fn run(args: StatusArgs) -> Result<()> {
    let state = DeploymentState::load(&args.agent)
        .with_context(|| format!("no deployment found for agent '{}'", args.agent))?;

    // Create backend and query live status
    let backend = crate::deploy::create_backend(&state.target)?;
    let live_status = backend.status(&state.project_id).await;

    match &args.format[..] {
        "json" => {
            match &live_status {
                Ok(services) => {
                    let output = serde_json::json!({
                        "agent": state.agent_name,
                        "project_name": state.project_name,
                        "project_id": state.project_id,
                        "target": state.target,
                        "template": state.template_name,
                        "deployed_at": state.deployed_at.to_rfc3339(),
                        "services": services,
                    });
                    println!("{}", serde_json::to_string_pretty(&output)?);
                }
                Err(e) => {
                    // Fall back to saved state
                    let output = serde_json::json!({
                        "agent": state.agent_name,
                        "project_name": state.project_name,
                        "project_id": state.project_id,
                        "target": state.target,
                        "template": state.template_name,
                        "deployed_at": state.deployed_at.to_rfc3339(),
                        "services": state.services,
                        "warning": format!("live status unavailable: {e}"),
                    });
                    println!("{}", serde_json::to_string_pretty(&output)?);
                }
            }
        }
        _ => {
            println!("Agent: {}", state.agent_name);
            println!("Project: {} ({})", state.project_name, state.project_id);
            println!("Target: {}", state.target);
            println!("Template: {}", state.template_name);
            println!("Deployed: {}", state.deployed_at.format("%Y-%m-%d %H:%M:%S UTC"));
            println!();

            match live_status {
                Ok(services) => {
                    let rows: Vec<ServiceRow> = services
                        .iter()
                        .map(|(name, svc)| ServiceRow {
                            name: name.clone(),
                            status: svc.status.clone(),
                            url: svc.url.clone().unwrap_or_else(|| "(internal)".to_string()),
                            service_id: svc.service_id.clone(),
                        })
                        .collect();

                    println!("{}", Table::new(rows));
                }
                Err(e) => {
                    eprintln!("Warning: could not fetch live status: {e}");
                    eprintln!("Showing last known state:");
                    println!();

                    let rows: Vec<ServiceRow> = state
                        .services
                        .iter()
                        .map(|(name, svc)| ServiceRow {
                            name: name.clone(),
                            status: svc.status.clone(),
                            url: svc.url.clone().unwrap_or_else(|| "(internal)".to_string()),
                            service_id: svc.service_id.clone(),
                        })
                        .collect();

                    println!("{}", Table::new(rows));
                }
            }
        }
    }

    Ok(())
}
