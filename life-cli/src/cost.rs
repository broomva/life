//! Cost tracking dashboard for deployed agents.
//!
//! Queries Haima's finance API for per-agent compute + LLM costs,
//! and supplements with Autonomic economic state.

use anyhow::{Context, Result};
use serde::Deserialize;
use tabled::{Table, Tabled};

use crate::cli::CostArgs;
use crate::deploy::DeploymentState;

/// Cost data from Haima finance API.
#[derive(Debug, Deserialize)]
struct HaimaCostReport {
    /// Total cost in micro-credits for the window.
    total_micro_credits: i64,
    /// Per-service cost breakdown.
    #[serde(default)]
    services: Vec<ServiceCost>,
    /// Economic mode from Autonomic.
    economic_mode: Option<String>,
    /// Balance remaining.
    balance_micro_credits: Option<i64>,
    /// Estimated monthly burn.
    monthly_burn_estimate: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct ServiceCost {
    name: String,
    /// LLM token costs (micro-credits).
    llm_cost: i64,
    /// Compute time costs (micro-credits).
    compute_cost: i64,
    /// Total cost.
    total_cost: i64,
}

#[derive(Tabled)]
struct CostRow {
    #[tabled(rename = "Service")]
    name: String,
    #[tabled(rename = "LLM Cost")]
    llm_cost: String,
    #[tabled(rename = "Compute Cost")]
    compute_cost: String,
    #[tabled(rename = "Total")]
    total: String,
}

/// Format micro-credits as human-readable credits (1 credit = 1,000,000 μcr).
fn format_credits(micro_credits: i64) -> String {
    let credits = micro_credits as f64 / 1_000_000.0;
    if credits >= 1.0 {
        format!("{credits:.2} cr")
    } else {
        format!("{micro_credits} μcr")
    }
}

pub async fn run(args: CostArgs) -> Result<()> {
    let state = DeploymentState::load(&args.agent)
        .with_context(|| format!("no deployment found for agent '{}'", args.agent))?;

    // Try to reach Haima API for live cost data
    let haima_url = state
        .services
        .get("haima")
        .and_then(|s| s.url.as_deref());

    let report = if let Some(url) = haima_url {
        match fetch_cost_report(url, &args.window).await {
            Ok(r) => Some(r),
            Err(e) => {
                eprintln!("Warning: could not reach Haima API at {url}: {e}");
                None
            }
        }
    } else {
        None
    };

    // Also try to get Autonomic economic state
    let autonomic_url = state
        .services
        .get("autonomic")
        .and_then(|s| s.url.as_deref());

    let economic_mode = if let Some(url) = autonomic_url {
        fetch_economic_mode(url).await.ok()
    } else {
        report.as_ref().and_then(|r| r.economic_mode.clone())
    };

    match &args.format[..] {
        "json" => {
            let output = serde_json::json!({
                "agent": state.agent_name,
                "window": args.window,
                "economic_mode": economic_mode,
                "cost_report": report.as_ref().map(|r| serde_json::json!({
                    "total_credits": format_credits(r.total_micro_credits),
                    "total_micro_credits": r.total_micro_credits,
                    "balance_credits": r.balance_micro_credits.map(format_credits),
                    "monthly_burn_credits": r.monthly_burn_estimate.map(format_credits),
                    "services": r.services.iter().map(|s| serde_json::json!({
                        "name": s.name,
                        "llm_cost": format_credits(s.llm_cost),
                        "compute_cost": format_credits(s.compute_cost),
                        "total": format_credits(s.total_cost),
                    })).collect::<Vec<_>>(),
                })),
            });
            println!("{}", serde_json::to_string_pretty(&output)?);
        }
        _ => {
            println!("Cost Report: {} (window: {})", state.agent_name, args.window);
            println!("═══════════════════════════════════════════");

            if let Some(mode) = &economic_mode {
                println!("Economic Mode: {mode}");
            }

            if let Some(report) = &report {
                println!("Total Cost: {}", format_credits(report.total_micro_credits));

                if let Some(balance) = report.balance_micro_credits {
                    println!("Balance: {}", format_credits(balance));
                }
                if let Some(burn) = report.monthly_burn_estimate {
                    println!("Monthly Burn Est.: {}", format_credits(burn));
                }

                println!();

                if !report.services.is_empty() {
                    let rows: Vec<CostRow> = report
                        .services
                        .iter()
                        .map(|s| CostRow {
                            name: s.name.clone(),
                            llm_cost: format_credits(s.llm_cost),
                            compute_cost: format_credits(s.compute_cost),
                            total: format_credits(s.total_cost),
                        })
                        .collect();

                    println!("{}", Table::new(rows));
                }
            } else {
                println!();
                println!("No live cost data available.");
                println!("Ensure the Haima service is deployed and reachable.");
                println!("  Template: {}", state.template_name);
                let has_haima = state.services.contains_key("haima");
                if !has_haima {
                    println!("  Note: this agent template does not include Haima.");
                    println!("  Use 'coding-agent' or 'data-agent' template for cost tracking.");
                }
            }
        }
    }

    Ok(())
}

/// Fetch cost report from Haima finance API.
async fn fetch_cost_report(base_url: &str, window: &str) -> Result<HaimaCostReport> {
    let url = format!("{base_url}/v1/cost?window={window}");
    let resp = reqwest::get(&url).await.context("failed to reach Haima")?;

    if !resp.status().is_success() {
        anyhow::bail!("Haima returned HTTP {}", resp.status());
    }

    resp.json().await.context("failed to parse Haima cost report")
}

/// Fetch current economic mode from Autonomic.
async fn fetch_economic_mode(base_url: &str) -> Result<String> {
    #[derive(Deserialize)]
    struct GatingResponse {
        economic_mode: String,
    }

    let url = format!("{base_url}/gating/default");
    let resp = reqwest::get(&url).await.context("failed to reach Autonomic")?;

    if !resp.status().is_success() {
        anyhow::bail!("Autonomic returned HTTP {}", resp.status());
    }

    let data: GatingResponse = resp.json().await?;
    Ok(data.economic_mode)
}
