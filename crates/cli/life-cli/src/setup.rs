//! `life setup` — interactive onboarding wizard for Life Agent OS.
//!
//! Displays a branded ASCII banner, system info, and walks the user through
//! provider configuration. Saves config to `~/.life/config.toml`.

use std::io::{self, BufRead, IsTerminal, Write};
use std::path::PathBuf;

use anyhow::{Context, Result};

// ── Constants ──────────────────────────────────────────────────────────────

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Whether stdout supports ANSI colors (not piped).
fn use_color() -> bool {
    io::stdout().is_terminal()
}

// ── ANSI helpers ───────────────────────────────────────────────────────────

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const BRIGHT_CYAN: &str = "\x1b[96m";
const CYAN: &str = "\x1b[36m";
const GREEN: &str = "\x1b[32m";
const BRIGHT_GREEN: &str = "\x1b[92m";
const RED: &str = "\x1b[31m";
const YELLOW: &str = "\x1b[33m";

fn c(code: &str, text: &str) -> String {
    if use_color() {
        format!("{code}{text}{RESET}")
    } else {
        text.to_string()
    }
}

fn bold(text: &str) -> String {
    c(BOLD, text)
}

fn dim(text: &str) -> String {
    c(DIM, text)
}

// ── Banner ─────────────────────────────────────────────────────────────────

const BANNER_LINES: [(&str, &str); 6] = [
    (BRIGHT_CYAN, "    ██╗     ██╗███████╗███████╗"),
    (CYAN, "    ██║     ██║██╔════╝██╔════╝"),
    (GREEN, "    ██║     ██║█████╗  █████╗  "),
    (BRIGHT_GREEN, "    ██║     ██║██╔══╝  ██╔══╝  "),
    (GREEN, "    ███████╗██║██║     ███████╗"),
    (CYAN, "    ╚══════╝╚═╝╚═╝     ╚══════╝"),
];

pub fn print_banner() {
    eprintln!();
    let colored = use_color();
    for (color, line) in &BANNER_LINES {
        if colored {
            eprintln!("{color}{line}{RESET}");
        } else {
            eprintln!("{line}");
        }
    }
    eprintln!();
    eprintln!("    {}", dim("Agent Operating System"));
    eprintln!("    {}", dim(&format!("v{VERSION}")));
    eprintln!();
}

// ── Quick help (when running `life` with no args) ──────────────────────────

pub fn print_quick_help() {
    print_banner();
    eprintln!(
        "  {}",
        dim("Run `life setup` to configure, or use a command below.")
    );
    eprintln!();
    eprintln!("  {}", bold("Commands"));
    eprintln!();
    eprintln!(
        "    {}        configure providers & keys",
        c(CYAN, "life setup")
    );
    eprintln!("    {}  deploy an agent to cloud", c(CYAN, "life deploy"));
    eprintln!("    {}  check deployed agents", c(CYAN, "life status"));
    eprintln!("    {}    list deployed agents", c(CYAN, "life list"));
    eprintln!("    {}     stream service logs", c(CYAN, "life logs"));
    eprintln!("    {}    scale agent services", c(CYAN, "life scale"));
    eprintln!("    {}     cost tracking", c(CYAN, "life cost"));
    eprintln!("    {}    manage relay daemon", c(CYAN, "life relay"));
    eprintln!();
    eprintln!("  {}", bold("Agent Runtime"));
    eprintln!();
    eprintln!("    {}       interactive TUI chat", c(GREEN, "arcan chat"));
    eprintln!("    {}      REPL mode", c(GREEN, "arcan shell"));
    eprintln!("    {}      start daemon", c(GREEN, "arcan serve"));
    eprintln!();
    eprintln!("  {}", dim("https://docs.broomva.tech/docs/life"));
    eprintln!();
}

// ── System info card ───────────────────────────────────────────────────────

