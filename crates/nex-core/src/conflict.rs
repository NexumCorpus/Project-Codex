//! Authoritative conflict detection types for Phase 1.
//!
//! These types are transcribed from the Implementation Specification §Phase 1.
//! They define the output contract for `nex check` — the semantic conflict
//! detection engine. Codex must not alter these type signatures.

use crate::{SemanticId, SemanticUnit};
use serde::{Deserialize, Serialize};

// ─────────────────────────────────────────────────────────────────────────────
// Semantic Conflict (the output of conflict detection)
// ─────────────────────────────────────────────────────────────────────────────

/// A semantic conflict detected between two branches.
///
/// Produced by `ConflictDetector::detect()` when cross-referencing
/// two semantic diffs (base→A, base→B) reveals incompatible changes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticConflict {
    /// What kind of conflict this is.
    pub kind: ConflictKind,
    /// How severe the conflict is.
    pub severity: Severity,
    /// The unit from branch A involved in the conflict.
    pub unit_a: SemanticUnit,
    /// The unit from branch B involved in the conflict.
    pub unit_b: SemanticUnit,
    /// Human-readable description of the conflict.
    pub description: String,
    /// Optional suggestion for resolution.
    pub suggestion: Option<String>,
}

/// Classification of semantic conflicts.
///
/// Each variant captures the specific IDs involved, enabling precise
/// error messages and automated fix suggestions.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ConflictKind {
    /// Branch A renamed/removed a function that branch B still calls.
    BrokenReference {
        caller: SemanticId,
        callee: SemanticId,
    },
    /// Both branches modify the same function body.
    ConcurrentBodyEdit { unit: SemanticId },
    /// Branch A changed a function's signature, but branch B calls it
    /// expecting the old signature.
    SignatureMismatch {
        function: SemanticId,
        caller: SemanticId,
    },
    /// Branch A deleted a unit that branch B depends on.
    DeletedDependency {
        deleted: SemanticId,
        dependent: SemanticId,
    },
    /// Both branches introduce a unit with the same qualified name.
    NamingCollision { name: String },
    /// Branch A changed an interface, but branch B's implementor
    /// doesn't conform to the new shape.
    InterfaceDrift {
        interface_id: SemanticId,
        implementor: SemanticId,
    },
}

/// Severity levels for semantic conflicts.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// Informational only — no action required.
    Info,
    /// May cause issues — manual review recommended.
    Warning,
    /// Will break at compile/runtime — must resolve before merge.
    Error,
}

// ─────────────────────────────────────────────────────────────────────────────
// Conflict Report (aggregated output)
// ─────────────────────────────────────────────────────────────────────────────

/// Aggregated result of conflict detection.
///
/// Exit codes: 0 = clean, 1 = errors found, 2 = warnings only.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConflictReport {
    /// All detected conflicts, ordered by severity (errors first).
    pub conflicts: Vec<SemanticConflict>,
    /// Branch A ref string.
    pub branch_a: String,
    /// Branch B ref string.
    pub branch_b: String,
    /// Merge base commit (hex string).
    pub merge_base: String,
}

impl ConflictReport {
    /// Count of Error-severity conflicts.
    pub fn error_count(&self) -> usize {
        self.conflicts
            .iter()
            .filter(|c| c.severity == Severity::Error)
            .count()
    }

    /// Count of Warning-severity conflicts.
    pub fn warning_count(&self) -> usize {
        self.conflicts
            .iter()
            .filter(|c| c.severity == Severity::Warning)
            .count()
    }

    /// Suggested exit code: 0 = clean, 1 = errors, 2 = warnings only.
    pub fn exit_code(&self) -> i32 {
        if self.error_count() > 0 {
            1
        } else if self.warning_count() > 0 {
            2
        } else {
            0
        }
    }
}
