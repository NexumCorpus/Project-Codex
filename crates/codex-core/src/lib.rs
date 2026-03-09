//! codex-core: Shared types, errors, and configuration for Project Codex.
//!
//! This crate defines the authoritative type system specified in the
//! Project Codex Implementation Specification. These types are contracts —
//! downstream crates implement against them but must not alter their signatures.

pub mod semantic;
pub mod error;

pub use semantic::*;
pub use error::*;
