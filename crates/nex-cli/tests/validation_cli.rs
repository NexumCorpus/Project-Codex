//! Integration tests for the pre-commit validation pipeline.
//!
//! These tests exercise the Prompt 007 validation workflow:
//! - ValidationEngine (nex-validate) checks lock coverage and reference integrity
//! - CLI pipeline (run_validate) wires git -> parse -> graph -> validate
//! - Output formatting (format_validation_report) for text and JSON
//!
//! IMPORTANT: These are the acceptance criteria for Prompt 007.
//! Do NOT modify these tests - Codex must write code that passes them.

use nex_cli::coordination_pipeline::{agent_name_to_id, run_lock, run_validate};
use nex_cli::output::format_validation_report;
use std::path::Path;

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

/// Set up a two-commit repo with a signature change and stale caller.
fn setup_signature_change_repo() -> (tempfile::TempDir, git2::Repository) {
    let (dir, repo) = init_temp_repo();

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

/// Set up a two-commit repo where a deleted function's caller was updated.
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

/// Set up a two-commit repo where a deleted function still has a live caller.
fn setup_deletion_broken_ref() -> (tempfile::TempDir, git2::Repository) {
    let (dir, repo) = init_temp_repo();

    write_and_stage(&repo, "shared.ts", "function helperFn(): void { }\n");
    write_and_stage(
        &repo,
        "main.ts",
        "function entryPoint(): void { helperFn(); }\n",
    );
    commit(&repo, "initial");

    write_and_stage(&repo, "shared.ts", "// empty\n");
    commit(&repo, "delete helperFn");

    (dir, repo)
}

/// Set up a simple two-commit repo with only a body change.
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

#[test]
fn validate_clean_with_write_lock() {
    let (_dir, repo) = setup_body_change_repo();
    let repo_path = repo.workdir().unwrap();

    run_lock(repo_path, "alice", "greet", "write").unwrap();

    let report = run_validate(repo_path, "alice", "HEAD~1").unwrap();
    assert_eq!(report.error_count(), 0, "no errors when lock is held");
    assert_eq!(
        report.warning_count(),
        0,
        "no warnings for body-only change"
    );
    assert!(report.units_checked > 0, "should have checked some units");
}

#[test]
fn validate_clean_with_delete_lock() {
    let (_dir, repo) = setup_deletion_with_updated_caller();
    let repo_path = repo.workdir().unwrap();

    use nex_cli::coordination_pipeline::{LockEntry, save_locks};
    use nex_core::IntentKind;

    let agent_id = agent_name_to_id("alice");
    let extractors: Vec<Box<dyn nex_parse::SemanticExtractor>> =
        vec![nex_parse::typescript_extractor()];
    let git_repo = git2::Repository::open(repo_path).unwrap();
    let files = nex_cli::pipeline::collect_files_at_ref(&git_repo, "HEAD~1", &extractors).unwrap();
    let graph = nex_cli::pipeline::build_graph(&files, &extractors).unwrap();
    let validate_unit = graph
        .units()
        .into_iter()
        .find(|u| u.name == "validate")
        .unwrap();
    let process_unit = graph
        .units()
        .into_iter()
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
    assert_eq!(
        report.error_count(),
        0,
        "no errors with proper delete+write locks"
    );
}

#[test]
fn validate_detects_unlocked_modification() {
    let (_dir, repo) = setup_body_change_repo();
    let repo_path = repo.workdir().unwrap();

    let report = run_validate(repo_path, "alice", "HEAD~1").unwrap();
    assert!(
        report.error_count() > 0,
        "should detect unlocked modification"
    );

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
    let unlocked_mod = report.issues.iter().find(|issue| {
        matches!(
            issue.kind,
            nex_core::ValidationKind::UnlockedModification { .. }
        )
    });
    assert!(
        unlocked_mod.is_some(),
        "should have UnlockedModification issue"
    );
    assert_eq!(unlocked_mod.unwrap().severity, nex_core::Severity::Error);
}

#[test]
fn validate_detects_unlocked_deletion() {
    let (_dir, repo) = setup_deletion_with_updated_caller();
    let repo_path = repo.workdir().unwrap();

    let report = run_validate(repo_path, "alice", "HEAD~1").unwrap();

    let deletion_issue = report.issues.iter().find(|issue| {
        matches!(
            issue.kind,
            nex_core::ValidationKind::UnlockedDeletion { .. }
        )
    });
    assert!(deletion_issue.is_some(), "should detect unlocked deletion");
    assert!(
        deletion_issue
            .unwrap()
            .description
            .contains("without a Delete lock"),
        "description should mention Delete lock"
    );
}

#[test]
fn validate_detects_stale_caller_after_signature_change() {
    let (_dir, repo) = setup_signature_change_repo();
    let repo_path = repo.workdir().unwrap();

    run_lock(repo_path, "alice", "validate", "write").unwrap();

    let report = run_validate(repo_path, "alice", "HEAD~1").unwrap();

    let stale = report
        .issues
        .iter()
        .find(|issue| matches!(issue.kind, nex_core::ValidationKind::StaleCallers { .. }));
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
    let stale = report
        .issues
        .iter()
        .find(|issue| matches!(issue.kind, nex_core::ValidationKind::StaleCallers { .. }));
    assert!(stale.is_some());
    assert_eq!(stale.unwrap().severity, nex_core::Severity::Warning);
}

#[test]
fn validate_detects_broken_reference() {
    let (_dir, repo) = setup_deletion_broken_ref();
    let repo_path = repo.workdir().unwrap();

    let report = run_validate(repo_path, "alice", "HEAD~1").unwrap();

    let broken = report
        .issues
        .iter()
        .find(|issue| matches!(issue.kind, nex_core::ValidationKind::BrokenReference { .. }));
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

    let broken = report
        .issues
        .iter()
        .find(|issue| matches!(issue.kind, nex_core::ValidationKind::BrokenReference { .. }));
    assert!(
        broken.is_none(),
        "should NOT flag broken ref when caller was also modified"
    );
}

#[test]
fn validate_issues_sorted_errors_before_warnings() {
    let (_dir, repo) = setup_signature_change_repo();
    let repo_path = repo.workdir().unwrap();

    let report = run_validate(repo_path, "alice", "HEAD~1").unwrap();

    if report.issues.len() >= 2 {
        for index in 0..report.issues.len() - 1 {
            assert!(
                report.issues[index].severity >= report.issues[index + 1].severity,
                "issues should be sorted by severity (Error >= Warning >= Info)"
            );
        }
    }
}

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
    assert!(
        text.contains("UnlockedModification"),
        "should show kind label"
    );
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
    assert!(
        parsed.get("issues").is_some(),
        "JSON should have issues field"
    );
    assert!(
        parsed.get("agent_name").is_some(),
        "JSON should have agent_name field"
    );
}

#[test]
fn validate_units_checked_includes_all_change_types() {
    let (_dir, repo) = setup_deletion_with_updated_caller();
    let repo_path = repo.workdir().unwrap();

    let report = run_validate(repo_path, "alice", "HEAD~1").unwrap();
    assert!(
        report.units_checked >= 2,
        "units_checked should count modified + removed + added, got {}",
        report.units_checked
    );
}
