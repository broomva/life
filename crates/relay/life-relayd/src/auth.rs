//! Device authorization flow (RFC 8628) for authenticating with broomva.tech.
//!
//! Usage: `relayd auth [--url https://broomva.tech]`
//!
//! Flow:
//!   1. POST /api/auth/device/code → get device_code, user_code, verification_uri
//!   2. Print verification URL and open browser
//!   3. Poll POST /api/auth/device/token every `interval` seconds
//!   4. On approval, store access_token + refresh_token in credentials.json

use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use tracing::info;

// ─── Wire types ──────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct DeviceCodeResponse {
    device_code: String,
    user_code: String,
    verification_uri_complete: String,
    expires_in: u64,
    interval: u64,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: Option<String>,
    refresh_token: Option<String>,
    // RFC 8628 error fields
    error: Option<String>,
    error_description: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct Credentials {
    token: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    refresh_token: Option<String>,
}

// ─── Public entry point ───────────────────────────────────────────────────────

/// Run the device authorization flow against `server_url`.
/// On success, writes credentials to `credentials_path`.
pub async fn run(server_url: &str, credentials_path: &Path) -> Result<()> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;

    // 1. Request a device code
    let code_resp: DeviceCodeResponse = client
        .post(format!("{server_url}/api/auth/device/code"))
        .json(&serde_json::json!({ "client_id": "broomva-cli", "scope": "" }))
        .send()
        .await
        .context("failed to request device code")?
        .error_for_status()
        .context("device code request rejected")?
        .json()
        .await
        .context("invalid device code response")?;

    // 2. Show the user what to do
    println!();
    println!("  Open the following URL to authenticate:");
    println!();
    println!("    {}", code_resp.verification_uri_complete);
    println!();
    println!("  User code: {}", code_resp.user_code);
    println!();

    // Try to open the browser on macOS/Linux
    let _ = std::process::Command::new("open")
        .arg(&code_resp.verification_uri_complete)
        .status();

    // 3. Poll for the token
    let poll_interval = Duration::from_secs(code_resp.interval.max(5));
    let deadline = Instant::now() + Duration::from_secs(code_resp.expires_in);

    info!(
        "polling for authorization (timeout: {}s)",
        code_resp.expires_in
    );

    loop {
        if Instant::now() >= deadline {
            bail!("device code expired — run `relayd auth` again");
        }

        tokio::time::sleep(poll_interval).await;

        let resp: TokenResponse = client
            .post(format!("{server_url}/api/auth/device/token"))
            .json(&serde_json::json!({
                "device_code": code_resp.device_code,
                "grant_type": "urn:ietf:params:oauth:grant-type:device_code",
            }))
            .send()
            .await
            .context("token poll failed")?
            .json()
            .await
            .context("invalid token poll response")?;

        match resp.error.as_deref() {
            None => {
                // Success
                let token = resp
                    .access_token
                    .context("server returned no access_token")?;

                let creds = Credentials {
                    token,
                    refresh_token: resp.refresh_token,
                };

                if let Some(parent) = credentials_path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(credentials_path, serde_json::to_string_pretty(&creds)?)?;

                println!("  Authenticated successfully!");
                println!("  Credentials saved to: {}", credentials_path.display());
                println!();
                info!(path = %credentials_path.display(), "credentials saved");
                return Ok(());
            }
            Some("authorization_pending") | Some("slow_down") => {
                // Keep polling
                continue;
            }
            Some("access_denied") => {
                bail!("authorization denied by user");
            }
            Some("expired_token") => {
                bail!("device code expired — run `relayd auth` again");
            }
            Some(code) => {
                bail!(
                    "authorization failed: {} — {}",
                    code,
                    resp.error_description.as_deref().unwrap_or("")
                );
            }
        }
    }
}
