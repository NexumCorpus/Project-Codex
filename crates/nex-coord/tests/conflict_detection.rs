//! Integration tests for the nex-coord conflict detection engine.
//!
//! These tests create temporary git repositories with two diverging branches,
//! then run `ConflictDetector::detect()` and assert on the `ConflictReport`.
//!
//! Test categories:
//! 1. Clean merges (no conflicts)
//! 2. Single-conflict scenarios (one type at a time)
//! 3. Multi-conflict scenarios
//! 4. Report utility methods (exit codes, counts)
//!
//! IMPORTANT: These tests exercise the full Phase 1 stack:
//! git2 → nex-parse → nex-graph → SemanticDiff → cross-reference → ConflictReport

use nex_coord::ConflictDetector;
use nex_core::{ConflictKind, Severity};
use std::path::Path;

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Create a temporary git repo with author config.
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

/// Write a file into the repo working directory, stage it.
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

/// Remove a file from the working directory and stage the removal.
fn remove_and_stage(repo: &git2::Repository, relative_path: &str) {
    let full_path = repo.workdir().unwrap().join(relative_path);
    if full_path.exists() {
        std::fs::remove_file(&full_path).expect("remove file");
    }
    let mut index = repo.index().expect("get index");
    index
        .remove_path(Path::new(relative_path))
        .expect("remove from index");
    index.write().expect("write index");
}

/// Create a commit on current HEAD and return its OID.
fn commit(repo: &git2::Repository, message: &str) -> git2::Oid {
    let mut index = repo.index().expect("get index");
    let tree_oid = index.write_tree().expect("write tree");
    let tree = repo.find_tree(tree_oid).expect("find tree");
    let sig = repo.signature().expect("get signature");

    let parents: Vec<git2::Commit> = match repo.head() {
        Ok(head) => vec![head.peel_to_commit().expect("peel to commit")],
        Err(_) => vec![],
    };
    let parent_refs: Vec<&git2::Commit> = parents.iter().collect();

    repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &parent_refs)
        .expect("create commit")
}

/// Create a branch at the given commit and switch HEAD + working directory to it.
fn create_branch_at(repo: &git2::Repository, name: &str, oid: git2::Oid) {
    let target_commit = repo.find_commit(oid).expect("find commit");
    repo.branch(name, &target_commit, false)
        .expect("create branch");
    repo.set_head(&format!("refs/heads/{name}"))
        .expect("set HEAD");
    repo.checkout_head(Some(
        git2::build::CheckoutBuilder::new()
            .force()
            .remove_untracked(true),
    ))
    .expect("checkout HEAD");
}

// ─────────────────────────────────────────────────────────────────────────────
// 1. Clean merge (no conflicts)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn clean_merge_no_conflicts() {
    let (_dir, repo) = init_temp_repo();

    // Base: two independent files
    write_and_stage(
        &repo,
        "src/lib.ts",
        "export function helper(): number { return 42; }",
    );
    write_and_stage(&repo, "src/main.ts", "function main(): void {}");
    let base = commit(&repo, "base");

    // Branch A: adds a new function in a new file (no overlap with B)
    create_branch_at(&repo, "branch_a", base);
    write_and_stage(
        &repo,
        "src/featureA.ts",
        "export function featureA(): void {}",
    );
    commit(&repo, "add featureA");

    // Branch B: adds a different function in a different file
    create_branch_at(&repo, "branch_b", base);
    write_and_stage(
        &repo,
        "src/featureB.ts",
        "export function featureB(): void {}",
    );
    commit(&repo, "add featureB");

    let report = ConflictDetector::detect(repo.workdir().unwrap(), "branch_a", "branch_b")
        .expect("detect conflicts");

    assert_eq!(report.conflicts.len(), 0, "expected no conflicts");
    assert_eq!(report.exit_code(), 0);
}

// ─────────────────────────────────────────────────────────────────────────────
// 2. Single-conflict scenarios
// ─────────────────────────────────────────────────────────────────────────────

