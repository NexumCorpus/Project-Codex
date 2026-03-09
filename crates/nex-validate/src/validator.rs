//! Pre-commit validation engine.
//!
//! Compares two CodeGraphs (base vs HEAD), checks that all modified/deleted
//! units are covered by the appropriate semantic locks, and verifies
//! reference integrity (no broken references, no stale callers).
//!
//! Validation rules:
//! 1. **Lock coverage (modification)**: Every modified unit must have a
//!    Write lock held by the current agent. Match against `before.id`.
//! 2. **Lock coverage (deletion)**: Every deleted unit must have a Delete
//!    lock held by the current agent. Match against the removed unit's id.
//! 3. **Broken references**: If a unit was deleted, any surviving caller
//!    (exists in graph_after, was NOT modified) is a broken reference.
//! 4. **Stale callers**: If a unit's signature changed, any caller in
//!    graph_after that was NOT also modified is a stale caller.

use nex_core::{
    AgentId, ChangeKind, IntentKind, SemanticDiff, SemanticLock, SemanticUnit, Severity,
    ValidationIssue, ValidationKind, ValidationReport,
};
use nex_graph::CodeGraph;
use std::collections::HashSet;

/// Pre-commit validation engine.
///
/// Stateless — call `validate()` with two graphs, an agent identity,
/// and the current lock set.
pub struct ValidationEngine;

impl ValidationEngine {
    /// Run pre-commit validation.
    ///
    /// Compares `graph_before` (base ref) to `graph_after` (HEAD), checks
    /// lock coverage for the given agent, and verifies reference integrity.
    ///
    /// Algorithm:
    /// 1. Compute `graph_before.diff(graph_after)` to get the SemanticDiff.
    /// 2. Call `check_modification_locks` for each modified unit.
    /// 3. Call `check_deletion_locks` for each removed unit.
    /// 4. Call `check_broken_references` for removed units with surviving callers.
    /// 5. Call `check_stale_callers` for signature-changed units.
    /// 6. Sort issues by severity (errors first, then warnings).
    /// 7. Compute `units_checked` = modified + removed + added counts.
    /// 8. Return `ValidationReport`.
    pub fn validate(
        graph_before: &CodeGraph,
        graph_after: &CodeGraph,
        agent_name: &str,
        agent_id: AgentId,
        locks: &[SemanticLock],
    ) -> ValidationReport {
        let diff = graph_before.diff(graph_after);
        let mut issues = Vec::new();

        check_modification_locks(&diff, agent_name, agent_id, locks, &mut issues);
        check_deletion_locks(&diff, agent_name, agent_id, locks, &mut issues);
        check_broken_references(&diff, graph_before, graph_after, &mut issues);
        check_stale_callers(&diff, graph_after, &mut issues);

        issues.sort_by_key(|issue| severity_rank(issue.severity));

        let units_checked = diff.modified.len() + diff.removed.len() + diff.added.len();

        ValidationReport {
            issues,
            agent_name: agent_name.to_string(),
            units_checked,
        }
    }
}

/// Check that every modified unit has a Write lock held by the agent.
///
/// For each `modified` entry in the diff:
/// - The lock target must match `modified.before.id` (the pre-change SemanticId).
/// - The lock kind must be `IntentKind::Write`.
/// - The lock agent_id must match the provided `agent_id`.
///
/// If no matching lock is found, push an `UnlockedModification` issue
/// with `Severity::Error`.
///
/// ```rust,ignore
/// for modified in &diff.modified {
///     let has_lock = locks.iter().any(|l| {
///         l.agent_id == agent_id
///             && l.target == modified.before.id
///             && matches!(l.kind, IntentKind::Write)
///     });
///     if !has_lock {
///         issues.push(ValidationIssue {
///             kind: ValidationKind::UnlockedModification { unit: modified.before.id },
///             severity: Severity::Error,
///             unit_name: modified.after.qualified_name.clone(),
///             description: format!(
///                 "modified `{}` without a Write lock",
///                 modified.after.qualified_name
///             ),
///             suggestion: Some(format!(
///                 "run `nex lock {} {} write` first",
///                 agent_name, modified.after.qualified_name
///             )),
///         });
///     }
/// }
/// ```
fn check_modification_locks(
    diff: &SemanticDiff,
    agent_name: &str,
    agent_id: AgentId,
    locks: &[SemanticLock],
    issues: &mut Vec<ValidationIssue>,
) {
    for modified in &diff.modified {
        let has_lock = locks.iter().any(|lock| {
            lock.agent_id == agent_id
                && lock.target == modified.before.id
                && matches!(lock.kind, IntentKind::Write)
        });

        if !has_lock {
            issues.push(ValidationIssue {
                kind: ValidationKind::UnlockedModification {
                    unit: modified.before.id,
                },
                severity: Severity::Error,
                unit_name: modified.after.qualified_name.clone(),
                description: format!(
                    "modified `{}` without a Write lock",
                    modified.after.qualified_name
                ),
                suggestion: Some(format!(
                    "run `nex lock {} {} write` first",
                    agent_name, modified.after.qualified_name
                )),
            });
        }
    }
}

