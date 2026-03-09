use chrono::Utc;
use nex_coord::{
    CoordinationService, GraphQuery, GraphQueryKind, IntentPayload, IntentResult, PlannedChange,
};
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

fn build_graph() -> (CodeGraph, SemanticUnit, SemanticUnit) {
    let validate = semantic_unit("validate", "handler.ts");
    let process_request = semantic_unit("processRequest", "handler.ts");

    let mut graph = CodeGraph::new();
    graph.add_unit(validate.clone());
    graph.add_unit(process_request.clone());
    graph.add_dep(process_request.id, validate.id, DepKind::Calls);

    (graph, validate, process_request)
}

fn payload(agent: &str, target: &SemanticUnit, ttl: Duration) -> IntentPayload {
    IntentPayload {
        id: Uuid::new_v4(),
        agent_id: agent.to_string(),
        timestamp: Utc::now(),
        description: format!("edit {}", target.name),
        target_units: vec![target.id],
        estimated_changes: vec![PlannedChange::ModifyBody { unit: target.id }],
        ttl,
    }
}

#[test]
fn declare_intent_approves_and_lists_locks() {
    let (graph, validate, _) = build_graph();
    let mut service = CoordinationService::new(graph);

    let result = service.declare_intent(payload("alice", &validate, Duration::from_secs(30)));
    assert!(matches!(result.unwrap(), IntentResult::Approved { .. }));

    let locks = service.locks();
    assert_eq!(locks.len(), 1);
    assert_eq!(locks[0].holder, "alice");
    assert_eq!(locks[0].target_name, "validate");
}

#[test]
fn declare_intent_rejects_conflicting_agent() {
    let (graph, validate, _) = build_graph();
    let mut service = CoordinationService::new(graph);

    let alice = service
        .declare_intent(payload("alice", &validate, Duration::from_secs(30)))
        .unwrap();
    assert!(matches!(alice, IntentResult::Approved { .. }));

    let bob = service
        .declare_intent(payload("bob", &validate, Duration::from_secs(30)))
        .unwrap();
    match bob {
        IntentResult::Rejected { conflicts } => {
            assert_eq!(conflicts.len(), 1);
            assert_eq!(conflicts[0].blocking_agent, "alice");
            assert_eq!(conflicts[0].contested_unit, validate.id);
        }
        other => panic!("expected rejection, got {other:?}"),
    }
}

#[test]
fn query_graph_returns_callers_and_dependencies() {
    let (graph, validate, process_request) = build_graph();
    let service = CoordinationService::new(graph);

    let callers = service
        .query_graph(&GraphQuery {
            kind: GraphQueryKind::CallersOf,
            value: "validate".to_string(),
        })
        .unwrap();
    assert_eq!(callers.len(), 1);
    assert_eq!(callers[0].qualified_name, process_request.qualified_name);

    let deps = service
        .query_graph(&GraphQuery {
            kind: GraphQueryKind::DepsOf,
            value: "processRequest".to_string(),
        })
        .unwrap();
    assert_eq!(deps.len(), 1);
    assert_eq!(deps[0].qualified_name, validate.qualified_name);
}

#[test]
fn commit_and_abort_release_locks() {
    let (graph, validate, process_request) = build_graph();
    let mut service = CoordinationService::new(graph);

    let first = payload("alice", &validate, Duration::from_secs(30));
    let first_result = service.declare_intent(first.clone()).unwrap();
    let first_token = match first_result {
        IntentResult::Approved { lock_token, .. } => lock_token,
        other => panic!("expected approval, got {other:?}"),
    };

    let commit = service.commit_intent(first.id, first_token).unwrap();
    assert_eq!(commit.released_locks, 1);
    assert!(service.locks().is_empty());

    let second = payload("bob", &process_request, Duration::from_secs(30));
    let second_result = service.declare_intent(second.clone()).unwrap();
    let second_token = match second_result {
        IntentResult::Approved { lock_token, .. } => lock_token,
        other => panic!("expected approval, got {other:?}"),
    };

    let abort = service.abort_intent(second.id, second_token).unwrap();
    assert_eq!(abort.released_locks, 1);
    assert!(service.locks().is_empty());
}

#[test]
fn expire_stale_releases_locks() {
    let (graph, validate, _) = build_graph();
    let mut service = CoordinationService::new(graph);

    let result = service.declare_intent(payload("alice", &validate, Duration::from_secs(0)));
    assert!(matches!(result.unwrap(), IntentResult::Approved { .. }));

    let expired = service.expire_stale();
    assert_eq!(expired.len(), 1);
    assert_eq!(expired[0].agent_id, "alice");
    assert!(service.locks().is_empty());
}
