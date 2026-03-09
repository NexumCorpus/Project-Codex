//! Async coordination server pipeline for `nex serve`.

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Utc;
use nex_coord::{CoordEvent, CoordinationService, GraphQuery, IntentPayload, IntentResult};
use nex_core::{CodexError, CodexResult};
use nex_eventlog::{EventLog, Mutation, SemanticEvent};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::{Mutex, broadcast, watch};
use tokio::task::JoinHandle;
use uuid::Uuid;

#[derive(Clone)]
struct AppState {
    service: Arc<Mutex<CoordinationService>>,
    events: broadcast::Sender<CoordEvent>,
    event_log: EventLog,
}

/// Request body for `/intent/commit`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitRequest {
    /// Intent id to commit.
    pub intent_id: Uuid,
    /// Lock token returned by `declare`.
    pub lock_token: Uuid,
    /// Optional commit description override.
    pub description: Option<String>,
    /// Semantic mutations emitted by the agent.
    pub mutations: Vec<Mutation>,
    /// Optional causal parent event.
    pub parent_event: Option<Uuid>,
    /// Free-form event tags.
    pub tags: Vec<String>,
}

/// Response body for `/intent/commit`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitResponse {
    /// Committed intent id.
    pub intent_id: Uuid,
    /// Event id appended to the local event log.
    pub event_id: Uuid,
    /// Number of locks released by commit.
    pub released_locks: usize,
}

/// Request body for `/intent/abort`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AbortRequest {
    /// Intent id to abort.
    pub intent_id: Uuid,
    /// Lock token returned by `declare`.
    pub lock_token: Uuid,
}

/// Response body for `/intent/abort`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AbortResponse {
    /// Aborted intent id.
    pub intent_id: Uuid,
    /// Number of locks released by abort.
    pub released_locks: usize,
}

/// Running server handle used by tests and the CLI.
pub struct ServerHandle {
    local_addr: SocketAddr,
    shutdown: watch::Sender<bool>,
    join: JoinHandle<()>,
}

impl ServerHandle {
    /// Actual bound address, useful when binding to port 0 in tests.
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Stop the server and wait for all tasks to exit.
    pub async fn shutdown(self) {
        let _ = self.shutdown.send(true);
        let _ = self.join.await;
    }
}

/// Start the coordination server and keep it alive until Ctrl-C.
pub async fn run_serve(repo_path: &Path, host: &str, port: u16) -> CodexResult<()> {
    let bind_addr: SocketAddr = format!("{host}:{port}").parse().map_err(|err| {
        CodexError::Coordination(format!("invalid bind address {host}:{port}: {err}"))
    })?;

    let handle = spawn_server(repo_path, bind_addr).await?;
    println!("nex serve listening on http://{}", handle.local_addr());
    tokio::signal::ctrl_c().await?;
    handle.shutdown().await;
    Ok(())
}

/// Spawn the coordination server on the requested address.
pub async fn spawn_server(repo_path: &Path, bind_addr: SocketAddr) -> CodexResult<ServerHandle> {
    let graph = crate::coordination_pipeline::build_graph_from_head(repo_path)?;
    let service = CoordinationService::new(graph);
    let (events, _) = broadcast::channel(128);
    let state = AppState {
        service: Arc::new(Mutex::new(service)),
        events,
        event_log: EventLog::for_repo(repo_path),
    };

    let app = Router::new()
        .route("/intent/declare", post(declare_intent))
        .route("/intent/commit", post(commit_intent))
        .route("/intent/abort", post(abort_intent))
        .route("/graph/query", get(query_graph))
        .route("/locks", get(list_locks))
        .route("/events", get(events_socket))
        .with_state(state.clone());

    let listener = TcpListener::bind(bind_addr).await?;
    let local_addr = listener.local_addr()?;
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let mut expiry_shutdown = shutdown_rx.clone();
    let expiry_state = state.clone();
    let expiry_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_millis(250));
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    let expired = {
                        let mut service = expiry_state.service.lock().await;
                        service.expire_stale()
                    };
                    if !expired.is_empty() {
                        let intent_ids = expired.into_iter().map(|intent| intent.intent_id).collect();
                        let _ = expiry_state.events.send(CoordEvent::LocksExpired { intent_ids });
                    }
                }
                changed = expiry_shutdown.changed() => {
                    if changed.is_err() || *expiry_shutdown.borrow() {
                        break;
                    }
                }
            }
        }
    });

    let mut server_shutdown = shutdown_rx.clone();
    let join = tokio::spawn(async move {
        let server = axum::serve(listener, app).with_graceful_shutdown(async move {
            let _ = server_shutdown.changed().await;
        });
        let _ = server.await;
        expiry_task.abort();
        let _ = expiry_task.await;
    });

    Ok(ServerHandle {
        local_addr,
        shutdown: shutdown_tx,
        join,
    })
}

