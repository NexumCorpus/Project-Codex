//! Error types for Project Codex.
//!
//! Each crate may define its own error variants, but the top-level
//! categories are defined here for cross-crate use.

use thiserror::Error;

/// Top-level error type for codex operations.
#[derive(Debug, Error)]
pub enum CodexError {
    #[error("parse error in {path}: {message}")]
    Parse {
        path: String,
        message: String,
    },

    #[error("git error: {0}")]
    Git(String),

    #[error("graph error: {0}")]
    Graph(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}

/// Alias for Results using CodexError.
pub type CodexResult<T> = Result<T, CodexError>;