/// Branch A deletes a function that branch B's code graph still depends on.
#[test]
fn detects_deleted_dependency() {
    let (_dir, repo) = init_temp_repo();

    // Base: shared function + consumer that calls it
    write_and_stage(
        &repo,
        "src/shared.ts",
        "export function shared(): number { return 42; }",
    );
    write_and_stage(
        &repo,
        "src/consumer.ts",
        "function useShared(): number { return shared(); }",
    );
    let base = commit(&repo, "base");

    // Branch A: deletes the shared function entirely
    create_branch_at(&repo, "branch_a", base);
    remove_and_stage(&repo, "src/shared.ts");
    write_and_stage(
        &repo,
        "src/replacement.ts",
        "export function newApi(): number { return 99; }",
    );
    commit(&repo, "delete shared, add newApi");

    // Branch B: keeps shared unchanged, adds another consumer
    create_branch_at(&repo, "branch_b", base);
    write_and_stage(
        &repo,
        "src/more.ts",
        "function moreStuff(): number { return shared() + 1; }",
    );
    commit(&repo, "add more consumer of shared");

    let report = ConflictDetector::detect(repo.workdir().unwrap(), "branch_a", "branch_b")
        .expect("detect conflicts");

    // Branch A deleted `shared`, but branch B's graph still depends on it
    let deleted_deps: Vec<_> = report
        .conflicts
        .iter()
        .filter(|c| matches!(c.kind, ConflictKind::DeletedDependency { .. }))
        .collect();
    assert!(
        !deleted_deps.is_empty(),
        "expected at least one DeletedDependency conflict"
    );
    assert!(
        deleted_deps.iter().all(|c| c.severity == Severity::Error),
        "DeletedDependency should be Error severity"
    );
    assert_eq!(report.exit_code(), 1, "errors → exit code 1");
}

/// Branch A changes a function's signature while branch B still calls it
/// with the old signature.
#[test]
fn detects_signature_mismatch() {
    let (_dir, repo) = init_temp_repo();

    // Base: function with specific signature + caller
    write_and_stage(
        &repo,
        "src/math.ts",
        "export function calculate(a: number, b: number): number { return a + b; }",
    );
    write_and_stage(
        &repo,
        "src/app.ts",
        "function run(): number { return calculate(1, 2); }",
    );
    let base = commit(&repo, "base");

    // Branch A: changes calculate's signature (adds a third parameter)
    create_branch_at(&repo, "branch_a", base);
    write_and_stage(
        &repo,
        "src/math.ts",
        "export function calculate(a: number, b: number, precision: number): number { return +(a + b).toFixed(precision); }",
    );
    commit(&repo, "add precision param to calculate");

    // Branch B: no change to math.ts, but adds another caller of calculate
    create_branch_at(&repo, "branch_b", base);
    write_and_stage(
        &repo,
        "src/extra.ts",
        "function extra(): number { return calculate(10, 20); }",
    );
    commit(&repo, "add extra caller of calculate");

    let report = ConflictDetector::detect(repo.workdir().unwrap(), "branch_a", "branch_b")
        .expect("detect conflicts");

    let sig_mismatches: Vec<_> = report
        .conflicts
        .iter()
        .filter(|c| matches!(c.kind, ConflictKind::SignatureMismatch { .. }))
        .collect();
    assert!(
        !sig_mismatches.is_empty(),
        "expected at least one SignatureMismatch conflict"
    );
    assert!(
        sig_mismatches.iter().all(|c| c.severity == Severity::Error),
        "SignatureMismatch should be Error severity"
    );
}

/// Both branches modify the body of the same function (but leave signature intact).
/// This is a Warning, not an Error, because the merge _might_ be reconcilable.
#[test]
fn detects_concurrent_body_edit() {
    let (_dir, repo) = init_temp_repo();

    // Base: a shared function
    write_and_stage(
        &repo,
        "src/logic.ts",
        "export function process(input: string): string { return input.trim(); }",
    );
    let base = commit(&repo, "base");

    // Branch A: changes body (different implementation)
    create_branch_at(&repo, "branch_a", base);
    write_and_stage(
        &repo,
        "src/logic.ts",
        "export function process(input: string): string { return input.trim().toLowerCase(); }",
    );
    commit(&repo, "add toLowerCase");

    // Branch B: also changes body (different implementation)
    create_branch_at(&repo, "branch_b", base);
    write_and_stage(
        &repo,
        "src/logic.ts",
        "export function process(input: string): string { return input.trim().toUpperCase(); }",
    );
    commit(&repo, "add toUpperCase");

    let report = ConflictDetector::detect(repo.workdir().unwrap(), "branch_a", "branch_b")
        .expect("detect conflicts");

    let body_edits: Vec<_> = report
        .conflicts
        .iter()
        .filter(|c| matches!(c.kind, ConflictKind::ConcurrentBodyEdit { .. }))
        .collect();
    assert!(
        !body_edits.is_empty(),
        "expected at least one ConcurrentBodyEdit conflict"
    );
    // Body-only edits are Warning severity per spec
    assert!(
        body_edits.iter().all(|c| c.severity == Severity::Warning),
        "ConcurrentBodyEdit (body only) should be Warning severity"
    );
    assert_eq!(report.exit_code(), 2, "warnings only → exit code 2");
}