/// Check that every deleted unit has a Delete lock held by the agent.
///
/// For each `removed` unit in the diff:
/// - The lock target must match `removed.id`.
/// - The lock kind must be `IntentKind::Delete`.
/// - The lock agent_id must match the provided `agent_id`.
///
/// If no matching lock is found, push an `UnlockedDeletion` issue
/// with `Severity::Error`.
///
/// ```rust,ignore
/// for removed in &diff.removed {
///     let has_lock = locks.iter().any(|l| {
///         l.agent_id == agent_id
///             && l.target == removed.id
///             && matches!(l.kind, IntentKind::Delete)
///     });
///     if !has_lock {
///         issues.push(ValidationIssue {
///             kind: ValidationKind::UnlockedDeletion { unit: removed.id },
///             severity: Severity::Error,
///             unit_name: removed.qualified_name.clone(),
///             description: format!(
///                 "deleted `{}` without a Delete lock",
///                 removed.qualified_name
///             ),
///             suggestion: Some(format!(
///                 "run `nex lock {} {} delete` first",
///                 agent_name, removed.qualified_name
///             )),
///         });
///     }
/// }
/// ```
fn check_deletion_locks(
    diff: &SemanticDiff,
    agent_name: &str,
    agent_id: AgentId,
    locks: &[SemanticLock],
    issues: &mut Vec<ValidationIssue>,
) {
    for removed in &diff.removed {
        let has_lock = locks.iter().any(|lock| {
            lock.agent_id == agent_id
                && lock.target == removed.id
                && matches!(lock.kind, IntentKind::Delete)
        });

        if !has_lock {
            issues.push(ValidationIssue {
                kind: ValidationKind::UnlockedDeletion { unit: removed.id },
                severity: Severity::Error,
                unit_name: removed.qualified_name.clone(),
                description: format!("deleted `{}` without a Delete lock", removed.qualified_name),
                suggestion: Some(format!(
                    "run `nex lock {} {} delete` first",
                    agent_name, removed.qualified_name
                )),
            });
        }
    }
}

/// Check for broken references caused by deleted units.
///
/// For each removed unit:
/// 1. Find its callers in `graph_before` via `graph_before.callers_of(&removed.id)`.
/// 2. Build a `HashSet<&str>` of modified qualified_names from the diff.
/// 3. For each caller, check if it still exists in `graph_after`
///    (search by `qualified_name` using `find_unit_by_name`).
/// 4. If the caller exists AND was NOT modified in the diff, it's a
///    `BrokenReference` with `Severity::Error`.
///
/// ```rust,ignore
/// let modified_names: HashSet<&str> = diff.modified.iter()
///     .map(|m| m.after.qualified_name.as_str())
///     .collect();
///
/// for removed in &diff.removed {
///     for caller in graph_before.callers_of(&removed.id) {
///         if modified_names.contains(caller.qualified_name.as_str()) {
///             continue; // Caller was modified — benefit of the doubt.
///         }
///         if find_unit_by_name(graph_after, &caller.qualified_name).is_some() {
///             issues.push(ValidationIssue {
///                 kind: ValidationKind::BrokenReference {
///                     deleted: removed.id,
///                     referencing: caller.id,
///                 },
///                 severity: Severity::Error,
///                 unit_name: caller.qualified_name.clone(),
///                 description: format!(
///                     "`{}` still references deleted `{}`",
///                     caller.qualified_name, removed.qualified_name
///                 ),
///                 suggestion: Some(format!(
///                     "update `{}` to remove the reference to `{}`",
///                     caller.name, removed.name
///                 )),
///             });
///         }
///     }
/// }
/// ```
fn check_broken_references(
    diff: &SemanticDiff,
    graph_before: &CodeGraph,
    graph_after: &CodeGraph,
    issues: &mut Vec<ValidationIssue>,
) {
    let modified_names: HashSet<&str> = diff
        .modified
        .iter()
        .map(|modified| modified.after.qualified_name.as_str())
        .collect();

    for removed in &diff.removed {
        for caller in graph_before.callers_of(&removed.id) {
            if modified_names.contains(caller.qualified_name.as_str()) {
                continue;
            }

            if find_unit_by_name(graph_after, &caller.qualified_name).is_some() {
                issues.push(ValidationIssue {
                    kind: ValidationKind::BrokenReference {
                        deleted: removed.id,
                        referencing: caller.id,
                    },
                    severity: Severity::Error,
                    unit_name: caller.qualified_name.clone(),
                    description: format!(
                        "`{}` still references deleted `{}`",
                        caller.qualified_name, removed.qualified_name
                    ),
                    suggestion: Some(format!(
                        "update `{}` to remove the reference to `{}`",
                        caller.name, removed.name
                    )),
                });
            }
        }
    }
}