async fn declare_intent(
    State(state): State<AppState>,
    Json(payload): Json<IntentPayload>,
) -> Result<Json<IntentResult>, ApiError> {
    let mut service = state.service.lock().await;
    let result = service.declare_intent(payload.clone())?;
    drop(service);

    match &result {
        IntentResult::Approved { expires, .. } => {
            let _ = state.events.send(CoordEvent::IntentDeclared {
                intent_id: payload.id,
                agent_id: payload.agent_id,
                description: payload.description,
                targets: payload.target_units,
                expires: *expires,
            });
        }
        IntentResult::Rejected { conflicts } => {
            let _ = state.events.send(CoordEvent::IntentRejected {
                intent_id: payload.id,
                agent_id: payload.agent_id,
                conflicts: conflicts.clone(),
            });
        }
        IntentResult::Queued { .. } => {}
    }

    Ok(Json(result))
}

async fn commit_intent(
    State(state): State<AppState>,
    Json(request): Json<CommitRequest>,
) -> Result<Json<CommitResponse>, ApiError> {
    let context = {
        let mut service = state.service.lock().await;
        service.commit_intent(request.intent_id, request.lock_token)?
    };

    let event = SemanticEvent {
        id: Uuid::new_v4(),
        timestamp: Utc::now(),
        intent_id: context.intent_id,
        agent_id: context.agent_id.clone(),
        description: request.description.unwrap_or(context.description),
        mutations: request.mutations,
        parent_event: request.parent_event,
        tags: request.tags,
    };

    state.event_log.append(event.clone()).await?;
    let _ = state.events.send(CoordEvent::IntentCommitted {
        intent_id: context.intent_id,
        agent_id: context.agent_id,
        event_id: Some(event.id),
        released_locks: context.released_locks,
    });

    Ok(Json(CommitResponse {
        intent_id: context.intent_id,
        event_id: event.id,
        released_locks: context.released_locks,
    }))
}

async fn abort_intent(
    State(state): State<AppState>,
    Json(request): Json<AbortRequest>,
) -> Result<Json<AbortResponse>, ApiError> {
    let context = {
        let mut service = state.service.lock().await;
        service.abort_intent(request.intent_id, request.lock_token)?
    };

    let _ = state.events.send(CoordEvent::IntentAborted {
        intent_id: context.intent_id,
        agent_id: context.agent_id,
        released_locks: context.released_locks,
    });

    Ok(Json(AbortResponse {
        intent_id: context.intent_id,
        released_locks: context.released_locks,
    }))
}

async fn query_graph(
    State(state): State<AppState>,
    Query(query): Query<GraphQuery>,
) -> Result<Json<Vec<nex_core::SemanticUnit>>, ApiError> {
    let service = state.service.lock().await;
    Ok(Json(service.query_graph(&query)?))
}

async fn list_locks(State(state): State<AppState>) -> Json<Vec<nex_coord::LockEntry>> {
    let service = state.service.lock().await;
    Json(service.locks())
}

async fn events_socket(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.on_upgrade(move |socket| stream_events(socket, state.events.subscribe()))
}

async fn stream_events(mut socket: WebSocket, mut receiver: broadcast::Receiver<CoordEvent>) {
    loop {
        match receiver.recv().await {
            Ok(event) => {
                let Ok(payload) = serde_json::to_string(&event) else {
                    continue;
                };
                if socket.send(Message::Text(payload.into())).await.is_err() {
                    break;
                }
            }
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
}

struct ApiError(CodexError);

impl From<CodexError> for ApiError {
    fn from(value: CodexError) -> Self {
        Self(value)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match self.0 {
            CodexError::Io(_) | CodexError::Serialization(_) => StatusCode::INTERNAL_SERVER_ERROR,
            CodexError::Git(_)
            | CodexError::Parse { .. }
            | CodexError::Graph(_)
            | CodexError::Coordination(_) => StatusCode::BAD_REQUEST,
        };

        let body = serde_json::json!({
            "error": self.0.to_string(),
        });
        (status, Json(body)).into_response()
    }
}
