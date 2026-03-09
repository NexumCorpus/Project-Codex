//! nex-core: Shared types, errors, and configuration for Nexum Graph.
//!
//! This crate defines the authoritative type system specified in the
//! Nexum Graph Implementation Specification. These types are contracts —
//! downstream crates implement against them but must not alter their signatures.

pub mod conflict;
pub mod coordination;
pub mod error;
pub mod semantic;
pub mod validation;

pub use conflict::*;
pub use coordination::*;
pub use error::*;
pub use semantic::*;
pub use validation::*;
