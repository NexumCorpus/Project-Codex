use crate::event::SemanticEvent;
use chrono::Utc;
use nex_core::{CodexError, CodexResult, SemanticId, SemanticUnit};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use uuid::Uuid;

const ENV_BACKEND: &str = "NEX_EVENTLOG_BACKEND";
const ENV_NATS_URL: &str = "NEX_NATS_URL";
const ENV_STREAM_BASE: &str = "NEX_EVENTLOG_STREAM";
const ENV_SUBJECT_BASE: &str = "NEX_EVENTLOG_SUBJECT_PREFIX";
const DEFAULT_NATS_URL: &str = "nats://127.0.0.1:4222";
const DEFAULT_STREAM_BASE: &str = "nex_events";
const DEFAULT_SUBJECT_BASE: &str = "nex.events";

#[derive(Debug, Clone)]
enum EventLogBackend {
    LocalFile { path: PathBuf },
    JetStream(JetStreamConfig),
}

#[derive(Debug, Clone)]
struct JetStreamConfig {
    server_url: String,
    stream_name: String,
    subject_prefix: String,
}

impl JetStreamConfig {
    fn for_repo(repo_path: &Path) -> Self {
        let repo_key = repo_key(repo_path);
        let stream_base = std::env::var(ENV_STREAM_BASE)
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_STREAM_BASE.to_string());
        let subject_base = std::env::var(ENV_SUBJECT_BASE)
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_SUBJECT_BASE.to_string());

        Self {
            server_url: std::env::var(ENV_NATS_URL)
                .ok()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| DEFAULT_NATS_URL.to_string()),
            stream_name: format!("{}_{}", sanitize_stream_name(&stream_base), repo_key),
            subject_prefix: format!("{}.{}", sanitize_subject_prefix(&subject_base), repo_key),
        }
    }
}

/// A rollback conflict caused by a later event touching the same unit.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RollbackConflict {
    pub unit: SemanticId,
    pub blocking_event: Uuid,
    pub reason: String,
}

/// Result of attempting to generate and append a rollback event.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RollbackOutcome {
    pub original_intent_id: Uuid,
    pub rollback_event: Option<SemanticEvent>,
    pub conflicts: Vec<RollbackConflict>,
}

impl RollbackOutcome {
    pub fn is_clean(&self) -> bool {
        self.conflicts.is_empty()
    }
}

/// Semantic event log with backend selection.
#[derive(Debug, Clone)]
pub struct EventLog {
    backend: EventLogBackend,
}

impl EventLog {
    /// Create an explicit local-file event log.
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self {
            backend: EventLogBackend::LocalFile {
                path: path.as_ref().to_path_buf(),
            },
        }
    }

    /// Create the selected event-log backend for a repository.
    ///
    /// Defaults to `.nex/events.json`. Set `NEX_EVENTLOG_BACKEND=jetstream`
    /// to publish events into a JetStream stream instead.
    pub fn for_repo(repo_path: &Path) -> Self {
        match std::env::var(ENV_BACKEND) {
            Ok(value) if value.eq_ignore_ascii_case("jetstream") => Self {
                backend: EventLogBackend::JetStream(JetStreamConfig::for_repo(repo_path)),
            },
            _ => Self::new(repo_path.join(".nex").join("events.json")),
        }
    }

    /// Human-readable backend identifier for diagnostics and tests.
    pub fn backend_name(&self) -> &'static str {
        match self.backend {
            EventLogBackend::LocalFile { .. } => "local-file",
            EventLogBackend::JetStream(_) => "jetstream",
        }
    }

    /// Append a semantic event to the configured event store.
    pub async fn append(&self, event: SemanticEvent) -> CodexResult<()> {
        match &self.backend {
            EventLogBackend::LocalFile { path } => {
                let mut events = load_events_from_path(path)?;
                events.push(event);
                save_events_to_path(path, &events)
            }
            EventLogBackend::JetStream(config) => append_jetstream_event(config, event).await,
        }
    }

    /// List all events ordered by timestamp.
    pub async fn list(&self) -> CodexResult<Vec<SemanticEvent>> {
        match &self.backend {
            EventLogBackend::LocalFile { path } => {
                let mut events = load_events_from_path(path)?;
                events.sort_by_key(|event| event.timestamp);
                Ok(events)
            }
            EventLogBackend::JetStream(config) => list_jetstream_events(config).await,
        }
    }

    /// All events emitted for a specific intent, ordered by timestamp.
    pub async fn events_for_intent(&self, intent_id: Uuid) -> CodexResult<Vec<SemanticEvent>> {
        let mut events: Vec<_> = self
            .list()
            .await?
            .into_iter()
            .filter(|event| event.intent_id == intent_id)
            .collect();
        events.sort_by_key(|event| event.timestamp);
        Ok(events)
    }

    /// Generate and append a rollback event if no later event touched the same units.
    pub async fn rollback(
        &self,
        intent_id: Uuid,
        agent_id: &str,
        description: &str,
    ) -> CodexResult<RollbackOutcome> {
        let events = self.list().await?;
        let target_events: Vec<_> = events
            .iter()
            .filter(|event| event.intent_id == intent_id)
            .cloned()
            .collect();
        if target_events.is_empty() {
            return Err(CodexError::Coordination(format!(
                "unknown intent: {intent_id}"
            )));
        }

        let touched_units = touched_units(&target_events);
        let last_target = target_events
            .last()
            .expect("checked non-empty target events");

        let conflicts: Vec<RollbackConflict> = events
            .iter()
            .filter(|event| event.timestamp > last_target.timestamp)
            .flat_map(|event| {
                event
                    .touched_units()
                    .into_iter()
                    .filter(|unit| touched_units.contains(unit))
                    .map(|unit| RollbackConflict {
                        unit,
                        blocking_event: event.id,
                        reason: format!(
                            "later event `{}` also touched rollback target",
                            event.description
                        ),
                    })
            })
            .collect();

        if !conflicts.is_empty() {
            return Ok(RollbackOutcome {
                original_intent_id: intent_id,
                rollback_event: None,
                conflicts,
            });
        }

        let mut compensating_mutations = Vec::new();
        for event in target_events.iter().rev() {
            for mutation in event.mutations.iter().rev() {
                compensating_mutations.push(mutation.compensate());
            }
        }

        let rollback_event = SemanticEvent {
            id: Uuid::new_v4(),
            timestamp: Utc::now(),
            intent_id: Uuid::new_v4(),
            agent_id: agent_id.to_string(),
            description: description.to_string(),
            mutations: compensating_mutations,
            parent_event: Some(last_target.id),
            tags: vec![format!("rollback:{intent_id}")],
        };

        self.append(rollback_event.clone()).await?;

        Ok(RollbackOutcome {
            original_intent_id: intent_id,
            rollback_event: Some(rollback_event),
            conflicts: Vec::new(),
        })
    }

    /// Rebuild semantic-unit state at a historical event boundary.
    pub async fn replay_to(&self, event_id: Uuid) -> CodexResult<Vec<SemanticUnit>> {
        let events = self.list().await?;
        let mut units = HashMap::new();
        let mut found = false;

        for event in events {
            for mutation in event.mutations {
                mutation.apply(&mut units);
            }
            if event.id == event_id {
                found = true;
                break;
            }
        }

        if !found {
            return Err(CodexError::Coordination(format!(
                "unknown event: {event_id}"
            )));
        }

        let mut ordered: Vec<_> = units.into_values().collect();
        ordered.sort_by(|left, right| left.qualified_name.cmp(&right.qualified_name));
        Ok(ordered)
    }
}

