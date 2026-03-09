//! nex-validate: Pre-commit validation hooks and constraint checking.
//!
//! Phase 2: `ValidationEngine` — checks that committed changes are
//! covered by the appropriate semantic locks and that reference
//! integrity is maintained (no broken references, no stale callers).

pub mod validator;

pub use validator::ValidationEngine;
