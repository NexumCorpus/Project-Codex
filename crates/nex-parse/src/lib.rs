//! nex-parse: Tree-sitter parsing, rowan CST bridge, semantic unit extraction.
//!
//! This crate owns:
//! - `SemanticExtractor` trait (language-agnostic extraction interface)
//! - Tree-sitter → Rowan bridge (first-of-its-kind in the Rust ecosystem)
//! - Per-language extractors (TypeScript P0, Python P0, Rust P1)
//! - `KindMap` generation from tree-sitter node-types.json

use nex_core::{CodexResult, DepKind, SemanticId, SemanticUnit};
use std::path::Path;

pub mod bridge;
pub mod python;
pub mod rust;
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

/// Create a Python semantic extractor.
pub fn python_extractor() -> Box<dyn SemanticExtractor> {
    Box::new(python::PythonExtractor::new())
}

/// Create a Rust semantic extractor.
pub fn rust_extractor() -> Box<dyn SemanticExtractor> {
    Box::new(rust::RustExtractor::new())
}

/// Create the default multi-language extractor set from the spec.
pub fn default_extractors() -> Vec<Box<dyn SemanticExtractor>> {
    vec![typescript_extractor(), python_extractor(), rust_extractor()]
}

/// Supported source file extensions across the default extractor set.
pub fn supported_extensions() -> &'static [&'static str] {
    &["ts", "tsx", "py", "rs"]
}

/// Return whether the extension is supported by any built-in extractor.
pub fn supports_extension(ext: &str) -> bool {
    supported_extensions().contains(&ext)
}

/// Create the built-in extractor that handles the given extension.
pub fn extractor_for_extension(ext: &str) -> Option<Box<dyn SemanticExtractor>> {
    match ext {
        "ts" | "tsx" => Some(typescript_extractor()),
        "py" => Some(python_extractor()),
        "rs" => Some(rust_extractor()),
        _ => None,
    }
}

/// Create the built-in extractor that handles the given path.
pub fn extractor_for_path(path: &Path) -> Option<Box<dyn SemanticExtractor>> {
    let ext = path.extension()?.to_str()?;
    extractor_for_extension(ext)
}
