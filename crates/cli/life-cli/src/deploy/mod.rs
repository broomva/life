//! Deploy orchestrator — provisions agent stacks on cloud targets.

mod backend;
mod railway;
mod state;

use std::collections::HashMap;
use std::time::Duration;

use anyhow::{Context, Result};
use tracing::info;

use crate::cli::{DeployArgs, DestroyArgs, ListArgs};
use crate::template::load_template;

pub use backend::DeployBackend;
pub use state::DeploymentState;

/// Execute a full agent deployment.
pub async fn run(args: DeployArgs) -> Result<()> {
    let template = load_template(&args.agent, args.template_path.as_deref())?;

    info!(
        agent = %args.agent,
        target = %args.target,
        services = template.services.len(),
        "deploying agent"
    );

    let project_name = args
        .project_name
        .unwrap_or_else(|| format!("life-{}", args.agent));

    // Parse extra env vars from --env KEY=VALUE
    let mut extra_env: HashMap<String, String> = HashMap::new();
    for kv in &args.env {
        if let Some((k, v)) = kv.split_once('=') {
            extra_env.insert(k.to_string(), v.to_string());
        }
    }

    // Inject provider override into arcan env
    if let Some(arcan) = template.services.get("arcan") {
        let _ = arcan; // template is immutable; we inject at deploy time
        extra_env
            .entry("ARCAN_PROVIDER".to_string())
            .or_insert_with(|| args.provider.clone());
    }

    let backend = create_backend(&args.target)?;

    println!(
        "Deploying {agent} to {target}...",
        agent = args.agent,
        target = args.target
    );
    println!(
        "  Template: {name} — {desc}",
        name = template.meta.name,
        desc = template.meta.description
    );
    println!(
        "  Services: {svcs}",
        svcs = template
            .services
            .keys()
            .cloned()
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!();

    // ── Provision ────────────────────────────────────────────────────────────
    let result = backend.deploy(&project_name, &template, &extra_env).await?;

    // Save deployment state for future status/destroy commands
    let state = DeploymentState {
        agent_name: args.agent.clone(),
        project_name: project_name.clone(),
        target: args.target.clone(),
        project_id: result.project_id.clone(),
        environment_id: result.environment_id.clone(),
        services: result.services.clone(),
        deployed_at: chrono::Utc::now(),
        template_name: template.meta.name.clone(),
    };
    state.save()?;

    println!("Deployment initiated:");
    for (name, svc) in &result.services {
        let url = svc.url.as_deref().unwrap_or("(internal)");
        println!("  {name}: {url} (service_id: {id})", id = svc.service_id);
    }
    println!();

    // ── Health check polling ─────────────────────────────────────────────────
    if !args.no_wait {
        println!("Waiting for services to become healthy...");
        let timeout = Duration::from_secs(300);
        let poll_interval = Duration::from_secs(10);
        let start = std::time::Instant::now();

        loop {
            if start.elapsed() > timeout {
                eprintln!("Timeout: not all services became healthy within 5 minutes.");
                eprintln!(
                    "Run `life status --agent {agent}` to check progress.",
                    agent = args.agent
                );
                break;
            }

            let status = backend.status(&result.project_id).await;
            match status {
                Ok(statuses) => {
                    let all_healthy = statuses.iter().all(|(_, s)| {
                        matches!(
                            s.status.as_str(),
                            "SUCCESS" | "HEALTHY" | "RUNNING" | "ACTIVE"
                        )
                    });

                    if all_healthy && !statuses.is_empty() {
                        println!();
                        println!("All services healthy!");
                        for (name, s) in &statuses {
                            println!("  {name}: {} {}", s.status, s.url.as_deref().unwrap_or(""));
                        }
                        break;
                    }

                    print!(".");
                }
                Err(_) => {
                    print!("?");
                }
            }

            tokio::time::sleep(poll_interval).await;
        }
    }

    println!();
    println!("Agent deployed: {project_name}");
    println!("  life status --agent {agent}", agent = args.agent);
    println!("  life cost   --agent {agent}", agent = args.agent);
    println!("  life destroy --agent {agent}", agent = args.agent);

    Ok(())
}

/// Tear down a deployed agent.
pub async fn destroy(args: DestroyArgs) -> Result<()> {
    let state = DeploymentState::load(&args.agent)
        .with_context(|| format!("no deployment found for agent '{}'", args.agent))?;

    if !args.yes {
        println!(
            "This will permanently destroy agent '{agent}' (project: {project}).",
            agent = args.agent,
            project = state.project_name,
        );
        println!("  Target: {}", state.target);
        println!(
            "  Services: {}",
            state
                .services
                .keys()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        );
        println!();
        println!("Type 'yes' to confirm:");

        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        if input.trim() != "yes" {
            println!("Aborted.");
            return Ok(());
        }
    }

    let backend = create_backend(&state.target)?;

    println!("Destroying agent '{}'...", args.agent);
    backend.destroy(&state.project_id).await?;

    // Remove local state file
    state.remove()?;

    println!("Agent '{}' destroyed.", args.agent);
    Ok(())
}

/// List all deployed agents.
pub async fn list(args: ListArgs) -> Result<()> {
    let states = DeploymentState::list_all()?;

    if states.is_empty() {
        println!("No deployed agents found.");
        println!("Deploy one with: life deploy --agent coding-agent --target railway");
        return Ok(());
    }

    match &args.format[..] {
        "json" => {
            let output: Vec<serde_json::Value> = states
                .iter()
                .map(|s| {
                    serde_json::json!({
                        "agent": s.agent_name,
                        "project": s.project_name,
                        "target": s.target,
                        "template": s.template_name,
                        "services": s.services.len(),
                        "deployed_at": s.deployed_at.to_rfc3339(),
                    })
                })
                .collect();
            println!("{}", serde_json::to_string_pretty(&output)?);
        }
        _ => {
            println!(
                "{:<20} {:<25} {:<10} {:<18} {:<6} Deployed",
                "Agent", "Project", "Target", "Template", "Svcs"
            );
            println!("{}", "─".repeat(95));

            for s in &states {
                println!(
                    "{:<20} {:<25} {:<10} {:<18} {:<6} {}",
                    s.agent_name,
                    s.project_name,
                    s.target,
                    s.template_name,
                    s.services.len(),
                    s.deployed_at.format("%Y-%m-%d %H:%M"),
                );
            }
        }
    }

    Ok(())
}

/// Create the appropriate backend based on the target name.
pub fn create_backend(target: &str) -> Result<Box<dyn DeployBackend>> {
    match target {
        "railway" => {
            let token = std::env::var("RAILWAY_API_TOKEN").context(
                "RAILWAY_API_TOKEN environment variable is required for Railway deploys",
            )?;
            Ok(Box::new(railway::RailwayBackend::new(token)))
        }
        "flyio" | "fly" => {
            anyhow::bail!(
                "Fly.io backend is planned but not yet implemented. Use --target railway."
            );
        }
        "ecs" | "aws" => {
            anyhow::bail!(
                "AWS ECS backend is planned but not yet implemented. Use --target railway."
            );
        }
        other => {
            anyhow::bail!(
                "Unknown deploy target: '{other}'. Supported: railway, flyio (planned), ecs (planned)."
            );
        }
    }
}
