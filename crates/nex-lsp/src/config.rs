//! Runtime configuration for the `nex-lsp` shim.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Runtime configuration for the LSP backend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexLspConfig {
    /// Repository root used for git reads and `.nex/` state files.
    pub repo_path: Option<PathBuf>,
    /// Base ref used when serving semantic diff and validation requests.
    pub base_ref: String,
    /// Poll interval for semantic event notifications.
    pub event_poll_ms: u64,
    /// Optional upstream stdio language server command.
    pub upstream_command: Option<String>,
    /// Arguments passed to the upstream stdio language server command.
    pub upstream_args: Vec<String>,
}

impl Default for CodexLspConfig {
    fn default() -> Self {
        Self {
            repo_path: None,
            base_ref: "HEAD~1".to_string(),
            event_poll_ms: 500,
            upstream_command: None,
            upstream_args: Vec::new(),
        }
    }
}
