use chrono::Utc;
use futures_util::StreamExt;
use nex_cli::serve_pipeline::{AbortRequest, CommitRequest, spawn_server};
use nex_coord::{CoordEvent, IntentPayload, IntentResult, LockEntry, PlannedChange};
use nex_core::SemanticUnit;
use nex_eventlog::{Mutation, SemanticEvent};
use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;
use tokio_tungstenite::connect_async;
use uuid::Uuid;

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

fn declare_payload(unit: &SemanticUnit) -> IntentPayload {
    IntentPayload {
        id: Uuid::new_v4(),
        agent_id: "alice".to_string(),
        timestamp: Utc::now(),
        description: format!("edit {}", unit.name),
        target_units: vec![unit.id],
        estimated_changes: vec![PlannedChange::ModifyBody { unit: unit.id }],
        ttl: Duration::from_secs(30),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn serve_exposes_declare_locks_commit_and_abort() {
    let (_dir, repo) = setup_repo();
    let repo_path = repo.workdir().unwrap().to_path_buf();
    let server = spawn_server(&repo_path, "127.0.0.1:0".parse::<SocketAddr>().unwrap())
        .await
        .unwrap();
    let base = format!("http://{}", server.local_addr());
    let client = reqwest::Client::new();

    let process_request: Vec<SemanticUnit> = client
        .get(format!("{base}/graph/query"))
        .query(&[("kind", "units_named"), ("value", "processRequest")])
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(process_request.len(), 1);

    let declare = declare_payload(&process_request[0]);
    let declare_result: IntentResult = client
        .post(format!("{base}/intent/declare"))
        .json(&declare)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();

    let lock_token = match declare_result {
        IntentResult::Approved { lock_token, .. } => lock_token,
        other => panic!("expected approval, got {other:?}"),
    };

    let locks: Vec<LockEntry> = client
        .get(format!("{base}/locks"))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(locks.len(), 1);
    assert_eq!(locks[0].holder, "alice");
    assert_eq!(locks[0].target_name, "processRequest");

    let mut after = process_request[0].clone();
    after.body_hash += 1;
    let commit_response: nex_cli::serve_pipeline::CommitResponse = client
        .post(format!("{base}/intent/commit"))
        .json(&CommitRequest {
            intent_id: declare.id,
            lock_token,
            description: Some("commit processRequest".to_string()),
            mutations: vec![Mutation::ModifyUnit {
                id: process_request[0].id,
                before: process_request[0].clone(),
                after: after.clone(),
            }],
            parent_event: None,
            tags: vec!["feature:test".to_string()],
        })
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(commit_response.intent_id, declare.id);

    let events_path = repo_path.join(".nex").join("events.json");
    let logged: Vec<SemanticEvent> =
        serde_json::from_str(&std::fs::read_to_string(events_path).unwrap()).unwrap();
    assert_eq!(logged.len(), 1);
    assert_eq!(logged[0].id, commit_response.event_id);

    let locks_after_commit: Vec<LockEntry> = client
        .get(format!("{base}/locks"))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(locks_after_commit.is_empty());

    let validate: Vec<SemanticUnit> = client
        .get(format!("{base}/graph/query"))
        .query(&[("kind", "units_named"), ("value", "validate")])
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let abort_declare = declare_payload(&validate[0]);
    let abort_result: IntentResult = client
        .post(format!("{base}/intent/declare"))
        .json(&abort_declare)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let abort_token = match abort_result {
        IntentResult::Approved { lock_token, .. } => lock_token,
        other => panic!("expected approval, got {other:?}"),
    };

    let abort_response: nex_cli::serve_pipeline::AbortResponse = client
        .post(format!("{base}/intent/abort"))
        .json(&AbortRequest {
            intent_id: abort_declare.id,
            lock_token: abort_token,
        })
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(abort_response.intent_id, abort_declare.id);

    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn serve_streams_coordination_events_over_websocket() {
    let (_dir, repo) = setup_repo();
    let repo_path = repo.workdir().unwrap().to_path_buf();
    let server = spawn_server(&repo_path, "127.0.0.1:0".parse::<SocketAddr>().unwrap())
        .await
        .unwrap();
    let http_base = format!("http://{}", server.local_addr());
    let ws_base = format!("ws://{}/events", server.local_addr());
    let client = reqwest::Client::new();

    let (mut socket, _) = connect_async(&ws_base).await.unwrap();

    let validate: Vec<SemanticUnit> = client
        .get(format!("{http_base}/graph/query"))
        .query(&[("kind", "units_named"), ("value", "validate")])
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();

    let declare = declare_payload(&validate[0]);
    let result: IntentResult = client
        .post(format!("{http_base}/intent/declare"))
        .json(&declare)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(matches!(result, IntentResult::Approved { .. }));

    let message = tokio::time::timeout(Duration::from_secs(5), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let text = message.into_text().unwrap();
    let event: CoordEvent = serde_json::from_str(&text).unwrap();
    match event {
        CoordEvent::IntentDeclared {
            intent_id,
            agent_id,
            ..
        } => {
            assert_eq!(intent_id, declare.id);
            assert_eq!(agent_id, "alice");
        }
        other => panic!("expected intent declared event, got {other:?}"),
    }

    server.shutdown().await;
}
