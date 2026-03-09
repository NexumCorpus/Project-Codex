use chrono::Utc;
use nex_coord::{CoordinationService, IntentPayload, IntentResult, PlannedChange};
use nex_core::{DepKind, SemanticUnit, UnitKind};
use nex_graph::CodeGraph;
use std::path::PathBuf;
use std::time::Duration;
use uuid::Uuid;

fn semantic_unit(name: &str, file: &str) -> SemanticUnit {
    let qualified_name = name.to_string();
    let id = *blake3::hash(qualified_name.as_bytes()).as_bytes();
    SemanticUnit {
        id,
        kind: UnitKind::Function,
        name: name.to_string(),
        qualified_name,
        file_path: PathBuf::from(file),
        byte_range: 0..10,
        signature_hash: 1,
        body_hash: 2,
        dependencies: Vec::new(),
    }
}

fn build_graph() -> (CodeGraph, SemanticUnit, SemanticUnit, SemanticUnit) {
    let validate = semantic_unit("validate", "handler.ts");
    let process_request = semantic_unit("processRequest", "handler.ts");
    let format_date = semantic_unit("formatDate", "utils.ts");

    let mut graph = CodeGraph::new();
    graph.add_unit(validate.clone());
    graph.add_unit(process_request.clone());
    graph.add_unit(format_date.clone());
    graph.add_dep(process_request.id, validate.id, DepKind::Calls);

    (graph, validate, process_request, format_date)
}

fn payload(agent: &str, target: &SemanticUnit) -> IntentPayload {
    IntentPayload {
        id: Uuid::new_v4(),
        agent_id: agent.to_string(),
        timestamp: Utc::now(),
        description: format!("edit {}", target.name),
        target_units: vec![target.id],
        estimated_changes: vec![PlannedChange::ModifyBody { unit: target.id }],
        ttl: Duration::from_secs(300),
    }
}

fn approved_token(result: IntentResult) -> Uuid {
    match result {
        IntentResult::Approved { lock_token, .. } => lock_token,
        other => panic!("expected approval, got {other:?}"),
    }
}

#[test]
fn merge_crdt_imports_remote_intent_locks() {
    let (_, validate, _, _) = build_graph();
    let mut left = CoordinationService::new_with_peer(build_graph().0, 1);
    let mut right = CoordinationService::new_with_peer(build_graph().0, 2);

    let result = left.declare_intent(payload("alice", &validate)).unwrap();
    assert!(matches!(result, IntentResult::Approved { .. }));

    let exported = left.export_crdt().unwrap();
    right.merge_crdt(&exported).unwrap();

    let locks = right.locks();
    assert_eq!(locks.len(), 1);
    assert_eq!(locks[0].holder, "alice");
    assert_eq!(locks[0].target_name, "validate");
}

#[test]
fn merge_crdt_converges_disjoint_intents() {
    let (_, validate, _, format_date) = build_graph();
    let mut left = CoordinationService::new_with_peer(build_graph().0, 11);
    let mut right = CoordinationService::new_with_peer(build_graph().0, 22);

    let left_result = left.declare_intent(payload("alice", &validate)).unwrap();
    let right_result = right.declare_intent(payload("bob", &format_date)).unwrap();
    assert!(matches!(left_result, IntentResult::Approved { .. }));
    assert!(matches!(right_result, IntentResult::Approved { .. }));

    let left_bytes = left.export_crdt().unwrap();
    let right_bytes = right.export_crdt().unwrap();
    left.merge_crdt(&right_bytes).unwrap();
    right.merge_crdt(&left_bytes).unwrap();

    let left_holders: Vec<_> = left.locks().into_iter().map(|lock| lock.holder).collect();
    let right_holders: Vec<_> = right.locks().into_iter().map(|lock| lock.holder).collect();
    assert_eq!(left_holders, vec!["bob".to_string(), "alice".to_string()]);
    assert_eq!(right_holders, vec!["bob".to_string(), "alice".to_string()]);
}

#[test]
fn merge_crdt_propagates_committed_intent_removal() {
    let (_, validate, _, _) = build_graph();
    let mut left = CoordinationService::new_with_peer(build_graph().0, 101);
    let mut right = CoordinationService::new_with_peer(build_graph().0, 202);

    let intent = payload("alice", &validate);
    let token = approved_token(left.declare_intent(intent.clone()).unwrap());

    let initial_bytes = left.export_crdt().unwrap();
    right.merge_crdt(&initial_bytes).unwrap();
    assert_eq!(right.locks().len(), 1);

    let commit = left.commit_intent(intent.id, token).unwrap();
    assert_eq!(commit.released_locks, 1);

    let after_commit = left.export_crdt().unwrap();
    right.merge_crdt(&after_commit).unwrap();
    assert!(right.locks().is_empty());
}
