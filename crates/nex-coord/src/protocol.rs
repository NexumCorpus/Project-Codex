//! Coordination protocol types for the Phase 2 server/API layer.
//!
//! These types mirror the implementation-spec intent declaration flow and
//! are used by the HTTP server, tests, and any future client SDK.

use chrono::{DateTime, Utc};
use nex_core::{SemanticId, UnitKind};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use uuid::Uuid;

/// Intent declaration payload submitted by an agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntentPayload {
    /// Stable intent id supplied by the caller.
    pub id: Uuid,
    /// Human-readable agent/session identifier.
    pub agent_id: String,
    /// Client-side declaration timestamp.
    pub timestamp: DateTime<Utc>,
    /// Human-readable description of the planned change.
    pub description: String,
    /// Semantic units the agent expects to touch.
    pub target_units: Vec<SemanticId>,
    /// Planned changes used to determine lock strength.
    pub estimated_changes: Vec<PlannedChange>,
    /// How long the declaration remains valid without commit/abort.
    pub ttl: Duration,
}

/// Planned semantic change kinds from the implementation plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PlannedChange {
    /// Modify the implementation body of an existing unit.
    ModifyBody { unit: SemanticId },
    /// Change a callable signature.
    ModifySignature {
        unit: SemanticId,
        new_params: Vec<String>,
    },
    /// Add a new unit under a parent scope.
    AddUnit {
        parent: SemanticId,
        kind: UnitKind,
        name: String,
    },
    /// Remove an existing unit.
    RemoveUnit { unit: SemanticId },
    /// Move a unit to a different parent scope.
    MoveUnit {
        unit: SemanticId,
        new_parent: SemanticId,
    },
    /// Rename an existing unit.
    RenameUnit { unit: SemanticId, new_name: String },
}

/// Result of an intent declaration request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum IntentResult {
    /// Locks were acquired successfully.
    Approved {
        /// Token required for commit/abort.
        lock_token: Uuid,
        /// Time at which the locks expire automatically.
        expires: DateTime<Utc>,
    },
    /// Intent was queued behind another holder.
    Queued {
        /// Position in the queue.
        position: usize,
        /// Estimated wait before the queue can advance.
        estimated_wait: Duration,
    },
    /// Intent conflicts with current lock ownership.
    Rejected {
        /// All discovered blocking conflicts.
        conflicts: Vec<IntentConflict>,
    },
}

/// A blocking reason for a rejected intent declaration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntentConflict {
    /// Blocking intent id, if known.
    pub blocking_intent: Uuid,
    /// Blocking agent identifier.
    pub blocking_agent: String,
    /// Contested semantic unit.
    pub contested_unit: SemanticId,
    /// Human-readable conflict explanation.
    pub reason: String,
}

/// Lock strength exposed by the server API.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum LockKind {
    /// Body/signature/delete changes.
    Exclusive,
    /// Read-only or additive scope access.
    Shared,
}

/// Serializable snapshot of an active semantic lock.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockEntry {
    /// Locked semantic unit.
    pub target: SemanticId,
    /// Human-readable qualified or short name for the target.
    pub target_name: String,
    /// Agent currently holding the lock.
    pub holder: String,
    /// Intent that owns the lock.
    pub intent_id: Uuid,
    /// API-level lock strength.
    pub lock_kind: LockKind,
    /// Acquisition time.
    pub acquired: DateTime<Utc>,
    /// Expiration time.
    pub expires: DateTime<Utc>,
}

/// Query kinds supported by the live semantic graph endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphQueryKind {
    /// Return all callers of the named target.
    CallersOf,
    /// Return all dependencies of the named target.
    DepsOf,
    /// Return the exact unit matching the provided name.
    UnitsNamed,
}

/// Request for `/graph/query`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphQuery {
    /// Query operation to execute.
    pub kind: GraphQueryKind,
    /// Unit name or qualified name to resolve first.
    pub value: String,
}

/// Coordination events published to websocket subscribers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CoordEvent {
    /// A new intent was declared and approved.
    IntentDeclared {
        intent_id: Uuid,
        agent_id: String,
        description: String,
        targets: Vec<SemanticId>,
        expires: DateTime<Utc>,
    },
    /// An intent declaration was rejected.
    IntentRejected {
        intent_id: Uuid,
        agent_id: String,
        conflicts: Vec<IntentConflict>,
    },
    /// An approved intent committed and released its locks.
    IntentCommitted {
        intent_id: Uuid,
        agent_id: String,
        event_id: Option<Uuid>,
        released_locks: usize,
    },
    /// An approved intent was aborted.
    IntentAborted {
        intent_id: Uuid,
        agent_id: String,
        released_locks: usize,
    },
    /// One or more intents expired automatically.
    LocksExpired { intent_ids: Vec<Uuid> },
}
