# Prompt 007 — Validation Engine + CLI Validate Pipeline

## Context

You are implementing the **Phase 2 pre-commit validation engine** for Nexum Graph.
The validation engine checks that committed changes are covered by semantic locks
and that reference integrity is maintained (no broken references, no stale callers).

This prompt adds:
1. **Validation engine** — `ValidationEngine::validate()` in `nex-validate`
2. **CLI pipeline** — `run_validate` in `nex-cli/coordination_pipeline.rs`
3. **Output formatting** — `format_validation_report` in `nex-cli/output.rs`
4. **Integration tests** — `nex-cli/tests/validation_cli.rs`

Phase 0 (semantic diff), Phase 1 (conflict detection), and Phase 2 coordination
(lock/unlock/locks) are complete. The CLI commands and engine are working and tested.

## Crate Layout

```
crates/nex-validate/
  src/
    validator.rs  — YOUR IMPLEMENTATION (fill in 7 todo!() stubs)
    lib.rs        — do not touch

crates/nex-cli/
  src/
    coordination_pipeline.rs  — fill in 1 todo!() stub (run_validate)
    output.rs                 — fill in 2 todo!() stubs (format_validation_report, validation_kind_label)
    cli.rs                    — do not touch (Validate command pre-wired)
    main.rs                   — do not touch (dispatch pre-wired)
    pipeline.rs               — do not touch (but you CALL its functions)
    lib.rs                    — do not touch
```

## Files You Must Implement

### 1. `crates/nex-validate/src/validator.rs` — 7 functions

Fill in all `todo!()` bodies. Every function already has complete doc comments
with pseudocode showing the exact algorithm.

#### `ValidationEngine::validate(graph_before, graph_after, agent_name, agent_id, locks) -> ValidationReport`

Main orchestrator:
```rust
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

    // Sort issues by severity (errors first, then warnings, then info).
    issues.sort_by_key(|issue| severity_rank(issue.severity));

    let units_checked = diff.modified.len() + diff.removed.len() + diff.added.len();

    ValidationReport {
        issues,
        agent_name: agent_name.to_string(),
        units_checked,
    }
}
```

#### `check_modification_locks(diff, agent_name, agent_id, locks, issues)`

For each modified unit, verify the agent holds a Write lock on `modified.before.id`:
```rust
for modified in &diff.modified {
    let has_lock = locks.iter().any(|l| {
        l.agent_id == agent_id
            && l.target == modified.before.id
            && matches!(l.kind, IntentKind::Write)
    });
    if !has_lock {
        issues.push(ValidationIssue {
            kind: ValidationKind::UnlockedModification { unit: modified.before.id },
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
```

#### `check_deletion_locks(diff, agent_name, agent_id, locks, issues)`

For each removed unit, verify the agent holds a Delete lock on `removed.id`:
```rust
for removed in &diff.removed {
    let has_lock = locks.iter().any(|l| {
        l.agent_id == agent_id
            && l.target == removed.id
            && matches!(l.kind, IntentKind::Delete)
    });
    if !has_lock {
        issues.push(ValidationIssue {
            kind: ValidationKind::UnlockedDeletion { unit: removed.id },
            severity: Severity::Error,
            unit_name: removed.qualified_name.clone(),
            description: format!(
                "deleted `{}` without a Delete lock",
                removed.qualified_name
            ),
            suggestion: Some(format!(
                "run `nex lock {} {} delete` first",
                agent_name, removed.qualified_name
            )),
        });
    }
}
```

#### `check_broken_references(diff, graph_before, graph_after, issues)`

For each removed unit, find callers in `graph_before` that still exist in
`graph_after` and were NOT modified (modified callers get benefit of the doubt):
```rust
let modified_names: HashSet<&str> = diff.modified.iter()
    .map(|m| m.after.qualified_name.as_str())
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
```

#### `check_stale_callers(diff, graph_after, issues)`

For each modified unit with `SignatureChanged`, find callers in `graph_after`
that were NOT also modified:
```rust
let modified_names: HashSet<&str> = diff.modified.iter()
    .map(|m| m.after.qualified_name.as_str())
    .collect();

for modified in &diff.modified {
    if !modified.changes.contains(&ChangeKind::SignatureChanged) {
        continue;
    }
    let Some(after_unit) = find_unit_by_name(graph_after, &modified.after.qualified_name) else {
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
```

