//! Output formatting for semantic diffs.
//!
//! Supports three formats:
//! - `json`: Machine-readable JSON via serde_json
//! - `text`: Human-readable summary
//! - `github`: GitHub-flavored markdown for PR comments

use nex_core::{ChangeKind, ConflictKind, ConflictReport, SemanticDiff, Severity};
use std::fmt::Write;

/// Format a SemanticDiff according to the requested format string.
///
/// Supported formats: "json", "text", "github".
/// Unknown formats fall back to "text".
pub fn format_diff(diff: &SemanticDiff, format: &str) -> String {
    match format {
        "json" => format_json(diff),
        "github" => format_github(diff),
        _ => format_text(diff),
    }
}

/// Serialize the diff as pretty-printed JSON.
fn format_json(diff: &SemanticDiff) -> String {
    serde_json::to_string_pretty(diff).unwrap_or_else(|err| format!("{{\"error\": \"{err}\"}}"))
}

/// Render a human-readable text summary.
fn format_text(diff: &SemanticDiff) -> String {
    let mut output = String::new();
    let _ = writeln!(output, "Semantic Diff Summary");
    let _ = writeln!(output, "=====================");
    let _ = writeln!(output, "Added:    {}", diff.added.len());
    let _ = writeln!(output, "Removed:  {}", diff.removed.len());
    let _ = writeln!(output, "Modified: {}", diff.modified.len());
    let _ = writeln!(output, "Moved:    {}", diff.moved.len());

    if !diff.added.is_empty() {
        let _ = writeln!(output);
        let _ = writeln!(output, "Added:");
        for unit in &diff.added {
            let _ = writeln!(
                output,
                "  + {:?} {} ({})",
                unit.kind,
                unit.qualified_name,
                unit.file_path.display()
            );
        }
    }

    if !diff.removed.is_empty() {
        let _ = writeln!(output);
        let _ = writeln!(output, "Removed:");
        for unit in &diff.removed {
            let _ = writeln!(
                output,
                "  - {:?} {} ({})",
                unit.kind,
                unit.qualified_name,
                unit.file_path.display()
            );
        }
    }

    if !diff.modified.is_empty() {
        let _ = writeln!(output);
        let _ = writeln!(output, "Modified:");
        for modified in &diff.modified {
            let changes = join_changes(&modified.changes);
            let _ = writeln!(
                output,
                "  ~ {:?} {} [{}]",
                modified.after.kind, modified.after.qualified_name, changes
            );
        }
    }

    if !diff.moved.is_empty() {
        let _ = writeln!(output);
        let _ = writeln!(output, "Moved:");
        for moved in &diff.moved {
            let _ = writeln!(
                output,
                "  -> {:?} {}: {} -> {}",
                moved.unit.kind,
                moved.unit.qualified_name,
                moved.old_path.display(),
                moved.new_path.display()
            );
        }
    }

    output
}

/// Render GitHub-flavored markdown for PR comments.
fn format_github(diff: &SemanticDiff) -> String {
    let mut output = String::new();
    let _ = writeln!(output, "# Semantic Diff");
    let _ = writeln!(output);
    let _ = writeln!(output, "| Category | Count |");
    let _ = writeln!(output, "|----------|-------|");
    let _ = writeln!(output, "| Added | {} |", diff.added.len());
    let _ = writeln!(output, "| Removed | {} |", diff.removed.len());
    let _ = writeln!(output, "| Modified | {} |", diff.modified.len());
    let _ = writeln!(output, "| Moved | {} |", diff.moved.len());

    if !diff.added.is_empty() {
        let _ = writeln!(output);
        let _ = writeln!(output, "## Added");
        let _ = writeln!(output, "| Kind | Name | File |");
        let _ = writeln!(output, "|------|------|------|");
        for unit in &diff.added {
            let _ = writeln!(
                output,
                "| {:?} | `{}` | `{}` |",
                unit.kind,
                unit.qualified_name,
                unit.file_path.display()
            );
        }
    }

    if !diff.removed.is_empty() {
        let _ = writeln!(output);
        let _ = writeln!(output, "## Removed");
        let _ = writeln!(output, "| Kind | Name | File |");
        let _ = writeln!(output, "|------|------|------|");
        for unit in &diff.removed {
            let _ = writeln!(
                output,
                "| {:?} | `{}` | `{}` |",
                unit.kind,
                unit.qualified_name,
                unit.file_path.display()
            );
        }
    }

    if !diff.modified.is_empty() {
        let _ = writeln!(output);
        let _ = writeln!(output, "## Modified");
        let _ = writeln!(output, "| Kind | Name | Changes |");
        let _ = writeln!(output, "|------|------|---------|");
        for modified in &diff.modified {
            let _ = writeln!(
                output,
                "| {:?} | `{}` | {} |",
                modified.after.kind,
                modified.after.qualified_name,
                join_changes(&modified.changes)
            );
        }
    }

    if !diff.moved.is_empty() {
        let _ = writeln!(output);
        let _ = writeln!(output, "## Moved");
        let _ = writeln!(output, "| Kind | Name | From | To |");
        let _ = writeln!(output, "|------|------|------|-----|");
        for moved in &diff.moved {
            let _ = writeln!(
                output,
                "| {:?} | `{}` | `{}` | `{}` |",
                moved.unit.kind,
                moved.unit.qualified_name,
                moved.old_path.display(),
                moved.new_path.display()
            );
        }
    }

    output
}