fn print_system_info() {
    let platform = format!("{} {}", os_name(), std::env::consts::ARCH);
    let crate_count = 87;
    let tool_count = 26;
    let skill_count = 307;

    let w = 47; // inner width
    let top = format!("  ┌{}┐", "─".repeat(w));
    let bot = format!("  └{}┘", "─".repeat(w));

    eprintln!("{top}");
    info_row("version", VERSION, w);
    info_row("platform", &platform, w);
    info_row("crates", &format!("{crate_count}"), w);
    info_row("tools", &format!("{tool_count}"), w);
    info_row("skills", &format!("{skill_count}"), w);
    eprintln!("  │{}│", " ".repeat(w));
    eprintln!(
        "  │  {}{}│",
        bold("Modules"),
        " ".repeat(w - 2 - "Modules".len())
    );
    let modules = "arcan · lago · praxis · autonomic · haima";
    eprintln!(
        "  │  {}{}│",
        c(DIM, modules),
        " ".repeat(w - 2 - modules.len())
    );
    let modules2 = "nous · anima · vigil · spaces · opsis";
    eprintln!(
        "  │  {}{}│",
        c(DIM, modules2),
        " ".repeat(w - 2 - modules2.len())
    );
    eprintln!("{bot}");
    eprintln!();
}

fn info_row(label: &str, value: &str, width: usize) {
    let content = format!("  {:<12}{}", label, value);
    let pad = width.saturating_sub(content.len());
    eprintln!("  │{}{}│", content, " ".repeat(pad));
}

fn os_name() -> &'static str {
    if cfg!(target_os = "macos") {
        "macOS"
    } else if cfg!(target_os = "linux") {
        "Linux"
    } else if cfg!(target_os = "windows") {
        "Windows"
    } else {
        "Unknown"
    }
}

// ── Config persistence ─────────────────────────────────────────────────────

fn config_dir() -> PathBuf {
    dirs::home_dir()
        .expect("cannot determine home directory")
        .join(".life")
}

fn config_path() -> PathBuf {
    config_dir().join("config.toml")
}

fn config_exists() -> bool {
    config_path().is_file()
}

#[derive(serde::Serialize, serde::Deserialize, Default)]
struct LifeConfig {
    provider: ProviderConfig,
    #[serde(default)]
    consciousness: ConsciousnessConfig,
    #[serde(default)]
    arcan: ArcanConfig,
}

#[derive(serde::Serialize, serde::Deserialize, Default)]
struct ProviderConfig {
    name: String,
    model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    base_url: Option<String>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct ConsciousnessConfig {
    enabled: bool,
}

impl Default for ConsciousnessConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct ArcanConfig {
    port: u16,
}

impl Default for ArcanConfig {
    fn default() -> Self {
        Self { port: 3000 }
    }
}

// ── Prompting helpers ──────────────────────────────────────────────────────

fn prompt(message: &str) -> Result<String> {
    eprint!("{message}");
    io::stderr().flush()?;
    let mut buf = String::new();
    io::stdin()
        .lock()
        .read_line(&mut buf)
        .context("failed to read input")?;
    Ok(buf.trim().to_string())
}

fn prompt_with_default(message: &str, default: &str) -> Result<String> {
    let input = prompt(&format!("{message} {}: ", dim(&format!("[{default}]"))))?;
    if input.is_empty() {
        Ok(default.to_string())
    } else {
        Ok(input)
    }
}

fn prompt_secret(message: &str) -> Result<String> {
    eprint!("{message}");
    io::stderr().flush()?;

    // Disable echo via `stty` (no unsafe needed)
    #[cfg(unix)]
    let stty_off = std::process::Command::new("stty")
        .arg("-echo")
        .stdin(std::process::Stdio::inherit())
        .status()
        .is_ok();

    let mut buf = String::new();
    let result = io::stdin().lock().read_line(&mut buf);

    // Restore echo
    #[cfg(unix)]
    if stty_off {
        let _ = std::process::Command::new("stty")
            .arg("echo")
            .stdin(std::process::Stdio::inherit())
            .status();
    }

    eprintln!(); // newline after hidden input

    result.context("failed to read input")?;
    Ok(buf.trim().to_string())
}

// ── Provider selection ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
enum Provider {
    Anthropic,
    OpenAi,
    Vercel,
    Ollama,
    Mock,
}

impl Provider {
    fn name(&self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::OpenAi => "openai",
            Self::Vercel => "vercel",
            Self::Ollama => "ollama",
            Self::Mock => "mock",
        }
    }

    fn needs_api_key(&self) -> bool {
        matches!(self, Self::Anthropic | Self::OpenAi | Self::Vercel)
    }

