//! Integration tests for the Phase 3 eventlog CLI pipeline.

use chrono::{TimeZone, Utc};
use nex_cli::eventlog_pipeline::{run_log, run_replay, run_rollback};
use nex_cli::output::{format_event_log, format_replay_state, format_rollback_outcome};
use nex_core::{SemanticUnit, UnitKind};
use nex_eventlog::{EventLog, Mutation, SemanticEvent};
use std::path::PathBuf;
use uuid::Uuid;

fn temp_repo() -> tempfile::TempDir {
    tempfile::tempdir().expect("tempdir")
}

fn unit(name: &str, file: &str, body_hash: u64) -> SemanticUnit {
    let id = *blake3::hash(format!("{name}:{file}").as_bytes()).as_bytes();
    SemanticUnit {
        id,
        kind: UnitKind::Function,
        name: name.to_string(),
        qualified_name: name.to_string(),
        file_path: PathBuf::from(file),
        byte_range: 0..100,
        signature_hash: 1,
        body_hash,
        dependencies: Vec::new(),
    }
}

fn event(
    event_id: Uuid,
    intent_id: Uuid,
    ts: i64,
    description: &str,
    mutations: Vec<Mutation>,
) -> SemanticEvent {
    SemanticEvent {
        id: event_id,
        timestamp: Utc.timestamp_opt(ts, 0).single().unwrap(),
        intent_id,
        agent_id: "alice".to_string(),
        description: description.to_string(),
        mutations,
        parent_event: None,
        tags: Vec::new(),
    }
}

fn repo_log(repo: &std::path::Path) -> EventLog {
    EventLog::new(repo.join(".nex").join("events.json"))
}

#[tokio::test(flavor = "current_thread")]
async fn run_log_returns_empty_when_no_event_file() {
    let dir = temp_repo();
    let events = run_log(dir.path(), None).await.unwrap();
    assert!(events.is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn run_log_lists_events_in_timestamp_order() {
    let dir = temp_repo();
    let log = repo_log(dir.path());
    let intent_id = Uuid::new_v4();
    let feature = unit("feature", "feature.ts", 10);

    let later = event(
        Uuid::new_v4(),
        intent_id,
        20,
        "later event",
        vec![Mutation::RenameUnit {
            id: feature.id,
            from: "feature".to_string(),
            to: "featureV2".to_string(),
        }],
    );
    let earlier = event(
        Uuid::new_v4(),
        intent_id,
        10,
        "earlier event",
        vec![Mutation::AddUnit {
            unit: feature.clone(),
        }],
    );
    log.append(later.clone()).await.unwrap();
    log.append(earlier.clone()).await.unwrap();

    let events = run_log(dir.path(), None).await.unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].id, earlier.id);
    assert_eq!(events[1].id, later.id);
}

#[tokio::test(flavor = "current_thread")]
async fn run_log_filters_by_intent_id() {
    let dir = temp_repo();
    let log = repo_log(dir.path());
    let intent_a = Uuid::new_v4();
    let intent_b = Uuid::new_v4();
    let helper = unit("helper", "helper.ts", 10);

    log.append(event(
        Uuid::new_v4(),
        intent_b,
        20,
        "other intent",
        vec![Mutation::AddUnit {
            unit: helper.clone(),
        }],
    ))
    .await
    .unwrap();
    let target = event(
        Uuid::new_v4(),
        intent_a,
        10,
        "target intent",
        vec![Mutation::AddUnit { unit: helper }],
    );
    log.append(target.clone()).await.unwrap();

    let events = run_log(dir.path(), Some(&intent_a.to_string()))
        .await
        .unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].id, target.id);
}

#[tokio::test(flavor = "current_thread")]
async fn run_rollback_appends_compensating_event_when_clean() {
    let dir = temp_repo();
    let log = repo_log(dir.path());
    let intent_id = Uuid::new_v4();
    let feature = unit("feature", "feature.ts", 10);

    let original = event(
        Uuid::new_v4(),
        intent_id,
        10,
        "add feature",
        vec![Mutation::AddUnit {
            unit: feature.clone(),
        }],
    );
    log.append(original.clone()).await.unwrap();

    let outcome = run_rollback(dir.path(), &intent_id.to_string(), "system")
        .await
        .unwrap();
    assert!(outcome.is_clean());
    assert!(outcome.rollback_event.is_some());

    let events = run_log(dir.path(), None).await.unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[1].parent_event, Some(original.id));
}