#### `find_unit_by_name(graph, name) -> Option<&SemanticUnit>`

Simple O(n) scan over `graph.units()`:
```rust
graph.units().into_iter().find(|u| u.qualified_name == name)
```

#### `severity_rank(severity) -> u8`

Map severity to sort order (errors first):
```rust
match severity {
    Severity::Error => 0,
    Severity::Warning => 1,
    Severity::Info => 2,
}
```

### 2. `crates/nex-cli/src/coordination_pipeline.rs` — 1 function

#### `run_validate(repo_path, agent_name, base_ref) -> CodexResult<ValidationReport>`

```rust
pub fn run_validate(
    repo_path: &Path,
    agent_name: &str,
    base_ref: &str,
) -> CodexResult<nex_core::ValidationReport> {
    let repo = git2::Repository::open(repo_path)
        .map_err(|err| CodexError::Git(err.to_string()))?;
    let extractors: Vec<Box<dyn nex_parse::SemanticExtractor>> =
        vec![nex_parse::typescript_extractor()];

    let files_before = crate::pipeline::collect_files_at_ref(&repo, base_ref, &extractors)?;
    let files_after = crate::pipeline::collect_files_at_ref(&repo, "HEAD", &extractors)?;
    let graph_before = crate::pipeline::build_graph(&files_before, &extractors)?;
    let graph_after = crate::pipeline::build_graph(&files_after, &extractors)?;

    let agent_id = agent_name_to_id(agent_name);
    let entries = load_locks(repo_path)?;
    let semantic_locks: Vec<nex_core::SemanticLock> = entries
        .iter()
        .map(|entry| nex_core::SemanticLock {
            agent_id: entry.agent_id,
            target: entry.target,
            kind: entry.kind,
        })
        .collect();

    Ok(nex_validate::ValidationEngine::validate(
        &graph_before,
        &graph_after,
        agent_name,
        agent_id,
        &semantic_locks,
    ))
}
```

You will need to add these imports at the top of the function body or
at the file-level `use` block:
```rust
use nex_core::CodexError;       // already imported
use nex_parse::SemanticExtractor; // already available via nex_parse
```

### 3. `crates/nex-cli/src/output.rs` — 2 functions

#### `format_validation_report(report, format) -> String`

```rust
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
```

#### `validation_kind_label(kind) -> &'static str`

```rust
fn validation_kind_label(kind: &nex_core::ValidationKind) -> &'static str {
    match kind {
        nex_core::ValidationKind::UnlockedModification { .. } => "UnlockedModification",
        nex_core::ValidationKind::UnlockedDeletion { .. } => "UnlockedDeletion",
        nex_core::ValidationKind::BrokenReference { .. } => "BrokenReference",
        nex_core::ValidationKind::StaleCallers { .. } => "StaleCallers",
    }
}
```

## Imports You Will Need

### In `validator.rs` (already present in the stub)
```rust
use nex_core::{
    AgentId, ChangeKind, IntentKind, SemanticDiff, SemanticLock, SemanticUnit, Severity,
    ValidationIssue, ValidationKind, ValidationReport,
};
use nex_graph::CodeGraph;
use std::collections::HashSet;
```