    fn models(&self) -> &[(&str, &str)] {
        match self {
            Self::Anthropic => &[
                ("claude-sonnet-4-5-20250929", "recommended"),
                ("claude-haiku-4.5", "fast & cheap"),
                ("claude-opus-4-6", "most capable"),
            ],
            Self::OpenAi => &[
                ("gpt-4.1", "recommended"),
                ("gpt-4.1-mini", "fast & cheap"),
                ("gpt-4o", "multimodal"),
            ],
            Self::Vercel => &[
                ("anthropic/claude-sonnet-4-5", "recommended"),
                ("openai/gpt-4.1", "alternative"),
            ],
            Self::Ollama => &[("llama3.2", "recommended"), ("custom", "enter your own")],
            Self::Mock => &[("mock-provider", "testing")],
        }
    }

    fn key_hint(&self) -> &'static str {
        match self {
            Self::Anthropic => "sk-ant-...",
            Self::OpenAi => "sk-...",
            Self::Vercel => "gateway key",
            _ => "",
        }
    }
}

fn select_provider() -> Result<Provider> {
    eprintln!("  {}", bold("Choose your LLM provider:"));
    eprintln!();
    eprintln!(
        "    {}  Anthropic (Claude)     {}",
        c(BRIGHT_CYAN, "1"),
        dim("— recommended")
    );
    eprintln!("    {}  OpenAI (GPT)", c(BRIGHT_CYAN, "2"));
    eprintln!(
        "    {}  Vercel AI Gateway      {}",
        c(BRIGHT_CYAN, "3"),
        dim("— multi-provider routing")
    );
    eprintln!(
        "    {}  Ollama                 {}",
        c(BRIGHT_CYAN, "4"),
        dim("— local, no API key")
    );
    eprintln!(
        "    {}  Mock                   {}",
        c(BRIGHT_CYAN, "5"),
        dim("— testing, no API key")
    );
    eprintln!();

    loop {
        let input = prompt_with_default("  >", "1")?;
        match input.as_str() {
            "1" => return Ok(Provider::Anthropic),
            "2" => return Ok(Provider::OpenAi),
            "3" => return Ok(Provider::Vercel),
            "4" => return Ok(Provider::Ollama),
            "5" => return Ok(Provider::Mock),
            _ => {
                eprintln!("  {} Enter a number 1-5.", c(YELLOW, "!"));
            }
        }
    }
}

fn prompt_api_key(provider: &Provider) -> Result<Option<String>> {
    if !provider.needs_api_key() {
        return Ok(None);
    }

    eprintln!();
    let hint = provider.key_hint();
    loop {
        let key = prompt_secret(&format!(
            "  Enter your {} API key ({hint}): ",
            bold(provider.name())
        ))?;
        if key.is_empty() {
            eprintln!(
                "  {} API key is required for {}.",
                c(YELLOW, "!"),
                provider.name()
            );
            continue;
        }
        // Show masked preview
        let visible = if key.len() > 8 {
            format!("{}...{}", &key[..4], &key[key.len() - 4..])
        } else {
            "****".to_string()
        };
        eprintln!("  {} Key: {}", c(GREEN, "ok"), dim(&visible));
        return Ok(Some(key));
    }
}

fn prompt_base_url(provider: &Provider) -> Result<Option<String>> {
    match provider {
        Provider::Ollama => {
            eprintln!();
            let url = prompt_with_default("  Ollama base URL", "http://localhost:11434")?;
            Ok(Some(url))
        }
        Provider::Vercel => {
            eprintln!();
            let url = prompt_with_default("  Vercel AI Gateway URL", "https://gateway.vercel.ai")?;
            Ok(Some(url))
        }
        _ => Ok(None),
    }
}

