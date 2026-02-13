use std::path::Path;

use lago_core::{Journal, Session, SessionConfig, SessionId};
use tracing::debug;

use crate::db::open_journal;

/// Create a new session.
pub async fn create(data_dir: &Path, name: &str) -> Result<(), Box<dyn std::error::Error>> {
    let journal = open_journal(data_dir)?;

    let session_id = SessionId::new();
    let session = Session {
        session_id: session_id.clone(),
        config: SessionConfig::new(name),
        created_at: lago_core::EventEnvelope::now_micros(),
        branches: vec![],
    };

    journal.put_session(session).await?;

    println!("{}", session_id);
    debug!(%session_id, name, "session created");
    Ok(())
}

/// List all sessions.
pub async fn list(data_dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let journal = open_journal(data_dir)?;
    let sessions = journal.list_sessions().await?;

    if sessions.is_empty() {
        println!("No sessions found.");
        return Ok(());
    }

    println!("{:<28}  {:<20}  CREATED AT", "SESSION ID", "NAME");
    println!("{}", "-".repeat(70));

    for session in &sessions {
        let created = format_timestamp(session.created_at);
        println!(
            "{:<28}  {:<20}  {}",
            session.session_id, session.config.name, created,
        );
    }

    println!("\n{} session(s) total.", sessions.len());
    Ok(())
}

/// Show details of a specific session.
pub async fn show(data_dir: &Path, id: &str) -> Result<(), Box<dyn std::error::Error>> {
    let journal = open_journal(data_dir)?;
    let session_id = SessionId::from_string(id);

    let session = journal
        .get_session(&session_id)
        .await?
        .ok_or_else(|| format!("session not found: {id}"))?;

    println!("Session ID:  {}", session.session_id);
    println!("Name:        {}", session.config.name);
    println!("Created At:  {}", format_timestamp(session.created_at));
    println!("Model:       {}", if session.config.model.is_empty() { "(default)" } else { &session.config.model });

    if !session.config.params.is_empty() {
        println!("Params:");
        for (k, v) in &session.config.params {
            println!("  {k}: {v}");
        }
    }

    if !session.branches.is_empty() {
        println!("Branches:");
        for branch in &session.branches {
            println!("  - {branch}");
        }
    }

    Ok(())
}

/// Format a microsecond timestamp to a human-readable string.
fn format_timestamp(micros: u64) -> String {
    let secs = micros / 1_000_000;
    let nanos = ((micros % 1_000_000) * 1000) as u32;
    let dt = std::time::UNIX_EPOCH + std::time::Duration::new(secs, nanos);
    let elapsed = dt
        .elapsed()
        .unwrap_or(std::time::Duration::ZERO);

    if elapsed.as_secs() < 60 {
        "just now".to_string()
    } else if elapsed.as_secs() < 3600 {
        format!("{}m ago", elapsed.as_secs() / 60)
    } else if elapsed.as_secs() < 86400 {
        format!("{}h ago", elapsed.as_secs() / 3600)
    } else {
        format!("{}d ago", elapsed.as_secs() / 86400)
    }
}
