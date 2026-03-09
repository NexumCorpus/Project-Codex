//! Python integration tests for the validation pipeline.

use nex_cli::coordination_pipeline::{run_lock, run_validate};
use std::path::Path;

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

fn setup_body_change_repo() -> (tempfile::TempDir, git2::Repository) {
    let (dir, repo) = init_temp_repo();

    write_and_stage(
        &repo,
        "greet.py",
        "def greet(name: str) -> str:\n    return 'hello ' + name\n",
    );
    commit(&repo, "initial");

    write_and_stage(
        &repo,
        "greet.py",
        "def greet(name: str) -> str:\n    return 'hi ' + name\n",
    );
    commit(&repo, "change body");

    (dir, repo)
}

#[test]
fn validate_detects_unlocked_python_modification() {
    let (_dir, repo) = setup_body_change_repo();
    let report = run_validate(repo.workdir().unwrap(), "alice", "HEAD~1").unwrap();

    assert!(report.error_count() > 0);
    assert!(
        report
            .issues
            .iter()
            .any(|issue| issue.description.contains("without a Write lock"))
    );
}

#[test]
fn validate_accepts_python_write_lock() {
    let (_dir, repo) = setup_body_change_repo();
    let repo_path = repo.workdir().unwrap();

    run_lock(repo_path, "alice", "greet", "write").unwrap();
    let report = run_validate(repo_path, "alice", "HEAD~1").unwrap();

    assert_eq!(report.error_count(), 0);
}