### In `coordination_pipeline.rs` (already present)
No new imports needed. `nex_core::CodexError`, `nex_core::SemanticLock`,
and `nex_parse::SemanticExtractor` are already accessible. Add `nex_validate`
usage inline (it's already in `Cargo.toml` dependencies).

### In `output.rs` (already present)
```rust
use nex_core::ValidationReport; // already imported
```

## Type Definitions (FROZEN — Do NOT Modify)

All types from `nex-core` are frozen. The relevant ones for this prompt:

```rust
// nex-core/src/validation.rs
pub struct ValidationIssue {
    pub kind: ValidationKind,
    pub severity: Severity,
    pub unit_name: String,
    pub description: String,
    pub suggestion: Option<String>,
}

pub enum ValidationKind {
    UnlockedModification { unit: SemanticId },
    UnlockedDeletion { unit: SemanticId },
    BrokenReference { deleted: SemanticId, referencing: SemanticId },
    StaleCallers { function: SemanticId, caller: SemanticId },
}

pub struct ValidationReport {
    pub issues: Vec<ValidationIssue>,
    pub agent_name: String,
    pub units_checked: usize,
}

// Existing from conflict.rs:
pub enum Severity { Info, Warning, Error }

// Existing from coordination.rs:
pub struct SemanticLock { pub agent_id: AgentId, pub target: SemanticId, pub kind: IntentKind }
pub enum IntentKind { Read, Write, Delete }
pub type AgentId = [u8; 16];

// Existing from semantic.rs:
pub struct SemanticDiff { pub added, pub removed, pub modified, pub moved }
pub struct ModifiedUnit { pub before: SemanticUnit, pub after: SemanticUnit, pub changes: Vec<ChangeKind> }
pub enum ChangeKind { SignatureChanged, BodyChanged, DocChanged }
```

## Test File

Create `crates/nex-cli/tests/validation_cli.rs` with the following tests.
These are acceptance criteria — Codex must write code that passes them all.

```rust
//! Integration tests for the pre-commit validation pipeline.
//!
//! These tests exercise the Prompt 007 validation workflow:
//! - ValidationEngine (nex-validate) checks lock coverage and reference integrity
//! - CLI pipeline (run_validate) wires git -> parse -> graph -> validate
//! - Output formatting (format_validation_report) for text and JSON
//!
//! IMPORTANT: These are the acceptance criteria for Prompt 007.
//! Do NOT modify these tests — Codex must write code that passes them.

use nex_cli::coordination_pipeline::{agent_name_to_id, run_lock, run_validate};
use nex_cli::output::format_validation_report;
use std::path::Path;

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Create a temporary git repo with user config.
fn init_temp_repo() -> (tempfile::TempDir, git2::Repository) {
    let dir = tempfile::tempdir().expect("create tempdir");
    let repo = git2::Repository::init(dir.path()).expect("init repo");

    let mut config = repo.config().expect("get config");
    config.set_str("user.name", "Test").expect("set name");
    config
        .set_str("user.email", "test@example.com")
        .expect("set email");

    (dir, repo)
}

/// Write a file into the repo working directory and stage it.
fn write_and_stage(repo: &git2::Repository, relative_path: &str, content: &str) {
    let full_path = repo.workdir().unwrap().join(relative_path);
    if let Some(parent) = full_path.parent() {
        std::fs::create_dir_all(parent).expect("create dirs");
    }
    std::fs::write(&full_path, content).expect("write file");

    let mut index = repo.index().expect("get index");
    index
        .add_path(Path::new(relative_path))
        .expect("add to index");
    index.write().expect("write index");
}

/// Create a commit with the current index state.
fn commit(repo: &git2::Repository, msg: &str) {
    let mut index = repo.index().expect("get index");
    let tree_oid = index.write_tree().expect("write tree");
    let tree = repo.find_tree(tree_oid).expect("find tree");
    let sig = repo.signature().expect("sig");
    let parents: Vec<git2::Commit> = match repo.head() {
        Ok(head) => vec![head.peel_to_commit().expect("peel")],
        Err(_) => vec![],
    };
    let refs: Vec<&git2::Commit> = parents.iter().collect();
    repo.commit(Some("HEAD"), &sig, &sig, msg, &tree, &refs)
        .expect("commit");
}

/// Set up a two-commit repo:
///
/// Commit 1 (HEAD~1):
///   handler.ts:
///     function validate(input: string): boolean { return input.length > 0; }
///     function processRequest(req: string): void { validate(req); }
///   utils.ts:
///     function formatDate(d: Date): string { return d.toISOString(); }
///
/// Commit 2 (HEAD):
///   handler.ts:
///     function validate(input: string, strict: boolean): boolean { return strict ? input.trim().length > 0 : input.length > 0; }
///     function processRequest(req: string): void { validate(req); }
///   utils.ts:
///     function formatDate(d: Date): string { return d.toISOString(); }
///
/// This means: validate's signature changed (added `strict` param),
/// but processRequest was NOT updated.
fn setup_signature_change_repo() -> (tempfile::TempDir, git2::Repository) {
    let (dir, repo) = init_temp_repo();

    // Commit 1: base state
    write_and_stage(
        &repo,
        "handler.ts",
        r#"function validate(input: string): boolean { return input.length > 0; }
function processRequest(req: string): void { validate(req); }
"#,
    );
    write_and_stage(
        &repo,
        "utils.ts",
        "function formatDate(d: Date): string { return d.toISOString(); }",
    );
    commit(&repo, "initial");

    // Commit 2: modify validate signature
    write_and_stage(
        &repo,
        "handler.ts",
        r#"function validate(input: string, strict: boolean): boolean { return strict ? input.trim().length > 0 : input.length > 0; }
function processRequest(req: string): void { validate(req); }
"#,
    );
    commit(&repo, "change validate signature");

    (dir, repo)
}

/// Set up a two-commit repo where a function is deleted:
///
/// Commit 1 (HEAD~1):
///   handler.ts:
///     function validate(input: string): boolean { return input.length > 0; }
///     function processRequest(req: string): void { validate(req); }
///
/// Commit 2 (HEAD):
///   handler.ts:
///     function processRequest(req: string): void { console.log(req); }
///
/// This means: validate was deleted, but processRequest still exists
/// (and was modified to remove the call, so it should NOT be flagged
/// as a broken reference — it was updated).
fn setup_deletion_with_updated_caller() -> (tempfile::TempDir, git2::Repository) {
    let (dir, repo) = init_temp_repo();

    write_and_stage(
        &repo,
        "handler.ts",
        r#"function validate(input: string): boolean { return input.length > 0; }
function processRequest(req: string): void { validate(req); }
"#,
    );
    commit(&repo, "initial");

    write_and_stage(
        &repo,
        "handler.ts",
        "function processRequest(req: string): void { console.log(req); }\n",
    );
    commit(&repo, "delete validate, update caller");

    (dir, repo)
}

/// Set up a two-commit repo where a function is deleted but its caller
/// is NOT updated:
///
/// Commit 1 (HEAD~1):
///   shared.ts:
///     function helperFn(): void { }
///   main.ts:
///     function entryPoint(): void { helperFn(); }
///
/// Commit 2 (HEAD):
///   shared.ts is DELETED (empty — no functions)
///   main.ts unchanged:
///     function entryPoint(): void { helperFn(); }
///
/// This means: helperFn was deleted, and entryPoint still references it
/// without being modified — should produce a BrokenReference error.
fn setup_deletion_broken_ref() -> (tempfile::TempDir, git2::Repository) {
    let (dir, repo) = init_temp_repo();

    write_and_stage(
        &repo,
        "shared.ts",
        "function helperFn(): void { }\n",
    );
    write_and_stage(
        &repo,
        "main.ts",
        "function entryPoint(): void { helperFn(); }\n",
    );
    commit(&repo, "initial");

    // Delete shared.ts by replacing with empty file (no functions).
    write_and_stage(
        &repo,
        "shared.ts",
        "// empty\n",
    );
    commit(&repo, "delete helperFn");

    (dir, repo)
}

/// Set up a simple two-commit repo with only a body change (no signature change):
///
/// Commit 1: function greet(name: string): string { return "Hello " + name; }
/// Commit 2: function greet(name: string): string { return "Hi " + name; }
fn setup_body_change_repo() -> (tempfile::TempDir, git2::Repository) {
    let (dir, repo) = init_temp_repo();

    write_and_stage(
        &repo,
        "greet.ts",
        "function greet(name: string): string { return \"Hello \" + name; }\n",
    );
    commit(&repo, "initial");

    write_and_stage(
        &repo,
        "greet.ts",
        "function greet(name: string): string { return \"Hi \" + name; }\n",
    );
    commit(&repo, "change body");

    (dir, repo)
}

// ─────────────────────────────────────────────────────────────────────────────
// Section 1: Validation with proper locks (clean results)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn validate_clean_with_write_lock() {
    let (_dir, repo) = setup_body_change_repo();
    let repo_path = repo.workdir().unwrap();

    // Lock the modified function.
    run_lock(repo_path, "alice", "greet", "write").unwrap();

    let report = run_validate(repo_path, "alice", "HEAD~1").unwrap();
    assert_eq!(report.error_count(), 0, "no errors when lock is held");
    assert_eq!(report.warning_count(), 0, "no warnings for body-only change");
    assert!(report.units_checked > 0, "should have checked some units");
}

#[test]
fn validate_clean_with_delete_lock() {
    let (_dir, repo) = setup_deletion_with_updated_caller();
    let repo_path = repo.workdir().unwrap();

    // Lock the deleted function.
    // Note: we need to lock against the base graph, so we lock before
    // the deletion happens. Since run_lock reads HEAD, and validate
    // was deleted in HEAD, we need a different approach.
    //
    // Actually, run_lock looks up targets in HEAD — and validate doesn't
    // exist in HEAD. So we must lock using the base ref's graph.
    //
    // For this test, we manually create the lock file with the correct IDs.
    // This tests the ValidationEngine directly, not the lock acquisition flow.
    use nex_cli::coordination_pipeline::{save_locks, LockEntry};
    use nex_core::IntentKind;

    // Build graph at HEAD~1 to get validate's SemanticId.
    let agent_id = agent_name_to_id("alice");
    let extractors: Vec<Box<dyn nex_parse::SemanticExtractor>> =
        vec![nex_parse::typescript_extractor()];
    let git_repo = git2::Repository::open(repo_path).unwrap();
    let files = nex_cli::pipeline::collect_files_at_ref(&git_repo, "HEAD~1", &extractors).unwrap();
    let graph = nex_cli::pipeline::build_graph(&files, &extractors).unwrap();
    let validate_unit = graph.units().into_iter()
        .find(|u| u.name == "validate")
        .unwrap();

    // Also lock processRequest with Write (since it was modified too).
    let process_unit = graph.units().into_iter()
        .find(|u| u.name == "processRequest")
        .unwrap();

    let entries = vec![
        LockEntry {
            agent_name: "alice".to_string(),
            agent_id,
            target_name: "validate".to_string(),
            target: validate_unit.id,
            kind: IntentKind::Delete,
        },
        LockEntry {
            agent_name: "alice".to_string(),
            agent_id,
            target_name: "processRequest".to_string(),
            target: process_unit.id,
            kind: IntentKind::Write,
        },
    ];
    save_locks(repo_path, &entries).unwrap();

    let report = run_validate(repo_path, "alice", "HEAD~1").unwrap();
    assert_eq!(report.error_count(), 0, "no errors with proper delete+write locks");
}

// ─────────────────────────────────────────────────────────────────────────────
// Section 2: Unlocked modification detection
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn validate_detects_unlocked_modification() {
    let (_dir, repo) = setup_body_change_repo();
    let repo_path = repo.workdir().unwrap();

    // No locks held — should detect unlocked modification.
    let report = run_validate(repo_path, "alice", "HEAD~1").unwrap();
    assert!(report.error_count() > 0, "should detect unlocked modification");

    let issue = &report.issues[0];
    assert!(
        issue.description.contains("without a Write lock"),
        "description should mention Write lock: {}",
        issue.description
    );
    assert!(
        issue.suggestion.as_ref().unwrap().contains("nex lock"),
        "suggestion should mention nex lock command"
    );
}

#[test]
fn validate_unlocked_modification_is_error_severity() {
    let (_dir, repo) = setup_body_change_repo();
    let repo_path = repo.workdir().unwrap();

    let report = run_validate(repo_path, "alice", "HEAD~1").unwrap();
    let unlocked_mod = report.issues.iter()
        .find(|i| matches!(i.kind, nex_core::ValidationKind::UnlockedModification { .. }));
    assert!(unlocked_mod.is_some(), "should have UnlockedModification issue");
    assert_eq!(unlocked_mod.unwrap().severity, nex_core::Severity::Error);
}

// ─────────────────────────────────────────────────────────────────────────────
// Section 3: Unlocked deletion detection
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn validate_detects_unlocked_deletion() {
    let (_dir, repo) = setup_deletion_with_updated_caller();
    let repo_path = repo.workdir().unwrap();

    // No locks — should detect unlocked deletion.
    let report = run_validate(repo_path, "alice", "HEAD~1").unwrap();

    let deletion_issue = report.issues.iter()
        .find(|i| matches!(i.kind, nex_core::ValidationKind::UnlockedDeletion { .. }));
    assert!(deletion_issue.is_some(), "should detect unlocked deletion");
    assert!(
        deletion_issue.unwrap().description.contains("without a Delete lock"),
        "description should mention Delete lock"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Section 4: Stale caller detection
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn validate_detects_stale_caller_after_signature_change() {
    let (_dir, repo) = setup_signature_change_repo();
    let repo_path = repo.workdir().unwrap();

    // Lock validate with Write so the modification lock check passes.
    run_lock(repo_path, "alice", "validate", "write").unwrap();

    let report = run_validate(repo_path, "alice", "HEAD~1").unwrap();

    let stale = report.issues.iter()
        .find(|i| matches!(i.kind, nex_core::ValidationKind::StaleCallers { .. }));
    assert!(stale.is_some(), "should detect stale caller");
    assert_eq!(stale.unwrap().severity, nex_core::Severity::Warning);
    assert!(
        stale.unwrap().description.contains("processRequest"),
        "stale caller should mention processRequest"
    );
}

#[test]
fn validate_stale_caller_is_warning_not_error() {
    let (_dir, repo) = setup_signature_change_repo();
    let repo_path = repo.workdir().unwrap();

    run_lock(repo_path, "alice", "validate", "write").unwrap();

    let report = run_validate(repo_path, "alice", "HEAD~1").unwrap();
    let stale = report.issues.iter()
        .find(|i| matches!(i.kind, nex_core::ValidationKind::StaleCallers { .. }));
    assert!(stale.is_some());
    assert_eq!(stale.unwrap().severity, nex_core::Severity::Warning);
}

// ─────────────────────────────────────────────────────────────────────────────
// Section 5: Broken reference detection
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn validate_detects_broken_reference() {
    let (_dir, repo) = setup_deletion_broken_ref();
    let repo_path = repo.workdir().unwrap();

    let report = run_validate(repo_path, "alice", "HEAD~1").unwrap();

    let broken = report.issues.iter()
        .find(|i| matches!(i.kind, nex_core::ValidationKind::BrokenReference { .. }));
    assert!(broken.is_some(), "should detect broken reference");
    assert_eq!(broken.unwrap().severity, nex_core::Severity::Error);
    assert!(
        broken.unwrap().description.contains("entryPoint"),
        "should mention the referencing function"
    );
    assert!(
        broken.unwrap().description.contains("helperFn"),
        "should mention the deleted function"
    );
}

#[test]
fn validate_no_broken_ref_when_caller_also_modified() {
    let (_dir, repo) = setup_deletion_with_updated_caller();
    let repo_path = repo.workdir().unwrap();

    let report = run_validate(repo_path, "alice", "HEAD~1").unwrap();

    let broken = report.issues.iter()
        .find(|i| matches!(i.kind, nex_core::ValidationKind::BrokenReference { .. }));
    assert!(broken.is_none(), "should NOT flag broken ref when caller was also modified");
}

// ─────────────────────────────────────────────────────────────────────────────
// Section 6: Issue ordering
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn validate_issues_sorted_errors_before_warnings() {
    let (_dir, repo) = setup_signature_change_repo();
    let repo_path = repo.workdir().unwrap();

    // No locks — should have both errors (unlocked modification) and warnings (stale caller).
    let report = run_validate(repo_path, "alice", "HEAD~1").unwrap();

    if report.issues.len() >= 2 {
        for i in 0..report.issues.len() - 1 {
            assert!(
                report.issues[i].severity >= report.issues[i + 1].severity,
                "issues should be sorted by severity (Error >= Warning >= Info)"
            );
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Section 7: Output formatting
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn format_validation_text_clean() {
    let (_dir, repo) = setup_body_change_repo();
    let repo_path = repo.workdir().unwrap();

    run_lock(repo_path, "alice", "greet", "write").unwrap();

    let report = run_validate(repo_path, "alice", "HEAD~1").unwrap();
    let text = format_validation_report(&report, "text");

    assert!(text.contains("Validation: alice"), "should show agent name");
    assert!(text.contains("All checks passed"), "should say all passed");
}

#[test]
fn format_validation_text_with_issues() {
    let (_dir, repo) = setup_body_change_repo();
    let repo_path = repo.workdir().unwrap();

    let report = run_validate(repo_path, "alice", "HEAD~1").unwrap();
    let text = format_validation_report(&report, "text");

    assert!(text.contains("[ERROR]"), "should contain ERROR label");
    assert!(text.contains("UnlockedModification"), "should show kind label");
    assert!(text.contains("Suggestion:"), "should show suggestion");
    assert!(text.contains("Exit code:"), "should show exit code");
}

#[test]
fn format_validation_json_is_valid() {
    let (_dir, repo) = setup_body_change_repo();
    let repo_path = repo.workdir().unwrap();

    let report = run_validate(repo_path, "alice", "HEAD~1").unwrap();
    let json = format_validation_report(&report, "json");

    let parsed: serde_json::Value = serde_json::from_str(&json).expect("should be valid JSON");
    assert!(parsed.get("issues").is_some(), "JSON should have issues field");
    assert!(parsed.get("agent_name").is_some(), "JSON should have agent_name field");
}

// ─────────────────────────────────────────────────────────────────────────────
// Section 8: Units checked count
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn validate_units_checked_includes_all_change_types() {
    let (_dir, repo) = setup_deletion_with_updated_caller();
    let repo_path = repo.workdir().unwrap();

    let report = run_validate(repo_path, "alice", "HEAD~1").unwrap();
    // validate was removed (1), processRequest was modified (1) = at least 2
    assert!(
        report.units_checked >= 2,
        "units_checked should count modified + removed + added, got {}",
        report.units_checked
    );
}
```

## Acceptance Criteria

All existing tests must continue to pass:
```
cargo test -p nex-parse         # 16 tests
cargo test -p nex-graph         # 20 tests
cargo test -p nex-coord         # 28 tests
cargo test -p nex-cli           # 29 existing + 15 new validation tests
```

New validation tests in `nex-cli`:
```
test validate_clean_with_write_lock ... ok
test validate_clean_with_delete_lock ... ok
test validate_detects_unlocked_modification ... ok
test validate_unlocked_modification_is_error_severity ... ok
test validate_detects_unlocked_deletion ... ok
test validate_detects_stale_caller_after_signature_change ... ok
test validate_stale_caller_is_warning_not_error ... ok
test validate_detects_broken_reference ... ok
test validate_no_broken_ref_when_caller_also_modified ... ok
test validate_issues_sorted_errors_before_warnings ... ok
test format_validation_text_clean ... ok
test format_validation_text_with_issues ... ok
test format_validation_json_is_valid ... ok
test validate_units_checked_includes_all_change_types ... ok
```

Additionally:
```
cargo clippy -p nex-validate -p nex-cli -- -D warnings    # No warnings
cargo fmt -p nex-validate -p nex-cli --check              # Formatted
```

## Constraints

1. Do NOT modify any file in `nex-core/` — types are frozen
2. Do NOT modify `nex-validate/src/lib.rs`
3. Do NOT modify `nex-coord/` — Phase 1 + 2 engine are complete
4. Do NOT modify `nex-cli/src/cli.rs`, `nex-cli/src/main.rs`, or `nex-cli/src/lib.rs`
5. Do NOT modify `nex-cli/src/pipeline.rs`
6. Do NOT modify existing test files (`diff_pipeline.rs`, `coordination_cli.rs`)
7. Do NOT add new dependencies to any `Cargo.toml`
8. Do NOT modify existing functions in `output.rs` — only fill the new stubs
9. The ONLY files you modify are:
   - `crates/nex-validate/src/validator.rs` (fill 7 `todo!()` bodies)
   - `crates/nex-cli/src/coordination_pipeline.rs` (fill 1 `todo!()` body: `run_validate`)
   - `crates/nex-cli/src/output.rs` (fill 2 `todo!()` bodies)
10. The ONLY file you create is:
    - `crates/nex-cli/tests/validation_cli.rs`
11. Map errors as follows:
    - `git2` errors → `CodexError::Git(e.to_string())`
    - `std::io` errors → use `?` (From impl exists)
    - `serde_json` errors → use `?` (From impl exists)
    - Coordination logic errors → `CodexError::Coordination(String)`

## File Checklist

| File | Action |
|------|--------|
| `crates/nex-validate/src/validator.rs` | Fill 7 `todo!()` bodies |
| `crates/nex-cli/src/coordination_pipeline.rs` | Fill 1 `todo!()` body (`run_validate`) |
| `crates/nex-cli/src/output.rs` | Fill 2 `todo!()` bodies (`format_validation_report`, `validation_kind_label`) |
| `crates/nex-cli/tests/validation_cli.rs` | CREATE — 15 integration tests |
| Everything else | DO NOT MODIFY |
