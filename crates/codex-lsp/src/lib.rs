//! codex-lsp: IDE-facing semantic coordination shim.
//!
//! This Phase 4 slice provides:
//! - a usable `codex-lsp` stdio server
//! - `codex/semanticDiff` as a custom JSON-RPC request
//! - `codex/activeLocks`, `codex/agentIntent`, `codex/validationStatus`,
//!   and `codex/eventStream` notifications
//! - lock-backed code lenses and validation diagnostics
//! - graceful degradation when `.codex/` state files are absent

pub mod backend;
pub mod config;
pub mod protocol;
mod upstream;

pub use backend::{CodexLspBackend, build_service};
pub use config::CodexLspConfig;
