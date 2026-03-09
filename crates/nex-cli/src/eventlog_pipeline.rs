//! Eventlog pipeline: selected event store -> list / rollback.

use nex_core::{CodexError, CodexResult, SemanticUnit};
use nex_eventlog::{EventLog, RollbackOutcome, SemanticEvent};
use std::path::Path;
use uuid::Uuid;

/// List semantic events, optionally filtered by intent id.
pub async fn run_log(repo_path: &Path, intent_id: Option<&str>) -> CodexResult<Vec<SemanticEvent>> {
    let log = EventLog::for_repo(repo_path);
    match intent_id {
        Some(intent_id) => {
            let parsed = parse_uuid(intent_id)?;
            log.events_for_intent(parsed).await
        }
        None => log.list().await,
    }
}

/// Generate and append a rollback event for the given intent id.
pub async fn run_rollback(
    repo_path: &Path,
    intent_id: &str,
    agent_name: &str,
) -> CodexResult<RollbackOutcome> {
    let parsed = parse_uuid(intent_id)?;
    let log = EventLog::for_repo(repo_path);
    log.rollback(parsed, agent_name, &format!("rollback {intent_id}"))
        .await
}

/// Replay semantic state to the given event boundary.
pub async fn run_replay(repo_path: &Path, event_id: &str) -> CodexResult<Vec<SemanticUnit>> {
    let parsed = parse_uuid(event_id)?;
    let log = EventLog::for_repo(repo_path);
    log.replay_to(parsed).await
}

fn parse_uuid(value: &str) -> CodexResult<Uuid> {
    Uuid::parse_str(value)
        .map_err(|_| CodexError::Coordination(format!("invalid intent id: {value}")))
}
