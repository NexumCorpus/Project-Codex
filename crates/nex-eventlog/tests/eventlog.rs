use chrono::{TimeZone, Utc};
use nex_core::{SemanticUnit, UnitKind};
use nex_eventlog::{EventLog, Mutation, SemanticEvent};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use uuid::Uuid;

fn make_unit(name: &str, file: &str, body_hash: u64) -> SemanticUnit {
    let id = *blake3::hash(format!("{name}:{file}").as_bytes()).as_bytes();
    SemanticUnit {
        id,
        kind: UnitKind::Function,
        name: name.to_string(),
        qualified_name: name.to_string(),
        file_path: PathBuf::from(file),
        byte_range: 0..100,
        signature_hash: 42,
        body_hash,
        dependencies: Vec::new(),
    }
}

fn temp_log_path() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("nex-eventlog-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("create temp eventlog dir");
    dir.join("events.json")
}

fn event(
    event_id: Uuid,
    intent_id: Uuid,
    timestamp_secs: i64,
    description: &str,
    mutations: Vec<Mutation>,
) -> SemanticEvent {
    SemanticEvent {
        id: event_id,
        timestamp: Utc.timestamp_opt(timestamp_secs, 0).single().unwrap(),
        intent_id,
        agent_id: "alice".to_string(),
        description: description.to_string(),
        mutations,
        parent_event: None,
        tags: Vec::new(),
    }
}

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn clear_eventlog_env() {
    unsafe {
        std::env::remove_var("NEX_EVENTLOG_BACKEND");
        std::env::remove_var("NEX_NATS_URL");
        std::env::remove_var("NEX_EVENTLOG_STREAM");
        std::env::remove_var("NEX_EVENTLOG_SUBJECT_PREFIX");
    }
}

#[test]
fn compensate_reverses_all_mutation_variants() {
    let before = make_unit("validate", "handler.ts", 10);
    let after = make_unit("validate", "handler.ts", 20);

    let add = Mutation::AddUnit {
        unit: before.clone(),
    };
    assert_eq!(
        add.compensate(),
        Mutation::RemoveUnit {
            id: before.id,
            snapshot: before.clone()
        }
    );

    let remove = Mutation::RemoveUnit {
        id: before.id,
        snapshot: before.clone(),
    };
    assert_eq!(
        remove.compensate(),
        Mutation::AddUnit {
            unit: before.clone()
        }
    );

    let modify = Mutation::ModifyUnit {
        id: before.id,
        before: before.clone(),
        after: after.clone(),
    };
    assert_eq!(
        modify.compensate(),
        Mutation::ModifyUnit {
            id: before.id,
            before: after.clone(),
            after: before.clone(),
        }
    );

    let move_unit = Mutation::MoveUnit {
        id: before.id,
        from: PathBuf::from("old.ts"),
        to: PathBuf::from("new.ts"),
    };
    assert_eq!(
        move_unit.compensate(),
        Mutation::MoveUnit {
            id: before.id,
            from: PathBuf::from("new.ts"),
            to: PathBuf::from("old.ts"),
        }
    );

    let rename = Mutation::RenameUnit {
        id: before.id,
        from: "oldName".to_string(),
        to: "newName".to_string(),
    };
    assert_eq!(
        rename.compensate(),
        Mutation::RenameUnit {
            id: before.id,
            from: "newName".to_string(),
            to: "oldName".to_string(),
        }
    );
}

#[test]
fn for_repo_uses_local_backend_by_default() {
    let _guard = env_lock().lock().unwrap();
    clear_eventlog_env();

    let repo = std::env::temp_dir().join(format!("nex-eventlog-repo-{}", Uuid::new_v4()));
    let log = EventLog::for_repo(Path::new(&repo));
    assert_eq!(log.backend_name(), "local-file");
}

#[test]
fn for_repo_selects_jetstream_backend_when_enabled() {
    let _guard = env_lock().lock().unwrap();
    clear_eventlog_env();
    unsafe {
        std::env::set_var("NEX_EVENTLOG_BACKEND", "jetstream");
        std::env::set_var("NEX_NATS_URL", "nats://127.0.0.1:4222");
    }

    let repo = std::env::temp_dir().join(format!("nex-eventlog-repo-{}", Uuid::new_v4()));
    let log = EventLog::for_repo(Path::new(&repo));
    assert_eq!(log.backend_name(), "jetstream");

    clear_eventlog_env();
}

