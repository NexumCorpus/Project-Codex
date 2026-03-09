//! Integration tests for the CLI coordination pipeline.
//!
//! These tests exercise the Prompt 006 CLI coordination workflow:
//! - Agent name to ID hashing
//! - Intent kind parsing
//! - Lock persistence (save/load round-trip)
//! - Full lock/unlock/list pipelines against a real git repo
//!
//! IMPORTANT: These are the acceptance criteria for Prompt 006.
//! Do NOT modify these tests — Codex must write code that passes them.

use nex_cli::coordination_pipeline::{
    LockEntry, agent_name_to_id, load_locks, parse_intent_kind, run_lock, run_locks, run_unlock,
    save_locks,
};
use nex_core::{IntentKind, LockResult};
use std::path::Path;

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Create a temporary git repo, returning the tempdir (keeps it alive) and the repo.
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

/// Set up a repo with two TypeScript files committed at HEAD:
///
/// handler.ts:
///   function validate(input: string): boolean { return input.length > 0; }
///   function processRequest(req: string): void { validate(req); }
///
/// utils.ts:
///   function formatDate(d: Date): string { return d.toISOString(); }
fn setup_repo() -> (tempfile::TempDir, git2::Repository) {
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
    (dir, repo)
}

// ─────────────────────────────────────────────────────────────────────────────
// 1. Helper function tests
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn agent_name_produces_deterministic_id() {
    let id1 = agent_name_to_id("alice");
    let id2 = agent_name_to_id("alice");
    let id3 = agent_name_to_id("bob");
    assert_eq!(id1, id2, "same name should produce same id");
    assert_ne!(id1, id3, "different names should produce different ids");
}

#[test]
fn parse_intent_kind_accepts_valid_kinds() {
    assert!(matches!(
        parse_intent_kind("read").unwrap(),
        IntentKind::Read
    ));
    assert!(matches!(
        parse_intent_kind("Write").unwrap(),
        IntentKind::Write
    ));
    assert!(matches!(
        parse_intent_kind("DELETE").unwrap(),
        IntentKind::Delete
    ));
}

#[test]
fn parse_intent_kind_rejects_invalid() {
    assert!(parse_intent_kind("mutate").is_err());
}

// ─────────────────────────────────────────────────────────────────────────────
// 2. Persistence tests
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn load_locks_returns_empty_when_no_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let entries = load_locks(dir.path()).unwrap();
    assert!(entries.is_empty());
}

#[test]
fn save_and_load_round_trip() {
    let dir = tempfile::tempdir().expect("tempdir");
    let id = agent_name_to_id("alice");
    let entries = vec![LockEntry {
        agent_name: "alice".to_string(),
        agent_id: id,
        target_name: "processRequest".to_string(),
        target: [0u8; 32],
        kind: IntentKind::Write,
    }];

    save_locks(dir.path(), &entries).unwrap();
    let loaded = load_locks(dir.path()).unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].agent_name, "alice");
    assert_eq!(loaded[0].target_name, "processRequest");
    assert_eq!(loaded[0].agent_id, id);
}

// ─────────────────────────────────────────────────────────────────────────────
// 3. Full pipeline tests
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn lock_grants_on_free_unit() {
    let (_dir, repo) = setup_repo();
    let repo_path = repo.workdir().unwrap();

    let result = run_lock(repo_path, "alice", "processRequest", "write").unwrap();
    assert!(matches!(result, LockResult::Granted));
}

#[test]
fn lock_denied_same_agent_double_lock() {
    let (_dir, repo) = setup_repo();
    let repo_path = repo.workdir().unwrap();

    run_lock(repo_path, "alice", "processRequest", "write").unwrap();
    let result = run_lock(repo_path, "alice", "processRequest", "read").unwrap();
    assert!(matches!(result, LockResult::Denied { .. }));
}

#[test]
fn lock_persists_and_lists() {
    let (_dir, repo) = setup_repo();
    let repo_path = repo.workdir().unwrap();

    run_lock(repo_path, "alice", "processRequest", "write").unwrap();
    let entries = run_locks(repo_path).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].agent_name, "alice");
    assert_eq!(entries[0].target_name, "processRequest");
}

#[test]
fn unlock_removes_lock() {
    let (_dir, repo) = setup_repo();
    let repo_path = repo.workdir().unwrap();

    run_lock(repo_path, "alice", "processRequest", "write").unwrap();
    run_unlock(repo_path, "alice", "processRequest").unwrap();
    let entries = run_locks(repo_path).unwrap();
    assert!(entries.is_empty());
}

#[test]
fn unlock_nonexistent_returns_error() {
    let (_dir, repo) = setup_repo();
    let repo_path = repo.workdir().unwrap();

    let result = run_unlock(repo_path, "alice", "processRequest");
    assert!(result.is_err());
}

#[test]
fn lock_denied_write_conflict() {
    let (_dir, repo) = setup_repo();
    let repo_path = repo.workdir().unwrap();

    run_lock(repo_path, "alice", "processRequest", "write").unwrap();
    let result = run_lock(repo_path, "bob", "processRequest", "write").unwrap();
    assert!(matches!(result, LockResult::Denied { .. }));
}

#[test]
fn lock_unknown_target_returns_error() {
    let (_dir, repo) = setup_repo();
    let repo_path = repo.workdir().unwrap();

    let result = run_lock(repo_path, "alice", "nonExistentFunction", "write");
    assert!(result.is_err());
}
