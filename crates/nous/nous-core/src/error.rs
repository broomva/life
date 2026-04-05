//! Nous error types.

use thiserror::Error;

/// Errors from Nous evaluation operations.
#[derive(Debug, Error)]
pub enum NousError {
    /// An evaluator failed during execution.
    #[error("evaluator '{name}' failed: {message}")]
    EvaluatorFailed { name: String, message: String },

    /// Registry error (duplicate name, not found).
    #[error("registry error: {0}")]
    Registry(String),

    /// Serialization/deserialization error.
    #[error("serde error: {0}")]
    Serde(#[from] serde_json::Error),

    /// Score value out of range.
    #[error("score out of range: {value} (expected 0.0..=1.0)")]
    ScoreOutOfRange { value: f64 },
}

/// Result type for Nous operations.
pub type NousResult<T> = Result<T, NousError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_display_evaluator_failed() {
        let err = NousError::EvaluatorFailed {
            name: "token_efficiency".into(),
            message: "division by zero".into(),
        };
        assert!(err.to_string().contains("token_efficiency"));
        assert!(err.to_string().contains("division by zero"));
    }

    #[test]
    fn error_display_score_out_of_range() {
        let err = NousError::ScoreOutOfRange { value: 1.5 };
        assert!(err.to_string().contains("1.5"));
    }
}