/// Both branches add a function with the same qualified name in different files.
#[test]
fn detects_naming_collision() {
    let (_dir, repo) = init_temp_repo();

    // Base: just a main file
    write_and_stage(&repo, "src/main.ts", "function main(): void {}");
    let base = commit(&repo, "base");

    // Branch A: adds "processData" in file A
    create_branch_at(&repo, "branch_a", base);
    write_and_stage(
        &repo,
        "src/processorA.ts",
        "export function processData(input: string): string { return input.toLowerCase(); }",
    );
    commit(&repo, "add processData in A");

    // Branch B: also adds "processData" in file B
    create_branch_at(&repo, "branch_b", base);
    write_and_stage(
        &repo,
        "src/processorB.ts",
        "export function processData(input: number): number { return input * 2; }",
    );
    commit(&repo, "add processData in B");

    let report = ConflictDetector::detect(repo.workdir().unwrap(), "branch_a", "branch_b")
        .expect("detect conflicts");

    let naming_collisions: Vec<_> = report
        .conflicts
        .iter()
        .filter(|c| matches!(c.kind, ConflictKind::NamingCollision { .. }))
        .collect();
    assert!(
        !naming_collisions.is_empty(),
        "expected at least one NamingCollision conflict"
    );
}

/// Both branches change the signature of the same function.
/// Per spec, concurrent signature edits are Error severity (not Warning).
#[test]
fn detects_concurrent_signature_edit_as_error() {
    let (_dir, repo) = init_temp_repo();

    // Base: a function with specific signature
    write_and_stage(
        &repo,
        "src/api.ts",
        "export function fetchData(url: string): string { return url; }",
    );
    let base = commit(&repo, "base");

    // Branch A: changes return type
    create_branch_at(&repo, "branch_a", base);
    write_and_stage(
        &repo,
        "src/api.ts",
        "export function fetchData(url: string): number { return url.length; }",
    );
    commit(&repo, "change return to number");

    // Branch B: changes parameter type
    create_branch_at(&repo, "branch_b", base);
    write_and_stage(
        &repo,
        "src/api.ts",
        "export function fetchData(url: number): string { return String(url); }",
    );
    commit(&repo, "change param to number");

    let report = ConflictDetector::detect(repo.workdir().unwrap(), "branch_a", "branch_b")
        .expect("detect conflicts");

    // Both modified signature → ConcurrentBodyEdit with Error severity
    let concurrent_edits: Vec<_> = report
        .conflicts
        .iter()
        .filter(|c| matches!(c.kind, ConflictKind::ConcurrentBodyEdit { .. }))
        .collect();
    assert!(
        !concurrent_edits.is_empty(),
        "expected ConcurrentBodyEdit for concurrent signature changes"
    );
    assert!(
        concurrent_edits
            .iter()
            .all(|c| c.severity == Severity::Error),
        "concurrent signature edits should be Error severity"
    );
    assert_eq!(report.exit_code(), 1, "errors → exit code 1");
}

// ─────────────────────────────────────────────────────────────────────────────
// 3. Multi-conflict scenarios
// ─────────────────────────────────────────────────────────────────────────────

/// A realistic scenario with multiple conflict types in one check.
#[test]
fn multiple_conflicts_in_single_check() {
    let (_dir, repo) = init_temp_repo();

    // Base: several functions with dependencies
    write_and_stage(
        &repo,
        "src/utils.ts",
        concat!(
            "export function formatName(name: string): string { return name.trim(); }\n",
            "export function validate(input: string): boolean { return input.length > 0; }",
        ),
    );
    write_and_stage(
        &repo,
        "src/app.ts",
        concat!(
            "function processUser(name: string): string {\n",
            "    if (validate(name)) { return formatName(name); }\n",
            "    return '';\n",
            "}",
        ),
    );
    let base = commit(&repo, "base");

    // Branch A: deletes validate, changes formatName signature
    create_branch_at(&repo, "branch_a", base);
    write_and_stage(
        &repo,
        "src/utils.ts",
        "export function formatName(first: string, last: string): string { return `${first} ${last}`; }",
    );
    commit(&repo, "remove validate, change formatName sig");

    // Branch B: modifies formatName body, adds new caller of validate
    create_branch_at(&repo, "branch_b", base);
    write_and_stage(
        &repo,
        "src/utils.ts",
        concat!(
            "export function formatName(name: string): string { return name.trim().toUpperCase(); }\n",
            "export function validate(input: string): boolean { return input.length > 0; }",
        ),
    );
    write_and_stage(
        &repo,
        "src/registration.ts",
        "function register(email: string): boolean { return validate(email); }",
    );
    commit(&repo, "modify formatName body, add registration");

    let report = ConflictDetector::detect(repo.workdir().unwrap(), "branch_a", "branch_b")
        .expect("detect conflicts");

    // Should detect multiple conflicts:
    // - formatName: A changed signature, B changed body → at least ConcurrentBodyEdit
    // - validate: A deleted it, B's graph depends on it → DeletedDependency
    assert!(
        report.conflicts.len() >= 2,
        "expected at least 2 conflicts, got {}",
        report.conflicts.len()
    );
    assert_eq!(report.exit_code(), 1, "should have errors");
}

