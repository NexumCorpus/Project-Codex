use nex_cli::coordination_pipeline::{
    LockEntry, agent_name_to_id, load_locks, run_lock, run_locks, run_unlock, save_locks,
};
use nex_core::IntentKind;
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

fn setup_repo() -> (tempfile::TempDir, git2::Repository) {
    let (dir, repo) = init_temp_repo();
    write_and_stage(
        &repo,
        "handler.ts",
        r#"function validate(input: string): boolean { return input.length > 0; }
function processRequest(req: string): void { validate(req); }
"#,
    );
    commit(&repo, "initial");
    (dir, repo)
}

#[test]
fn save_and_load_round_trip_survives_json_removal() {
    let dir = tempfile::tempdir().expect("tempdir");
    let entry = LockEntry {
        agent_name: "alice".to_string(),
        agent_id: agent_name_to_id("alice"),
        target_name: "processRequest".to_string(),
        target: [7u8; 32],
        kind: IntentKind::Write,
    };

    save_locks(dir.path(), std::slice::from_ref(&entry)).unwrap();
    std::fs::remove_file(dir.path().join(".nex").join("locks.json")).unwrap();

    let loaded = load_locks(dir.path()).unwrap();
    assert_eq!(loaded, vec![entry]);
    assert!(dir.path().join(".nex").join("coordination.loro").exists());
}

#[test]
fn unlock_uses_crdt_state_when_json_snapshot_is_missing() {
    let (_dir, repo) = setup_repo();
    let repo_path = repo.workdir().unwrap();

    run_lock(repo_path, "alice", "processRequest", "write").unwrap();
    std::fs::remove_file(repo_path.join(".nex").join("locks.json")).unwrap();

    run_unlock(repo_path, "alice", "processRequest").unwrap();
    let entries = run_locks(repo_path).unwrap();
    assert!(entries.is_empty());
    assert!(repo_path.join(".nex").join("coordination.loro").exists());
}