fn select_model(provider: &Provider) -> Result<String> {
    let models = provider.models();

    // Single model — no need to prompt
    if models.len() == 1 {
        let m = models[0].0;
        eprintln!();
        eprintln!("  Model: {}", c(GREEN, m));
        return Ok(m.to_string());
    }

    eprintln!();
    eprintln!("  {}", bold("Choose a model:"));
    eprintln!();
    for (i, (name, desc)) in models.iter().enumerate() {
        let num = format!("{}", i + 1);
        let default_marker = if i == 0 { " (default)" } else { "" };
        eprintln!(
            "    {}  {:<36} {}{}",
            c(BRIGHT_CYAN, &num),
            name,
            dim(desc),
            dim(default_marker)
        );
    }
    eprintln!();

    loop {
        let input = prompt_with_default("  >", "1")?;

        // Allow typing the model name directly
        if models.iter().any(|(m, _)| *m == input) {
            return Ok(input);
        }

        if let Ok(n) = input.parse::<usize>() {
            if n >= 1 && n <= models.len() {
                let chosen = models[n - 1].0;
                // Handle "custom" for Ollama
                if chosen == "custom" {
                    eprintln!();
                    let custom = prompt("  Enter model name: ")?;
                    if custom.is_empty() {
                        eprintln!("  {} Model name cannot be empty.", c(YELLOW, "!"));
                        continue;
                    }
                    return Ok(custom);
                }
                return Ok(chosen.to_string());
            }
        }
        eprintln!("  {} Enter a number 1-{}.", c(YELLOW, "!"), models.len());
    }
}

// ── Save config ────────────────────────────────────────────────────────────

fn save_config(
    provider: &Provider,
    api_key: &Option<String>,
    model: &str,
    base_url: &Option<String>,
) -> Result<()> {
    let dir = config_dir();
    std::fs::create_dir_all(&dir).context("failed to create ~/.life directory")?;

    let cfg = LifeConfig {
        provider: ProviderConfig {
            name: provider.name().to_string(),
            model: model.to_string(),
            base_url: base_url.clone(),
        },
        consciousness: ConsciousnessConfig::default(),
        arcan: ArcanConfig::default(),
    };

    let content = toml::to_string_pretty(&cfg).context("failed to serialize config")?;
    let path = config_path();
    std::fs::write(&path, &content)
        .with_context(|| format!("failed to write {}", path.display()))?;

    eprintln!();
    eprintln!(
        "  {} Config saved to {}",
        c(GREEN, "ok"),
        dim(&path.display().to_string())
    );

    // Store the API key securely via credential cascade (keychain → .env fallback)
    if let Some(key) = api_key {
        let (env_var, kc_account) =
            life_paths::credentials::provider_credential_names(provider.name());
        let source = life_paths::credentials::store_credential(env_var, kc_account, key);
        eprintln!(
            "  {} API key stored in {}",
            c(GREEN, "ok"),
            dim(&source.to_string())
        );
    }

    Ok(())
}

// ── Connection test ────────────────────────────────────────────────────────

async fn test_connection(
    provider: &Provider,
    api_key: &Option<String>,
    model: &str,
    base_url: &Option<String>,
) -> Result<bool> {
    eprintln!();
    eprint!("  {} Testing connection...", c(CYAN, "◎"));
    io::stderr().flush()?;

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()?;

    let result = match provider {
        Provider::Anthropic => {
            let key = api_key.as_deref().unwrap_or("");
            client
                .post("https://api.anthropic.com/v1/messages")
                .header("x-api-key", key)
                .header("anthropic-version", "2023-06-01")
                .header("content-type", "application/json")
                .json(&serde_json::json!({
                    "model": model,
                    "max_tokens": 1,
                    "messages": [{"role": "user", "content": "ping"}]
                }))
                .send()
                .await
        }
        Provider::OpenAi => {
            let key = api_key.as_deref().unwrap_or("");
            client
                .post("https://api.openai.com/v1/chat/completions")
                .header("Authorization", format!("Bearer {key}"))
                .header("content-type", "application/json")
                .json(&serde_json::json!({
                    "model": model,
                    "max_tokens": 1,
                    "messages": [{"role": "user", "content": "ping"}]
                }))
                .send()
                .await
        }
        Provider::Vercel => {
            let url = base_url.as_deref().unwrap_or("https://gateway.vercel.ai");
            let key = api_key.as_deref().unwrap_or("");
            client
                .post(format!("{url}/v1/chat/completions"))
                .header("Authorization", format!("Bearer {key}"))
                .header("content-type", "application/json")
                .json(&serde_json::json!({
                    "model": model,
                    "max_tokens": 1,
                    "messages": [{"role": "user", "content": "ping"}]
                }))
                .send()
                .await
        }
        Provider::Ollama => {
            let url = base_url.as_deref().unwrap_or("http://localhost:11434");
            client.get(format!("{url}/api/tags")).send().await
        }
        Provider::Mock => {
            // Mock always succeeds
            eprint!("\r");
            eprintln!(
                "  {} Connected to {} ({})",
                c(GREEN, "✓"),
                bold("mock"),
                model
            );
            return Ok(true);
        }
    };

    match result {
        Ok(resp) if resp.status().is_success() || resp.status().as_u16() == 200 => {
            eprint!("\r");
            eprintln!(
                "  {} Connected to {} ({})",
                c(GREEN, "✓"),
                bold(provider.name()),
                model
            );
            Ok(true)
        }
        Ok(resp) => {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();

            // Try to extract error message from JSON
            let msg = serde_json::from_str::<serde_json::Value>(&body)
                .ok()
                .and_then(|v| {
                    v.get("error").and_then(|e| {
                        e.get("message")
                            .or(Some(e))
                            .and_then(|m| m.as_str().map(String::from))
                    })
                })
                .unwrap_or_else(|| format!("HTTP {status}"));

            eprint!("\r");
            eprintln!("  {} Connection failed: {}", c(RED, "✗"), msg);
            eprintln!("  {}", dim("Run `life setup` to reconfigure."));
            Ok(false)
        }
        Err(e) => {
            eprint!("\r");
            eprintln!("  {} Connection failed: {e}", c(RED, "✗"));
            eprintln!("  {}", dim("Run `life setup` to reconfigure."));
            Ok(false)
        }
    }
}

