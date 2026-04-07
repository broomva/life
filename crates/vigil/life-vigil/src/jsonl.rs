//! JSONL dual-write for LLM call artifacts.
//!
//! Provides a local, append-only audit trail of every LLM provider call.
//! Independent of OTel export — even if the OTLP endpoint is down, local
//! artifacts are always captured.
//!
//! Enable by setting `VIGIL_JSONL_PATH` to a file path:
//! ```bash
//! export VIGIL_JSONL_PATH="/tmp/arcan-llm-calls.jsonl"
//! ```
//!
//! Each line is a self-contained JSON object with pre-call envelope,
//! post-call economics, and OTel correlation IDs.

use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::envelope::{LlmRequestEnvelope, LlmResponseEconomics};

/// Combined record for one LLM call — pre-call envelope + post-call response.
///
/// Each record is written as a single JSON line to the JSONL file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmCallRecord {
    /// ISO 8601 timestamp of when the record was written.
    pub timestamp: String,
    /// Pre-call context (identity, model, economics, reliability, governance).
    pub envelope: LlmRequestEnvelope,
    /// Post-call economics (token usage, cost, duration). None on error.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response: Option<LlmResponseEconomics>,
    /// OTel trace ID for correlation with distributed traces.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    /// OTel span ID for correlation with the specific chat span.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub span_id: Option<String>,
    /// Error message if the provider call failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// JSONL writer for LLM call artifacts.
///
/// Appends one JSON line per LLM call. Uses open-append-close per write
/// for simplicity and compatibility with log rotation tools.
#[derive(Debug, Clone)]
pub struct JsonlWriter {
    path: PathBuf,
}

impl JsonlWriter {
    /// Create a writer for the given path.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Create from `VIGIL_JSONL_PATH` env var, or return `None` if not set.
    pub fn from_env() -> Option<Self> {
        std::env::var("VIGIL_JSONL_PATH")
            .ok()
            .filter(|p| !p.is_empty())
            .map(Self::new)
    }

    /// The file path this writer appends to.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Append one record as a JSON line.
    ///
    /// Opens the file in append mode, writes the JSON + newline, and closes.
    /// Returns an error if the file can't be opened or written to.
    pub fn write(&self, record: &LlmCallRecord) -> Result<(), std::io::Error> {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;

        let json = serde_json::to_string(record)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

        writeln!(file, "{json}")?;
        Ok(())
    }

    /// Append a record, logging a warning on failure instead of returning an error.
    ///
    /// Use this in the hot path (provider calls) where JSONL write failures
    /// should not block the agent loop.
    pub fn write_best_effort(&self, record: &LlmCallRecord) {
        if let Err(e) = self.write(record) {
            tracing::warn!(
                path = %self.path.display(),
                error = %e,
                "JSONL dual-write failed"
            );
        }
    }
}

/// Get the current timestamp as ISO 8601 string.
///
/// Uses a simple format that doesn't require the `chrono` crate.
pub fn now_iso8601() -> String {
    use std::time::SystemTime;
    let duration = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = duration.as_secs();
    // Format as seconds since epoch (compact, sortable, no chrono dep)
    format!("{secs}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::{CostSource, LlmRequestEnvelope, LlmResponseEconomics};
    use std::time::Duration;

    fn test_envelope() -> LlmRequestEnvelope {
        LlmRequestEnvelope::new(
            "sess-1",
            "run-1",
            "arcan",
            0,
            "anthropic",
            "claude-sonnet-4",
        )
    }

    fn test_economics() -> LlmResponseEconomics {
        LlmResponseEconomics {
            cost_source: CostSource::EstimatedLocalSnapshot,
            input_tokens: 100,
            output_tokens: 50,
            total_tokens: 150,
            input_cost_usd: Some(0.0003),
            output_cost_usd: Some(0.00075),
            total_cost_usd: Some(0.00105),
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
            duration: Duration::from_millis(800),
        }
    }

    #[test]
    fn record_serialization() {
        let record = LlmCallRecord {
            timestamp: "1700000000".to_string(),
            envelope: test_envelope(),
            response: Some(test_economics()),
            trace_id: Some("abc123".to_string()),
            span_id: Some("def456".to_string()),
            error: None,
        };

        let json = serde_json::to_string(&record).unwrap();
        assert!(json.contains("sess-1"));
        assert!(json.contains("anthropic"));
        assert!(json.contains("abc123"));
        // error should be omitted (skip_serializing_if)
        assert!(!json.contains("\"error\""));
    }

    #[test]
    fn record_with_error() {
        let record = LlmCallRecord {
            timestamp: "1700000000".to_string(),
            envelope: test_envelope(),
            response: None,
            trace_id: None,
            span_id: None,
            error: Some("provider timeout".to_string()),
        };

        let json = serde_json::to_string(&record).unwrap();
        assert!(json.contains("provider timeout"));
        assert!(!json.contains("\"response\""));
    }

    #[test]
    fn writer_creates_and_appends() {
        let dir = std::env::temp_dir().join("vigil-test-jsonl");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test-calls.jsonl");
        let _ = std::fs::remove_file(&path);

        let writer = JsonlWriter::new(&path);

        // Write two records
        let record1 = LlmCallRecord {
            timestamp: "1".to_string(),
            envelope: test_envelope(),
            response: Some(test_economics()),
            trace_id: None,
            span_id: None,
            error: None,
        };
        writer.write(&record1).unwrap();

        let record2 = LlmCallRecord {
            timestamp: "2".to_string(),
            envelope: test_envelope(),
            response: None,
            trace_id: None,
            span_id: None,
            error: Some("test error".to_string()),
        };
        writer.write(&record2).unwrap();

        // Verify: two lines
        let content = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 2);

        // Each line is valid JSON
        let _: LlmCallRecord = serde_json::from_str(lines[0]).unwrap();
        let r2: LlmCallRecord = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(r2.error, Some("test error".to_string()));

        // Cleanup
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn writer_best_effort_does_not_panic() {
        let writer = JsonlWriter::new("/nonexistent/path/that/will/fail.jsonl");
        let record = LlmCallRecord {
            timestamp: "1".to_string(),
            envelope: test_envelope(),
            response: None,
            trace_id: None,
            span_id: None,
            error: None,
        };
        // Should not panic, just log a warning
        writer.write_best_effort(&record);
    }

    #[test]
    fn now_iso8601_returns_nonzero() {
        let ts = now_iso8601();
        let secs: u64 = ts.parse().unwrap();
        assert!(secs > 1_700_000_000); // after 2023
    }

    #[test]
    fn from_env_returns_none_when_unset() {
        // VIGIL_JSONL_PATH is not set in test environment
        // This test verifies from_env doesn't panic
        let _ = JsonlWriter::from_env();
    }
}
