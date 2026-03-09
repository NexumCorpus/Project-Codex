//! Authoritative coordination types for Phase 2.
//!
//! These types are transcribed from the Implementation Specification §Phase 2.
//! They define the intent/lock protocol for semantic coordination between
//! concurrent AI agents. Codex must not alter these type signatures.

use crate::SemanticId;
use serde::{Deserialize, Serialize};

// ─────────────────────────────────────────────────────────────────────────────
// Agent Identity
// ─────────────────────────────────────────────────────────────────────────────

/// Unique identifier for an AI agent session.
///
/// 128-bit value, typically generated from a UUID. Each agent session gets
/// a unique AgentId so the coordination engine can track who holds what.
pub type AgentId = [u8; 16];

// ─────────────────────────────────────────────────────────────────────────────
// Intent (what an agent wants to do)
// ─────────────────────────────────────────────────────────────────────────────

/// What kind of access an agent needs on a semantic unit.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum IntentKind {
    /// Read-only access. Multiple agents can hold Read locks simultaneously.
    Read,
    /// Modify the unit's body or signature.
    Write,
    /// Delete the unit entirely.
    Delete,
}

/// A declared intent from an agent to interact with a semantic unit.
///
/// Intents are submitted to the `CoordinationEngine` which either grants
/// or denies them based on the current lock state and dependency graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Intent {
    /// Which agent is declaring this intent.
    pub agent_id: AgentId,
    /// Which semantic unit the agent wants to interact with.
    pub target: SemanticId,
    /// What kind of access the agent needs.
    pub kind: IntentKind,
}

// ─────────────────────────────────────────────────────────────────────────────
// Semantic Lock (a granted intent)
// ─────────────────────────────────────────────────────────────────────────────

/// A granted lock on a semantic unit.
///
/// Created when the `CoordinationEngine` grants an `Intent`. Remains active
/// until explicitly released by the agent or cleared by `release_all()`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticLock {
    /// The agent holding this lock.
    pub agent_id: AgentId,
    /// The locked semantic unit.
    pub target: SemanticId,
    /// The kind of access granted.
    pub kind: IntentKind,
}

// ─────────────────────────────────────────────────────────────────────────────
// Lock Result (outcome of a lock request)
// ─────────────────────────────────────────────────────────────────────────────

/// Result of a lock request.
///
/// Either the lock was granted, or it was denied with a list of conflicts
/// explaining why.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LockResult {
    /// Lock was granted. The agent can proceed with its intended operation.
    Granted,
    /// Lock was denied due to one or more conflicts.
    Denied {
        /// The conflicting locks that prevented this acquisition.
        conflicts: Vec<LockConflict>,
    },
}

/// A conflict between a requested lock and an existing lock or graph edge.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockConflict {
    /// The agent holding the conflicting lock.
    pub held_by: AgentId,
    /// The target unit of the conflicting lock.
    pub target: SemanticId,
    /// Human-readable reason for the conflict.
    pub reason: String,
}

// ─────────────────────────────────────────────────────────────────────────────
// Coordination State (snapshot)
// ─────────────────────────────────────────────────────────────────────────────

/// Snapshot of the coordination engine's state.
///
/// Used for monitoring, debugging, and CLI status output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoordinationState {
    /// All active locks.
    pub locks: Vec<SemanticLock>,
    /// Number of distinct agents with active locks.
    pub agent_count: usize,
}
