//! Authoritative semantic types for Project Codex.
//!
//! These types are transcribed directly from the Implementation Specification
//! and constitute the API contract for all downstream crates. Codex (the code
//! generator) must not alter these type signatures.

use serde::{Deserialize, Serialize};
use std::ops::Range;
use std::path::PathBuf;

// ─────────────────────────────────────────────────────────────────────────────
// Core Identity
// ─────────────────────────────────────────────────────────────────────────────

/// Content-addressed identity for semantic units.
/// BLAKE3 256-bit hash over the unit's qualified name, file path, and content.
pub type SemanticId = [u8; 32];

// ─────────────────────────────────────────────────────────────────────────────
// Semantic Unit (the fundamental node)
// ─────────────────────────────────────────────────────────────────────────────

/// A single semantic unit extracted from source code.
///
/// Represents a function, class, interface, module, or other named code entity
/// at a granularity suitable for semantic diffing and coordination.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct SemanticUnit {
    /// Content-addressed hash (BLAKE3 over qualified_name + file_path + body).
    pub id: SemanticId,
    /// What kind of code entity this is.
    pub kind: UnitKind,
    /// Short name, e.g. "validateToken".
    pub name: String,
    /// Fully qualified name, e.g. "auth::AuthManager::validateToken".
    pub qualified_name: String,
    /// Path to the source file.
    pub file_path: PathBuf,
    /// Byte range within the source file.
    pub byte_range: Range<usize>,
    /// Hash of parameter types + return type (for change detection).
    pub signature_hash: u64,
    /// Hash of normalized body AST (for change detection).
    pub body_hash: u64,
    /// IDs of units this unit depends on (calls, imports, inherits, etc.).
    pub dependencies: Vec<SemanticId>,
}

/// Classification of semantic unit kinds.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum UnitKind {
    Function,
    Method,
    Class,
    Struct,
    Interface,
    Trait,
    Enum,
    Module,
    Constant,
}

// ─────────────────────────────────────────────────────────────────────────────
// Semantic Diff (output of graph comparison)
// ─────────────────────────────────────────────────────────────────────────────

/// The result of comparing two CodeGraphs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticDiff {
    /// Units present in `after` but not `before`.
    pub added: Vec<SemanticUnit>,
    /// Units present in `before` but not `after`.
    pub removed: Vec<SemanticUnit>,
    /// Units present in both but with changed signature or body.
    pub modified: Vec<ModifiedUnit>,
    /// Units that moved to a different file path.
    pub moved: Vec<MovedUnit>,
}

/// A unit that exists in both refs but has changed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModifiedUnit {
    /// The unit as it appeared before the change.
    pub before: SemanticUnit,
    /// The unit as it appears after the change.
    pub after: SemanticUnit,
    /// What specifically changed.
    pub changes: Vec<ChangeKind>,
}

/// Classification of what changed within a modified unit.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ChangeKind {
    /// Parameter types, return type, or visibility changed.
    SignatureChanged,
    /// Function/method body changed (but signature is the same).
    BodyChanged,
    /// Documentation or comments changed.
    DocChanged,
}

/// A unit that moved from one file path to another.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MovedUnit {
    /// The unit (with its current state).
    pub unit: SemanticUnit,
    /// Where it was before.
    pub old_path: PathBuf,
    /// Where it is now.
    pub new_path: PathBuf,
}

// ─────────────────────────────────────────────────────────────────────────────
// Dependency Classification
// ─────────────────────────────────────────────────────────────────────────────

/// Classification of dependency edges between semantic units.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum DepKind {
    /// Unit A calls unit B.
    Calls,
    /// Unit A imports unit B.
    Imports,
    /// Unit A inherits from / extends unit B.
    Inherits,
    /// Unit A implements unit B (trait/interface).
    Implements,
    /// Unit A uses unit B (type reference, field access, etc.).
    Uses,
}

// ─────────────────────────────────────────────────────────────────────────────
// File Identity (for git integration)
// ─────────────────────────────────────────────────────────────────────────────

/// Identifies a source file at a specific git revision.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct FileIdentity {
    /// Relative path within the repository.
    pub path: PathBuf,
    /// Git blob OID (if available).
    pub blob_oid: Option<[u8; 20]>,
    /// Language detected from file extension.
    pub language: Language,
}

/// Supported programming languages.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum Language {
    TypeScript,
    Tsx,
    Python,
    Rust,
    Go,
    Java,
    Unknown,
}

impl Language {
    /// Detect language from file extension.
    pub fn from_extension(ext: &str) -> Self {
        match ext {
            "ts" => Language::TypeScript,
            "tsx" => Language::Tsx,
            "py" => Language::Python,
            "rs" => Language::Rust,
            "go" => Language::Go,
            "java" => Language::Java,
            _ => Language::Unknown,
        }
    }
}