// ── Success screen ─────────────────────────────────────────────────────────

fn print_success(provider: &Provider, api_key: &Option<String>) {
    eprintln!();
    eprintln!("  {}", c(GREEN, "✓ Setup complete!"));
    eprintln!();
    eprintln!("  {}", bold("Quick start"));
    eprintln!();
    eprintln!("    {}        reconfigure", c(CYAN, "life setup"));
    eprintln!("    {}       interactive TUI chat", c(CYAN, "arcan chat"));
    eprintln!("    {}      REPL mode", c(CYAN, "arcan shell"));
    eprintln!("    {}      start daemon", c(CYAN, "arcan serve"));
    eprintln!("    {}  deploy to cloud", c(CYAN, "life deploy"));
    eprintln!("    {}  check deployments", c(CYAN, "life status"));
    eprintln!();

    // Show credential storage location (never print the raw key)
    if api_key.is_some() && provider.needs_api_key() {
        let (env_var, kc_account) =
            life_paths::credentials::provider_credential_names(provider.name());
        let storage_hint = if life_paths::keychain::is_available() {
            format!("keychain (account: {kc_account})")
        } else {
            "~/.life/credentials/.env".to_string()
        };
        eprintln!("  {}", bold("Credentials"));
        eprintln!();
        eprintln!("    {env_var} stored in {}", dim(&storage_hint),);
        eprintln!();
    }

    eprintln!("  Or run directly:");
    eprintln!();
    eprintln!("    {}", c(GREEN, "arcan chat"));
    eprintln!();
}

// ── Prerequisites check ───────────────────────────────────────────────────

/// Check whether Rust is installed and meets MSRV.
fn check_rust() -> (bool, String) {
    let output = std::process::Command::new("rustc")
        .arg("--version")
        .output();
    match output {
        Ok(out) if out.status.success() => {
            let version_str = String::from_utf8_lossy(&out.stdout).trim().to_string();
            // Extract version number (e.g., "rustc 1.93.0 (..." -> "1.93.0")
            let version = version_str.split_whitespace().nth(1).unwrap_or("unknown");
            let parts: Vec<u32> = version.split('.').filter_map(|p| p.parse().ok()).collect();
            let meets_msrv =
                parts.len() >= 2 && (parts[0] > 1 || (parts[0] == 1 && parts[1] >= 93));
            (meets_msrv, format!("rustc {version}"))
        }
        _ => (false, "not found".to_string()),
    }
}

/// Check whether protoc (protobuf compiler) is installed.
fn check_protoc() -> (bool, String) {
    let output = std::process::Command::new("protoc")
        .arg("--version")
        .output();
    match output {
        Ok(out) if out.status.success() => {
            let version_str = String::from_utf8_lossy(&out.stdout).trim().to_string();
            (true, version_str)
        }
        _ => (false, "not found".to_string()),
    }
}

