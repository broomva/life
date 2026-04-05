//! Auto-scaling based on Autonomic economic modes.
//!
//! Queries the Autonomic service for current economic state and maps
//! economic modes (Sovereign, Conserving, Hustle, Hibernate) to replica
//! counts using the template's scaling configuration.

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::cli::ScaleArgs;
use crate::deploy::DeploymentState;
use crate::template::load_template;

/// Economic state from the Autonomic gating endpoint.
#[derive(Debug, Deserialize)]
struct GatingProfile {
    economic_mode: String,
    #[serde(default)]
    balance_micro_credits: Option<i64>,
    #[serde(default)]
    monthly_burn_estimate: Option<i64>,
}

/// Determine the target replica count based on economic mode and scaling config.
fn replicas_for_mode(
    mode: &str,
    min_replicas: u32,
    max_replicas: u32,
    scale_down_mode: &str,
    scale_up_mode: &str,
) -> u32 {
    // Mode severity ordering (lowest to highest resource allocation):
    //   Hibernate → Hustle → Conserving → Sovereign
    let mode_rank = |m: &str| -> u32 {
        match m.to_lowercase().as_str() {
            "hibernate" => 0,
            "hustle" => 1,
            "conserving" => 2,
            "sovereign" => 3,
            _ => 2, // Default to conserving-level
        }
    };

    let current_rank = mode_rank(mode);
    let down_rank = mode_rank(scale_down_mode);
    let up_rank = mode_rank(scale_up_mode);

    if current_rank <= down_rank {
        // At or below scale-down threshold → minimum replicas
        min_replicas
    } else if current_rank >= up_rank {
        // At or above scale-up threshold → maximum replicas
        max_replicas
    } else {
        // In between → linear interpolation
        let range = max_replicas - min_replicas;
        let position = if up_rank > down_rank {
            (current_rank - down_rank) as f32 / (up_rank - down_rank) as f32
        } else {
            0.5
        };
        min_replicas + (range as f32 * position) as u32
    }
}

/// Fetch current economic mode from the Autonomic service.
async fn fetch_gating_profile(base_url: &str) -> Result<GatingProfile> {
    let url = format!("{base_url}/gating/default");
    let resp = reqwest::get(&url)
        .await
        .context("failed to reach Autonomic service")?;

    if !resp.status().is_success() {
        anyhow::bail!("Autonomic returned HTTP {}", resp.status());
    }

    resp.json()
        .await
        .context("failed to parse Autonomic gating profile")
}

pub async fn run(args: ScaleArgs) -> Result<()> {
    let state = DeploymentState::load(&args.agent)
        .with_context(|| format!("no deployment found for agent '{}'", args.agent))?;

    // Load the template to get scaling configuration
    let template = load_template(&state.template_name, None)
        .with_context(|| format!("failed to load template '{}'", state.template_name))?;
    let scaling = &template.scaling;

    // Verify the target service exists
    if !state.services.contains_key(&args.service) {
        let available: Vec<&str> = state.services.keys().map(String::as_str).collect();
        anyhow::bail!(
            "service '{}' not found. Available: {}",
            args.service,
            available.join(", ")
        );
    }

    let target_replicas = if args.auto {
        // ── Auto-scaling: query Autonomic for economic mode ──────────────
        let autonomic_url = state
            .services
            .get("autonomic")
            .and_then(|s| s.url.as_deref());

        let Some(autonomic_url) = autonomic_url else {
            anyhow::bail!(
                "auto-scaling requires an Autonomic service.\n\
                 This agent template ('{}') {} an Autonomic service.\n\
                 Use --replicas N for manual scaling instead.",
                state.template_name,
                if state.services.contains_key("autonomic") {
                    "has no public URL for"
                } else {
                    "does not include"
                }
            );
        };

        println!("Querying Autonomic at {autonomic_url}...");

        let profile = fetch_gating_profile(autonomic_url).await?;

        let target = replicas_for_mode(
            &profile.economic_mode,
            scaling.min_replicas,
            scaling.max_replicas,
            &scaling.scale_down_mode,
            &scaling.scale_up_mode,
        );

        println!("Economic Mode: {}", profile.economic_mode);
        if let Some(balance) = profile.balance_micro_credits {
            let credits = balance as f64 / 1_000_000.0;
            println!("Balance: {credits:.2} credits");
        }
        if let Some(burn) = profile.monthly_burn_estimate {
            let credits = burn as f64 / 1_000_000.0;
            println!("Monthly Burn: {credits:.2} credits");
        }
        println!(
            "Scaling Config: min={}, max={}, down_at={}, up_at={}",
            scaling.min_replicas,
            scaling.max_replicas,
            scaling.scale_down_mode,
            scaling.scale_up_mode,
        );
        println!();

        target
    } else if let Some(replicas) = args.replicas {
        // ── Manual scaling ───────────────────────────────────────────────
        if replicas < scaling.min_replicas || replicas > scaling.max_replicas {
            eprintln!(
                "Warning: requested {} replicas is outside template bounds ({}-{}).",
                replicas, scaling.min_replicas, scaling.max_replicas,
            );
        }
        replicas
    } else {
        anyhow::bail!("specify --replicas N or --auto for Autonomic-driven scaling.");
    };

    println!(
        "Scaling {service} to {target_replicas} replica(s)...",
        service = args.service
    );

    // Attempt to scale via the backend
    let backend = crate::deploy::create_backend(&state.target)?;

    match backend
        .scale(&state.project_id, &args.service, target_replicas)
        .await
    {
        Ok(()) => {
            println!(
                "Scaled {service} to {target_replicas} replica(s).",
                service = args.service
            );
        }
        Err(e) => {
            eprintln!("Backend scaling failed: {e}");
            eprintln!();
            eprintln!("Manual steps:");
            eprintln!(
                "  1. Open the Railway dashboard for project '{}'",
                state.project_name
            );
            eprintln!(
                "  2. Navigate to service '{}' → Settings → Scaling",
                args.service
            );
            eprintln!("  3. Set replicas to {target_replicas}");
            eprintln!();
            eprintln!(
                "Or use the Railway CLI: railway service --id {} scale --replicas {}",
                state
                    .services
                    .get(&args.service)
                    .map(|s| s.service_id.as_str())
                    .unwrap_or("???"),
                target_replicas,
            );
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_replicas_for_mode_sovereign_scales_up() {
        assert_eq!(
            replicas_for_mode("sovereign", 1, 5, "conserving", "sovereign"),
            5
        );
    }

    #[test]
    fn test_replicas_for_mode_hibernate_scales_down() {
        assert_eq!(
            replicas_for_mode("hibernate", 1, 5, "conserving", "sovereign"),
            1
        );
    }

    #[test]
    fn test_replicas_for_mode_conserving_at_threshold() {
        assert_eq!(
            replicas_for_mode("conserving", 1, 5, "conserving", "sovereign"),
            1
        );
    }

    #[test]
    fn test_replicas_for_mode_hustle_interpolates() {
        // hustle(1) is below conserving(2) = scale_down_mode → min replicas
        assert_eq!(
            replicas_for_mode("hustle", 2, 8, "conserving", "sovereign"),
            2
        );
    }

    #[test]
    fn test_replicas_for_mode_between_thresholds() {
        // conserving(2) between hustle(1) and sovereign(3)
        assert_eq!(
            replicas_for_mode("conserving", 1, 5, "hustle", "sovereign"),
            3 // 1 + (5-1) * (2-1)/(3-1) = 1 + 4*0.5 = 3
        );
    }
}
