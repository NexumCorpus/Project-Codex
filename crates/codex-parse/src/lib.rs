//! codex-parse: Tree-sitter parsing, rowan CST bridge, semantic unit extraction.
//!
//! This crate owns:
//! - `SemanticExtractor` trait (language-agnostic extraction interface)
//! - Tree-sitter → Rowan bridge (first-of-its-kind in the Rust ecosystem)
//! - Per-language extractors (TypeScript P0, Python P0, Rust P1)
//! - `KindMap` generation from tree-sitter node-types.json

use codex_core::{CodexResult, DepKind, SemanticId, SemanticUnit};
use std::path::Path;

pub mod bridge;
pub mod typescript;

// ─────────────────────────────────────────────────────────────────────────────
// SemanticExtractor Trait (SPEC CONTRACT — do not alter signature)
// ─────────────────────────────────────────────────────────────────────────────

/// Language-agnostic extraction interface.
/// Each language implements this with tree-sitter queries.
pub trait SemanticExtractor: Send + Sync {
    /// Supported file extensions (e.g., &["ts", "tsx"] for TypeScript).
    fn extensions(&self) -> &[&str];

    /// Parse source bytes, return extracted semantic units.
    fn extract(&self, path: &Path, source: &[u8]) -> CodexResult<Vec<SemanticUnit>>;

    /// Extract dependency edges between units.
    /// Returns (from_id, to_id, dependency_kind) triples.
    fn dependencies(
        &self,
        units: &[SemanticUnit],
        source: &[u8],
    ) -> CodexResult<Vec<(SemanticId, SemanticId, DepKind)>>;
}

// ─────────────────────────────────────────────────────────────────────────────
// Public API — extractor constructors
// ─────────────────────────────────────────────────────────────────────────────

/// Create a TypeScript semantic extractor.
pub fn typescript_extractor() -> Box<dyn SemanticExtractor> {
    Box::new(typescript::TypeScriptExtractor::new())
}