/// Format a ConflictReport according to the requested format string.
///
/// Supported formats: "json", "text", "github".
/// Unknown formats fall back to "text".
pub fn format_report(report: &ConflictReport, format: &str) -> String {
    match format {
        "json" => serde_json::to_string_pretty(report)
            .unwrap_or_else(|err| format!("{{\"error\": \"{err}\"}}")),
        "github" => format_report_github(report),
        _ => format_report_text(report),
    }
}

fn join_changes(changes: &[ChangeKind]) -> String {
    changes
        .iter()
        .map(change_label)
        .collect::<Vec<_>>()
        .join(", ")
}

fn change_label(change: &ChangeKind) -> &'static str {
    match change {
        ChangeKind::SignatureChanged => "signature",
        ChangeKind::BodyChanged => "body",
        ChangeKind::DocChanged => "docs",
    }
}

fn format_report_text(report: &ConflictReport) -> String {
    let mut output = String::new();
    let _ = writeln!(
        output,
        "Conflict Check: {} vs {}",
        report.branch_a, report.branch_b
    );
    let _ = writeln!(output, "Merge base: {}", report.merge_base);
    let _ = writeln!(output, "=====================================");
    let _ = writeln!(output, "Errors:   {}", report.error_count());
    let _ = writeln!(output, "Warnings: {}", report.warning_count());

    if !report.conflicts.is_empty() {
        let _ = writeln!(output);
    }

    for conflict in &report.conflicts {
        let _ = writeln!(
            output,
            "[{}] {}: {}",
            severity_label(conflict.severity),
            conflict_kind_label(&conflict.kind),
            conflict.description
        );
        if let Some(suggestion) = &conflict.suggestion {
            let _ = writeln!(output, "  Suggestion: {suggestion}");
        }
        let _ = writeln!(output);
    }

    let _ = writeln!(output, "Exit code: {}", report.exit_code());
    output
}

fn format_report_github(report: &ConflictReport) -> String {
    let mut output = String::new();
    let _ = writeln!(output, "# Conflict Check");
    let _ = writeln!(output);
    let _ = writeln!(
        output,
        "**Branches**: `{}` vs `{}`",
        report.branch_a, report.branch_b
    );
    let _ = writeln!(output, "**Merge base**: `{}`", report.merge_base);
    let _ = writeln!(output);
    let _ = writeln!(output, "| Severity | Count |");
    let _ = writeln!(output, "|----------|-------|");
    let _ = writeln!(output, "| Error | {} |", report.error_count());
    let _ = writeln!(output, "| Warning | {} |", report.warning_count());

    if !report.conflicts.is_empty() {
        let _ = writeln!(output);
        let _ = writeln!(output, "## Conflicts");
        let _ = writeln!(output, "| # | Severity | Kind | Description |");
        let _ = writeln!(output, "|---|----------|------|-------------|");
        for (index, conflict) in report.conflicts.iter().enumerate() {
            let _ = writeln!(
                output,
                "| {} | {} | {} | {} |",
                index + 1,
                severity_title(conflict.severity),
                conflict_kind_label(&conflict.kind),
                conflict.description
            );
        }
    }

    output
}

fn severity_label(severity: Severity) -> &'static str {
    match severity {
        Severity::Info => "INFO",
        Severity::Warning => "WARNING",
        Severity::Error => "ERROR",
    }
}

fn severity_title(severity: Severity) -> &'static str {
    match severity {
        Severity::Info => "Info",
        Severity::Warning => "Warning",
        Severity::Error => "Error",
    }
}