async fn append_jetstream_event(config: &JetStreamConfig, event: SemanticEvent) -> CodexResult<()> {
    let (jetstream, _) = ensure_stream(config).await?;
    let payload = serde_json::to_vec(&event)?;
    let subject = format!("{}.{}", config.subject_prefix, event.id);

    jetstream
        .publish(subject, payload.into())
        .await
        .map_err(jetstream_error)?;
    Ok(())
}

async fn list_jetstream_events(config: &JetStreamConfig) -> CodexResult<Vec<SemanticEvent>> {
    let (_, stream) = ensure_stream(config).await?;
    let message_count = stream.cached_info().state.messages;
    let mut events = Vec::new();

    for sequence in 1..=message_count {
        let message = stream
            .get_raw_message(sequence)
            .await
            .map_err(jetstream_error)?;
        let event: SemanticEvent = serde_json::from_slice(message.payload.as_ref())?;
        events.push(event);
    }

    events.sort_by_key(|event| event.timestamp);
    Ok(events)
}

async fn ensure_stream(
    config: &JetStreamConfig,
) -> CodexResult<(
    async_nats::jetstream::Context,
    async_nats::jetstream::stream::Stream,
)> {
    let client = async_nats::connect(&config.server_url)
        .await
        .map_err(jetstream_error)?;
    let jetstream = async_nats::jetstream::new(client);
    let stream = jetstream
        .get_or_create_stream(async_nats::jetstream::stream::Config {
            name: config.stream_name.clone(),
            subjects: vec![format!("{}.*", config.subject_prefix)],
            ..Default::default()
        })
        .await
        .map_err(jetstream_error)?;

    Ok((jetstream, stream))
}

fn load_events_from_path(path: &Path) -> CodexResult<Vec<SemanticEvent>> {
    if !path.exists() {
        return Ok(Vec::new());
    }

    let content = std::fs::read_to_string(path)?;
    Ok(serde_json::from_str(&content)?)
}

fn save_events_to_path(path: &Path, events: &[SemanticEvent]) -> CodexResult<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let content = serde_json::to_string_pretty(events)?;
    std::fs::write(path, content)?;
    Ok(())
}

fn touched_units(events: &[SemanticEvent]) -> Vec<SemanticId> {
    let mut touched = Vec::new();
    for event in events {
        for unit in event.touched_units() {
            if !touched.contains(&unit) {
                touched.push(unit);
            }
        }
    }
    touched
}

fn repo_key(repo_path: &Path) -> String {
    let canonical = repo_path
        .canonicalize()
        .unwrap_or_else(|_| repo_path.to_path_buf());
    let digest = blake3::hash(canonical.to_string_lossy().as_bytes());
    short_hex(digest.as_bytes(), 6)
}

fn short_hex(bytes: &[u8], len: usize) -> String {
    bytes
        .iter()
        .take(len)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn sanitize_stream_name(value: &str) -> String {
    value
        .chars()
        .map(|ch| match ch {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '_' | '-' => ch,
            _ => '_',
        })
        .collect()
}

fn sanitize_subject_prefix(value: &str) -> String {
    value
        .trim_matches('.')
        .split('.')
        .filter(|segment| !segment.is_empty())
        .map(|segment| {
            segment
                .chars()
                .map(|ch| match ch {
                    'a'..='z' | 'A'..='Z' | '0'..='9' | '_' | '-' => ch,
                    _ => '_',
                })
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join(".")
}

fn jetstream_error(err: impl std::fmt::Display) -> CodexError {
    CodexError::Coordination(format!("jetstream error: {err}"))
}
