//! nex-lsp: IDE-facing semantic coordination shim.
//!
//! This Phase 4 slice provides:
//! - a usable `nex-lsp` stdio server
//! - `nex/semanticDiff` as a custom JSON-RPC request
//! - `nex/activeLocks`, `nex/agentIntent`, `nex/validationStatus`,
//!   and `nex/eventStream` notifications
//! - lock-backed code lenses and validation diagnostics
//! - graceful degradation when `.nex/` state files are absent

pub mod backend;
pub mod config;
pub mod protocol;
mod upstream;

pub use backend::{CodexLspBackend, build_service};
pub use config::CodexLspConfig;