/// Run prerequisites check and print results. Returns true if all pass.
fn check_prerequisites() -> bool {
    eprintln!("  {}", bold("Prerequisites"));
    eprintln!();

    let (rust_ok, rust_ver) = check_rust();
    if rust_ok {
        eprintln!("    {} Rust: {}", c(GREEN, "ok"), dim(&rust_ver));
    } else if rust_ver == "not found" {
        eprintln!(
            "    {} Rust: not found. Install from {}",
            c(RED, "!!"),
            c(CYAN, "https://rustup.rs/")
        );
    } else {
        eprintln!(
            "    {} Rust: {} (need 1.93+, run {})",
            c(YELLOW, "!!"),
            rust_ver,
            c(CYAN, "rustup update")
        );
    }

    let (protoc_ok, protoc_ver) = check_protoc();
    if protoc_ok {
        eprintln!("    {} protoc: {}", c(GREEN, "ok"), dim(&protoc_ver));
    } else {
        let install_hint = if cfg!(target_os = "macos") {
            "brew install protobuf"
        } else if cfg!(target_os = "linux") {
            "sudo apt install protobuf-compiler"
        } else {
            "choco install protoc"
        };
        eprintln!(
            "    {} protoc: not found (run {})",
            c(YELLOW, "!!"),
            c(CYAN, install_hint)
        );
    }

    eprintln!();
    rust_ok && protoc_ok
}

// ── Test shell session ────────────────────────────────────────────────────

fn launch_test_shell() {
    eprintln!();
    eprintln!("  {}", bold("Test session"));
    eprintln!();

    let arcan_path = which_arcan();
    if arcan_path.is_none() {
        eprintln!(
            "  {} arcan not found in PATH. Run {} to install.",
            c(YELLOW, "!"),
            c(CYAN, "cargo install arcan")
        );
        eprintln!();
        return;
    }

    let answer = prompt(&format!(
        "  Launch a test shell session? {}: ",
        dim("[Y/n]")
    ));
    match answer {
        Ok(a) if a.to_lowercase() == "n" || a.to_lowercase() == "no" => {
            eprintln!();
            eprintln!(
                "  Skipped. Run {} when ready.",
                c(CYAN, "arcan shell --provider mock")
            );
            eprintln!();
        }
        _ => {
            eprintln!();
            eprintln!("  Launching {} ...", c(CYAN, "arcan shell --provider mock"));
            eprintln!();
            let _ = std::process::Command::new(arcan_path.unwrap())
                .args(["shell", "--provider", "mock"])
                .status();
        }
    }
}

/// Find arcan binary in PATH.
fn which_arcan() -> Option<std::path::PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths).find_map(|dir| {
            let candidate = dir.join("arcan");
            if candidate.is_file() {
                Some(candidate)
            } else {
                None
            }
        })
    })
}

// ── Main entry point ───────────────────────────────────────────────────────

pub async fn run() -> Result<()> {
    print_banner();
    print_system_info();

    // Step 0: Prerequisites check
    let prereqs_ok = check_prerequisites();
    if !prereqs_ok {
        eprintln!(
            "  {} Some prerequisites are missing. Setup will continue, but builds may fail.",
            c(YELLOW, "!")
        );
        eprintln!();
    }

    // Check for existing config
    if config_exists() {
        let answer = prompt(&format!(
            "  Existing config found at {}. Reconfigure? {}: ",
            dim(&config_path().display().to_string()),
            dim("[y/N]")
        ))?;
        if !matches!(answer.to_lowercase().as_str(), "y" | "yes") {
            eprintln!();
            eprintln!("  {} Keeping existing configuration.", c(GREEN, "ok"));
            eprintln!("  Run {} to start.", c(CYAN, "arcan chat"));
            eprintln!();
            return Ok(());
        }
        eprintln!();
    }

    // Step 1: Provider
    let provider = select_provider()?;

    // Step 2: API key (if needed)
    let api_key = prompt_api_key(&provider)?;

    // Step 3: Base URL (Ollama / Vercel)
    let base_url = prompt_base_url(&provider)?;

    // Step 4: Model
    let model = select_model(&provider)?;

    // Step 5: Save
    save_config(&provider, &api_key, &model, &base_url)?;

    // Step 6: Test connection
    let _ok = test_connection(&provider, &api_key, &model, &base_url).await?;

    // Step 7: Success
    print_success(&provider, &api_key);

    // Step 8: Offer to launch a test shell session
    launch_test_shell();

    Ok(())
}