fn conflict_kind_label(kind: &ConflictKind) -> &'static str {
    match kind {
        ConflictKind::BrokenReference { .. } => "BrokenReference",
        ConflictKind::ConcurrentBodyEdit { .. } => "ConcurrentBodyEdit",
        ConflictKind::SignatureMismatch { .. } => "SignatureMismatch",
        ConflictKind::DeletedDependency { .. } => "DeletedDependency",
        ConflictKind::NamingCollision { .. } => "NamingCollision",
        ConflictKind::InterfaceDrift { .. } => "InterfaceDrift",
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Phase 2: Coordination output
// ─────────────────────────────────────────────────────────────────────────────

use crate::coordination_pipeline::LockEntry;
use nex_core::SemanticUnit;
use nex_core::{LockResult, ValidationReport};
use nex_eventlog::{RollbackOutcome, SemanticEvent};

/// Format a `LockResult` for CLI output.
///
/// Supported formats: `"json"`, `"text"`.
/// Unknown formats fall back to `"text"`.
///
/// **Text format (Granted)**:
/// ```text
/// Lock GRANTED: {agent_name} -> {target_name}
/// ```
///
/// **Text format (Denied)**:
/// ```text
/// Lock DENIED: {agent_name} -> {target_name}
/// Conflicts:
///   - {conflict.reason}
///   - {conflict.reason}
/// ```
///
/// **JSON format**: Pretty-printed `LockResult` via serde_json.
pub fn format_lock_result(
    result: &LockResult,
    agent_name: &str,
    target_name: &str,
    format: &str,
) -> String {
    match format {
        "json" => serde_json::to_string_pretty(result)
            .unwrap_or_else(|err| format!("{{\"error\": \"{err}\"}}")),
        _ => match result {
            LockResult::Granted => format!("Lock GRANTED: {agent_name} -> {target_name}\n"),
            LockResult::Denied { conflicts } => {
                let mut output = String::new();
                let _ = writeln!(output, "Lock DENIED: {agent_name} -> {target_name}");
                let _ = writeln!(output, "Conflicts:");
                for conflict in conflicts {
                    let _ = writeln!(output, "  - {}", conflict.reason);
                }
                output
            }
        },
    }
}

/// Format a list of `LockEntry` items for CLI output.
///
/// Supported formats: `"json"`, `"text"`.
/// Unknown formats fall back to `"text"`.
///
/// **Text format (non-empty)**:
/// ```text
/// Active Locks (N)
/// ================
///   [Write]  alice -> processRequest
///   [Read]   bob   -> validate
/// ```
///
/// **Text format (empty)**:
/// ```text
/// Active Locks (0)
/// ================
///   (none)
/// ```
///
/// **JSON format**: Pretty-printed `Vec<LockEntry>` via serde_json.
///
/// Use `std::fmt::Write` for building the text output, consistent
/// with the existing formatters in this module.
/// Format the kind with `{:?}` (Debug formatting, e.g. `Write`, `Read`).
pub fn format_locks(entries: &[LockEntry], format: &str) -> String {
    match format {
        "json" => serde_json::to_string_pretty(entries)
            .unwrap_or_else(|err| format!("{{\"error\": \"{err}\"}}")),
        _ => {
            let mut output = String::new();
            let _ = writeln!(output, "Active Locks ({})", entries.len());
            let _ = writeln!(output, "================");
            if entries.is_empty() {
                let _ = writeln!(output, "  (none)");
            } else {
                for entry in entries {
                    let _ = writeln!(
                        output,
                        "  [{:?}] {} -> {}",
                        entry.kind, entry.agent_name, entry.target_name
                    );
                }
            }
            output
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Phase 2: Validation output
// ─────────────────────────────────────────────────────────────────────────────

/// Format a `ValidationReport` for CLI output.
///
/// Supported formats: `"json"`, `"text"`.
/// Unknown formats fall back to `"text"`.
///
/// **Text format (clean)**:
/// ```text
/// Validation: alice (3 units checked)
/// ====================================
/// Errors:   0
/// Warnings: 0
///
/// All checks passed.
/// ```
///
/// **Text format (issues)**:
/// ```text
/// Validation: alice (3 units checked)
/// ====================================
/// Errors:   1
/// Warnings: 1
///
/// [ERROR] UnlockedModification: modified `validate` without a Write lock
///   Suggestion: run `nex lock alice validate write` first
///
/// [WARNING] StaleCallers: `processRequest` may be using old signature of `validate`
///   Suggestion: update `processRequest` to match new signature of `validate`
///
/// Exit code: 1
/// ```
///
/// **JSON format**: Pretty-printed `ValidationReport` via serde_json.
///
/// Use `std::fmt::Write` for building text output.
/// Format issue severity with the existing `severity_label()` helper (uppercase).
/// Format the `ValidationKind` variant name with `validation_kind_label()`.
pub fn format_validation_report(report: &ValidationReport, format: &str) -> String {
    match format {
        "json" => serde_json::to_string_pretty(report)
            .unwrap_or_else(|err| format!("{{\"error\": \"{err}\"}}")),
        _ => {
            let mut output = String::new();
            let _ = writeln!(
                output,
                "Validation: {} ({} units checked)",
                report.agent_name, report.units_checked
            );
            let _ = writeln!(output, "====================================");
            let _ = writeln!(output, "Errors:   {}", report.error_count());
            let _ = writeln!(output, "Warnings: {}", report.warning_count());

            if report.issues.is_empty() {
                let _ = writeln!(output);
                let _ = writeln!(output, "All checks passed.");
            } else {
                let _ = writeln!(output);
                for issue in &report.issues {
                    let _ = writeln!(
                        output,
                        "[{}] {}: {}",
                        severity_label(issue.severity),
                        validation_kind_label(&issue.kind),
                        issue.description
                    );
                    if let Some(suggestion) = &issue.suggestion {
                        let _ = writeln!(output, "  Suggestion: {suggestion}");
                    }
                    let _ = writeln!(output);
                }
                let _ = writeln!(output, "Exit code: {}", report.exit_code());
            }

            output
        }
    }
}

fn validation_kind_label(kind: &nex_core::ValidationKind) -> &'static str {
    match kind {
        nex_core::ValidationKind::UnlockedModification { .. } => "UnlockedModification",
        nex_core::ValidationKind::UnlockedDeletion { .. } => "UnlockedDeletion",
        nex_core::ValidationKind::BrokenReference { .. } => "BrokenReference",
        nex_core::ValidationKind::StaleCallers { .. } => "StaleCallers",
    }
}

// Phase 3: Event log output

pub fn format_event_log(events: &[SemanticEvent], format: &str) -> String {
    match format {
        "json" => serde_json::to_string_pretty(events)
            .unwrap_or_else(|err| format!("{{\"error\": \"{err}\"}}")),
        _ => {
            let mut output = String::new();
            let _ = writeln!(output, "Semantic Event Log ({})", events.len());
            let _ = writeln!(output, "======================");

            if events.is_empty() {
                let _ = writeln!(output, "  (none)");
            } else {
                for event in events {
                    let _ = writeln!(
                        output,
                        "- [{}] {}",
                        event.timestamp.to_rfc3339(),
                        event.description
                    );
                    let _ = writeln!(output, "  Intent: {}", event.intent_id);
                    let _ = writeln!(output, "  Agent: {}", event.agent_id);
                    let _ = writeln!(output, "  Mutations: {}", event.mutations.len());
                    if !event.tags.is_empty() {
                        let _ = writeln!(output, "  Tags: {}", event.tags.join(", "));
                    }
                }
            }

            output
        }
    }
}

pub fn format_rollback_outcome(outcome: &RollbackOutcome, format: &str) -> String {
    match format {
        "json" => serde_json::to_string_pretty(outcome)
            .unwrap_or_else(|err| format!("{{\"error\": \"{err}\"}}")),
        _ => {
            let mut output = String::new();
            if let Some(event) = &outcome.rollback_event {
                let _ = writeln!(output, "Rollback APPLIED: {}", outcome.original_intent_id);
                let _ = writeln!(output, "Event: {}", event.id);
                let _ = writeln!(output, "Mutations: {}", event.mutations.len());
            } else {
                let _ = writeln!(output, "Rollback BLOCKED: {}", outcome.original_intent_id);
                let _ = writeln!(output, "Conflicts:");
                for conflict in &outcome.conflicts {
                    let _ = writeln!(output, "  - {}", conflict.reason);
                }
            }
            output
        }
    }
}

pub fn format_replay_state(units: &[SemanticUnit], format: &str) -> String {
    match format {
        "json" => serde_json::to_string_pretty(units)
            .unwrap_or_else(|err| format!("{{\"error\": \"{err}\"}}")),
        _ => {
            let mut output = String::new();
            let _ = writeln!(output, "Replayed State ({})", units.len());
            let _ = writeln!(output, "==================");

            if units.is_empty() {
                let _ = writeln!(output, "  (none)");
            } else {
                for unit in units {
                    let _ = writeln!(
                        output,
                        "  - {:?} {} ({})",
                        unit.kind,
                        unit.qualified_name,
                        unit.file_path.display()
                    );
                }
            }

            output
        }
    }
}
