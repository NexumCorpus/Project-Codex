//! Authoritative validation types for Phase 2 pre-commit checks.
//!
//! These types are transcribed from the Implementation Specification §Phase 2.
//! They define the output contract for `nex validate` — the pre-commit
//! validation engine that checks lock coverage and reference integrity.
//! Codex must not alter these type signatures.

use crate::{SemanticId, Severity};
use serde::{Deserialize, Serialize};

// ─────────────────────────────────────────────────────────────────────────────
// Validation Issue (a single problem found)
// ─────────────────────────────────────────────────────────────────────────────

/// A single validation issue found during pre-commit checking.
///
/// Produced by `ValidationEngine::validate()` when the current commit's
/// changes violate lock coverage rules or reference integrity constraints.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationIssue {
    /// What kind of validation issue this is.
    pub kind: ValidationKind,
    /// How severe the issue is.
    pub severity: Severity,
    /// Human-readable name of the primary unit involved.
    pub unit_name: String,
    /// Human-readable description of the issue.
    pub description: String,
    /// Optional suggestion for resolution.
    pub suggestion: Option<String>,
}

/// Classification of validation issues.
///
/// Each variant captures the specific IDs involved, enabling precise
/// error messages and automated fix suggestions.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ValidationKind {
    /// A unit was modified but the agent does not hold a Write lock on it.
    ///
    /// The `unit` field is the **before** SemanticId (matching the lock target).
    UnlockedModification { unit: SemanticId },

    /// A unit was deleted but the agent does not hold a Delete lock on it.
    ///
    /// The `unit` field is the SemanticId from the before graph.
    UnlockedDeletion { unit: SemanticId },

    /// A deleted unit is still referenced by an existing unit that was not
    /// updated to remove the reference.
    ///
    /// `deleted` is the SemanticId of the removed unit (from before graph).
    /// `referencing` is the SemanticId of the caller that still exists (from before graph).
    BrokenReference {
        deleted: SemanticId,
        referencing: SemanticId,
    },

    /// A function's signature changed but a caller was not updated.
    ///
    /// `function` is the SemanticId of the modified function (from after graph).
    /// `caller` is the SemanticId of the un-updated caller (from after graph).
    StaleCallers {
        function: SemanticId,
        caller: SemanticId,
    },
}

// ─────────────────────────────────────────────────────────────────────────────
// Validation Report (aggregated output)
// ─────────────────────────────────────────────────────────────────────────────

/// Aggregated result of pre-commit validation.
///
/// Exit codes: 0 = clean, 1 = errors found, 2 = warnings only.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationReport {
    /// All detected validation issues, ordered by severity (errors first).
    pub issues: Vec<ValidationIssue>,
    /// The agent name whose lock coverage was checked.
    pub agent_name: String,
    /// Number of changed units that were checked (modified + removed + added).
    pub units_checked: usize,
}

impl ValidationReport {
    /// Count of Error-severity issues.
    pub fn error_count(&self) -> usize {
        self.issues
            .iter()
            .filter(|i| i.severity == Severity::Error)
            .count()
    }

    /// Count of Warning-severity issues.
    pub fn warning_count(&self) -> usize {
        self.issues
            .iter()
            .filter(|i| i.severity == Severity::Warning)
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