// ─────────────────────────────────────────────────────────────────────────────
// 4. Report utility methods
// ─────────────────────────────────────────────────────────────────────────────

/// Test ConflictReport counting methods and exit codes directly.
#[test]
fn conflict_report_counts_and_exit_codes() {
    use nex_core::{ConflictReport, SemanticConflict, SemanticUnit, UnitKind};

    let dummy_unit = SemanticUnit {
        id: [0u8; 32],
        kind: UnitKind::Function,
        name: "dummy".into(),
        qualified_name: "dummy".into(),
        file_path: "test.ts".into(),
        byte_range: 0..10,
        signature_hash: 0,
        body_hash: 0,
        dependencies: vec![],
    };

    let error_conflict = SemanticConflict {
        kind: ConflictKind::DeletedDependency {
            deleted: [1u8; 32],
            dependent: [2u8; 32],
        },
        severity: Severity::Error,
        unit_a: dummy_unit.clone(),
        unit_b: dummy_unit.clone(),
        description: "test error".into(),
        suggestion: None,
    };

    let warning_conflict = SemanticConflict {
        kind: ConflictKind::ConcurrentBodyEdit { unit: [3u8; 32] },
        severity: Severity::Warning,
        unit_a: dummy_unit.clone(),
        unit_b: dummy_unit.clone(),
        description: "test warning".into(),
        suggestion: Some("merge manually".into()),
    };

    // Empty report → exit code 0
    let empty = ConflictReport {
        conflicts: vec![],
        branch_a: "a".into(),
        branch_b: "b".into(),
        merge_base: "abc123".into(),
    };
    assert_eq!(empty.error_count(), 0);
    assert_eq!(empty.warning_count(), 0);
    assert_eq!(empty.exit_code(), 0);

    // Warnings only → exit code 2
    let warnings = ConflictReport {
        conflicts: vec![warning_conflict.clone()],
        branch_a: "a".into(),
        branch_b: "b".into(),
        merge_base: "abc123".into(),
    };
    assert_eq!(warnings.error_count(), 0);
    assert_eq!(warnings.warning_count(), 1);
    assert_eq!(warnings.exit_code(), 2);

    // Errors present → exit code 1
    let errors = ConflictReport {
        conflicts: vec![error_conflict, warning_conflict],
        branch_a: "a".into(),
        branch_b: "b".into(),
        merge_base: "abc123".into(),
    };
    assert_eq!(errors.error_count(), 1);
    assert_eq!(errors.warning_count(), 1);
    assert_eq!(errors.exit_code(), 1);
}

/// The merge_base and branch names should be populated in the report.
#[test]
fn merge_base_is_populated_in_report() {
    let (_dir, repo) = init_temp_repo();

    write_and_stage(&repo, "src/app.ts", "function main(): void {}");
    let base = commit(&repo, "base");

    create_branch_at(&repo, "branch_a", base);
    write_and_stage(&repo, "src/a.ts", "function a(): void {}");
    commit(&repo, "branch a commit");

    create_branch_at(&repo, "branch_b", base);
    write_and_stage(&repo, "src/b.ts", "function b(): void {}");
    commit(&repo, "branch b commit");

    let report = ConflictDetector::detect(repo.workdir().unwrap(), "branch_a", "branch_b")
        .expect("detect conflicts");

    assert_eq!(report.branch_a, "branch_a");
    assert_eq!(report.branch_b, "branch_b");
    assert!(!report.merge_base.is_empty(), "merge_base should be set");
    // Merge base should be a hex string (40 chars for full SHA-1)
    assert!(
        report.merge_base.len() >= 7,
        "merge_base should be a valid hex string"
    );
}