#[tokio::test(flavor = "current_thread")]
async fn append_and_list_round_trip() {
    let log = EventLog::new(temp_log_path());
    let intent_id = Uuid::new_v4();
    let unit = make_unit("validate", "handler.ts", 10);

    let first = event(
        Uuid::new_v4(),
        intent_id,
        10,
        "add validate",
        vec![Mutation::AddUnit { unit: unit.clone() }],
    );
    let second = event(
        Uuid::new_v4(),
        intent_id,
        20,
        "move validate",
        vec![Mutation::MoveUnit {
            id: unit.id,
            from: PathBuf::from("handler.ts"),
            to: PathBuf::from("api/handler.ts"),
        }],
    );

    log.append(second.clone()).await.unwrap();
    log.append(first.clone()).await.unwrap();

    let events = log.list().await.unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].id, first.id);
    assert_eq!(events[1].id, second.id);
}

#[tokio::test(flavor = "current_thread")]
async fn events_for_intent_filters_and_orders() {
    let log = EventLog::new(temp_log_path());
    let intent_a = Uuid::new_v4();
    let intent_b = Uuid::new_v4();
    let unit = make_unit("validate", "handler.ts", 10);

    log.append(event(
        Uuid::new_v4(),
        intent_b,
        30,
        "other intent",
        vec![Mutation::AddUnit { unit: unit.clone() }],
    ))
    .await
    .unwrap();
    let first = event(
        Uuid::new_v4(),
        intent_a,
        10,
        "intent a first",
        vec![Mutation::AddUnit { unit: unit.clone() }],
    );
    let second = event(
        Uuid::new_v4(),
        intent_a,
        20,
        "intent a second",
        vec![Mutation::RenameUnit {
            id: unit.id,
            from: "validate".to_string(),
            to: "auth::validate".to_string(),
        }],
    );
    log.append(second.clone()).await.unwrap();
    log.append(first.clone()).await.unwrap();

    let events = log.events_for_intent(intent_a).await.unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].id, first.id);
    assert_eq!(events[1].id, second.id);
}

#[tokio::test(flavor = "current_thread")]
async fn replay_to_rebuilds_historical_state() {
    let log = EventLog::new(temp_log_path());
    let intent_id = Uuid::new_v4();
    let before = make_unit("validate", "handler.ts", 10);
    let after = make_unit("validate", "handler.ts", 20);
    let helper = make_unit("helper", "utils.ts", 30);

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

    let units = log.replay_to(second.id).await.unwrap();
    assert_eq!(units.len(), 1);
    assert_eq!(units[0].qualified_name, "validate");
    assert_eq!(units[0].body_hash, after.body_hash);
}

#[tokio::test(flavor = "current_thread")]
async fn rollback_appends_compensating_event_when_clean() {
    let log = EventLog::new(temp_log_path());
    let intent_id = Uuid::new_v4();
    let unit = make_unit("featureFn", "feature.ts", 10);

    let original = event(
        Uuid::new_v4(),
        intent_id,
        10,
        "add feature",
        vec![Mutation::AddUnit { unit: unit.clone() }],
    );
    log.append(original.clone()).await.unwrap();

    let outcome = log
        .rollback(intent_id, "system", "rollback feature")
        .await
        .unwrap();
    assert!(outcome.is_clean());
    let rollback_event = outcome.rollback_event.expect("rollback event");
    assert_eq!(rollback_event.parent_event, Some(original.id));
    assert_eq!(rollback_event.tags, vec![format!("rollback:{intent_id}")]);
    assert_eq!(
        rollback_event.mutations,
        vec![Mutation::RemoveUnit {
            id: unit.id,
            snapshot: unit.clone(),
        }]
    );

    let replayed = log.replay_to(rollback_event.id).await.unwrap();
    assert!(
        replayed.is_empty(),
        "rollback should remove the added feature"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn rollback_reports_conflict_when_later_event_touches_same_unit() {
    let log = EventLog::new(temp_log_path());
    let intent_a = Uuid::new_v4();
    let intent_b = Uuid::new_v4();
    let before = make_unit("validate", "handler.ts", 10);
    let after = make_unit("validate", "handler.ts", 20);

    log.append(event(
        Uuid::new_v4(),
        intent_a,
        10,
        "add validate",
        vec![Mutation::AddUnit {
            unit: before.clone(),
        }],
    ))
    .await
    .unwrap();
    log.append(event(
        Uuid::new_v4(),
        intent_b,
        20,
        "modify validate later",
        vec![Mutation::ModifyUnit {
            id: before.id,
            before: before.clone(),
            after: after.clone(),
        }],
    ))
    .await
    .unwrap();

    let outcome = log
        .rollback(intent_a, "system", "rollback validate")
        .await
        .unwrap();
    assert!(!outcome.is_clean());
    assert!(outcome.rollback_event.is_none());
    assert_eq!(outcome.conflicts.len(), 1);
    assert_eq!(outcome.conflicts[0].unit, before.id);

    let events = log.list().await.unwrap();
    assert_eq!(
        events.len(),
        2,
        "conflicting rollback must not append an event"
    );
}