/// Check for stale callers after signature changes.
///
/// For each modified unit where `changes` contains `SignatureChanged`:
/// 1. Find the unit in `graph_after` by qualified_name.
/// 2. Find callers of that unit in `graph_after` via `callers_of`.
/// 3. Build a `HashSet<&str>` of modified qualified_names from the diff.
/// 4. For each caller that was NOT also modified, push a `StaleCallers`
///    issue with `Severity::Warning`.
///
/// ```rust,ignore
/// let modified_names: HashSet<&str> = diff.modified.iter()
///     .map(|m| m.after.qualified_name.as_str())
///     .collect();
///
/// for modified in &diff.modified {
///     if !modified.changes.contains(&ChangeKind::SignatureChanged) {
///         continue;
///     }
///     let Some(after_unit) = find_unit_by_name(graph_after, &modified.after.qualified_name) else {
///         continue;
///     };
///     for caller in graph_after.callers_of(&after_unit.id) {
///         if modified_names.contains(caller.qualified_name.as_str()) {
///             continue; // Caller was also modified — likely updated.
///         }
///         issues.push(ValidationIssue {
///             kind: ValidationKind::StaleCallers {
///                 function: after_unit.id,
///                 caller: caller.id,
///             },
///             severity: Severity::Warning,
///             unit_name: caller.qualified_name.clone(),
///             description: format!(
///                 "`{}` may be using old signature of `{}`",
///                 caller.qualified_name, modified.after.qualified_name
///             ),
///             suggestion: Some(format!(
///                 "update `{}` to match new signature of `{}`",
///                 caller.name, modified.after.name
///             )),
///         });
///     }
/// }
/// ```
fn check_stale_callers(
    diff: &SemanticDiff,
    graph_after: &CodeGraph,
    issues: &mut Vec<ValidationIssue>,
) {
    let modified_names: HashSet<&str> = diff
        .modified
        .iter()
        .map(|modified| modified.after.qualified_name.as_str())
        .collect();

    for modified in &diff.modified {
        if !modified.changes.contains(&ChangeKind::SignatureChanged) {
            continue;
        }

        let Some(after_unit) = find_unit_by_name(graph_after, &modified.after.qualified_name)
        else {
            continue;
        };

        for caller in graph_after.callers_of(&after_unit.id) {
            if modified_names.contains(caller.qualified_name.as_str()) {
                continue;
            }

            issues.push(ValidationIssue {
                kind: ValidationKind::StaleCallers {
                    function: after_unit.id,
                    caller: caller.id,
                },
                severity: Severity::Warning,
                unit_name: caller.qualified_name.clone(),
                description: format!(
                    "`{}` may be using old signature of `{}`",
                    caller.qualified_name, modified.after.qualified_name
                ),
                suggestion: Some(format!(
                    "update `{}` to match new signature of `{}`",
                    caller.name, modified.after.name
                )),
            });
        }
    }
}

/// Find a semantic unit by qualified_name in the graph.
///
/// Returns `Some(&SemanticUnit)` if found, `None` otherwise.
/// Uses O(n) scan over `graph.units()`.
///
/// ```rust,ignore
/// graph.units().into_iter().find(|u| u.qualified_name == name)
/// ```
fn find_unit_by_name<'a>(graph: &'a CodeGraph, name: &str) -> Option<&'a SemanticUnit> {
    graph
        .units()
        .into_iter()
        .find(|unit| unit.qualified_name == name)
}

/// Map severity to a sort rank (errors first).
///
/// ```rust,ignore
/// match severity {
///     Severity::Error => 0,
///     Severity::Warning => 1,
///     Severity::Info => 2,
/// }
/// ```
fn severity_rank(severity: Severity) -> u8 {
    match severity {
        Severity::Error => 0,
        Severity::Warning => 1,
        Severity::Info => 2,
    }
}