#[tokio::test(flavor = "current_thread")]
async fn run_rollback_reports_conflict_and_does_not_append() {
    let dir = temp_repo();
    let log = repo_log(dir.path());
    let intent_a = Uuid::new_v4();
    let intent_b = Uuid::new_v4();
    let feature = unit("feature", "feature.ts", 10);
    let changed = unit("feature", "feature.ts", 20);

    log.append(event(
        Uuid::new_v4(),
        intent_a,
        10,
        "add feature",
        vec![Mutation::AddUnit {
            unit: feature.clone(),
        }],
    ))
    .await
    .unwrap();
    log.append(event(
        Uuid::new_v4(),
        intent_b,
        20,
        "modify feature",
        vec![Mutation::ModifyUnit {
            id: feature.id,
            before: feature.clone(),
            after: changed,
        }],
    ))
    .await
    .unwrap();

    let outcome = run_rollback(dir.path(), &intent_a.to_string(), "system")
        .await
        .unwrap();
    assert!(!outcome.is_clean());
    assert!(outcome.rollback_event.is_none());

    let events = run_log(dir.path(), None).await.unwrap();
    assert_eq!(events.len(), 2);
}

#[tokio::test(flavor = "current_thread")]
async fn run_replay_rebuilds_state_at_event_boundary() {
    let dir = temp_repo();
    let log = repo_log(dir.path());
    let intent_id = Uuid::new_v4();
    let before = unit("validate", "handler.ts", 10);
    let after = unit("validate", "handler.ts", 20);
    let helper = unit("helper", "utils.ts", 30);

    let first = event(
        Uuid::new_v4(),
        intent_id,
        10,
        "add validate",
        vec![Mutation::AddUnit {
            unit: before.clone(),
        }],
    );
    let second = event(
        Uuid::new_v4(),
        intent_id,
        20,
        "modify validate",
        vec![Mutation::ModifyUnit {
            id: before.id,
            before: before.clone(),
            after: after.clone(),
        }],
    );
    let third = event(
        Uuid::new_v4(),
        intent_id,
        30,
        "add helper",
        vec![Mutation::AddUnit {
            unit: helper.clone(),
        }],
    );
    log.append(first).await.unwrap();
    log.append(second.clone()).await.unwrap();
    log.append(third).await.unwrap();

    let units = run_replay(dir.path(), &second.id.to_string())
        .await
        .unwrap();
    assert_eq!(units.len(), 1);
    assert_eq!(units[0].qualified_name, "validate");
    assert_eq!(units[0].body_hash, after.body_hash);
}

#[tokio::test(flavor = "current_thread")]
async fn run_replay_invalid_event_id_errors() {
    let dir = temp_repo();
    let result = run_replay(dir.path(), "not-a-uuid").await;
    assert!(result.is_err());
}

#[test]
fn format_event_log_text_includes_descriptions() {
    let feature = unit("feature", "feature.ts", 10);
    let intent_id = Uuid::new_v4();
    let events = vec![event(
        Uuid::new_v4(),
        intent_id,
        10,
        "add feature",
        vec![Mutation::AddUnit { unit: feature }],
    )];

    let text = format_event_log(&events, "text");
    assert!(text.contains("Semantic Event Log (1)"));
    assert!(text.contains("add feature"));
    assert!(text.contains("Mutations: 1"));
}

#[tokio::test(flavor = "current_thread")]
async fn format_rollback_outcome_text_variants() {
    let dir = temp_repo();
    let log = repo_log(dir.path());
    let intent_id = Uuid::new_v4();
    let feature = unit("feature", "feature.ts", 10);

    log.append(event(
        Uuid::new_v4(),
        intent_id,
        10,
        "add feature",
        vec![Mutation::AddUnit {
            unit: feature.clone(),
        }],
    ))
    .await
    .unwrap();
    let clean = run_rollback(dir.path(), &intent_id.to_string(), "system")
        .await
        .unwrap();
    let clean_text = format_rollback_outcome(&clean, "text");
    assert!(clean_text.contains("Rollback APPLIED"));
    assert!(clean_text.contains("Mutations: 1"));

    let blocked = format_rollback_outcome(
        &nex_eventlog::RollbackOutcome {
            original_intent_id: Uuid::new_v4(),
            rollback_event: None,
            conflicts: vec![nex_eventlog::RollbackConflict {
                unit: feature.id,
                blocking_event: Uuid::new_v4(),
                reason: "later event touched rollback target".to_string(),
            }],
        },
        "text",
    );
    assert!(blocked.contains("Rollback BLOCKED"));
    assert!(blocked.contains("Conflicts:"));
}

#[test]
fn format_replay_state_text_and_json() {
    let units = vec![unit("feature", "feature.ts", 10)];

    let text = format_replay_state(&units, "text");
    assert!(text.contains("Replayed State (1)"));
    assert!(text.contains("feature"));

    let json = format_replay_state(&units, "json");
    let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid json");
    assert!(parsed.is_array());
}
