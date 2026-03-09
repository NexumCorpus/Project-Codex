//! Semantic coordination engine - lock table with dependency-aware conflict detection.
//!
//! Implements the Phase 2 coordination protocol from the spec:
//! - Agents declare intents (Read/Write/Delete on a semantic unit)
//! - The engine grants or denies based on lock compatibility and graph edges
//! - Transitive conflicts detected via CodeGraph dependency/caller queries
//!
//! Lock compatibility matrix:
//! - Read  + Read   -> Compatible (multiple readers allowed)
//! - Read  + Write  -> Incompatible
//! - Read  + Delete -> Incompatible
//! - Write + Write  -> Incompatible
//! - Write + Delete -> Incompatible
//! - Delete + Delete -> Incompatible
//!
//! Transitive rule (Write/Delete only, different agents):
//! If agent A holds a Write/Delete lock on unit X, and agent B requests
//! a Write/Delete lock on unit Y where X and Y are directly connected
//! in the dependency graph (either direction), the request is denied.

use nex_core::{
    AgentId, CodexError, CodexResult, CoordinationState, Intent, IntentKind, LockConflict,
    LockResult, SemanticId, SemanticLock, SemanticUnit,
};
use nex_graph::CodeGraph;
use std::collections::{HashMap, HashSet};

/// Semantic coordination engine.
///
/// Manages a lock table for semantic units, using the `CodeGraph` to detect
/// transitive conflicts through dependency edges. Stateless with respect to
/// networking - the engine is a pure in-memory data structure.
pub struct CoordinationEngine {
    /// Active locks indexed by target SemanticId.
    locks: HashMap<SemanticId, Vec<SemanticLock>>,
    /// The code graph used for transitive conflict detection.
    graph: CodeGraph,
}

impl CoordinationEngine {
    /// Create a new coordination engine with the given code graph.
    ///
    /// The graph is used for transitive conflict detection: when an agent
    /// requests a Write/Delete lock, the engine checks whether any directly
    /// connected units (callers or dependencies) are Write/Delete-locked
    /// by a different agent.
    pub fn new(graph: CodeGraph) -> Self {
        Self {
            locks: HashMap::new(),
            graph,
        }
    }

    /// Request a lock on a semantic unit.
    ///
    /// Returns `LockResult::Granted` if the lock is compatible with all
    /// existing locks, or `LockResult::Denied { conflicts }` if not.
    ///
    /// Algorithm:
    /// 1. If the same agent already holds a lock on this target -> Denied
    /// 2. For each existing lock on this target from a different agent,
    ///    check compatibility. If incompatible -> collect into conflicts.
    /// 3. If the request is Write or Delete, check transitive conflicts:
    ///    get `graph.callers_of(target)` and `graph.deps_of(target)`,
    ///    and for each related unit, check if a *different* agent holds
    ///    a Write or Delete lock on it. If so -> collect into conflicts.
    /// 4. If conflicts is empty -> insert a new SemanticLock and return Granted.
    ///    Otherwise -> return Denied with the collected conflicts.
    pub fn request_lock(&mut self, intent: Intent) -> LockResult {
        if self
            .locks
            .get(&intent.target)
            .is_some_and(|locks| locks.iter().any(|lock| lock.agent_id == intent.agent_id))
        {
            return LockResult::Denied {
                conflicts: vec![LockConflict {
                    held_by: intent.agent_id,
                    target: intent.target,
                    reason: "agent already holds a lock on this unit".to_string(),
                }],
            };
        }

        let mut conflicts = Vec::new();

        if let Some(existing_locks) = self.locks.get(&intent.target) {
            for existing in existing_locks
                .iter()
                .filter(|lock| lock.agent_id != intent.agent_id)
            {
                if !compatible(existing.kind, intent.kind) {
                    conflicts.push(LockConflict {
                        held_by: existing.agent_id,
                        target: existing.target,
                        reason: format!(
                            "{:?} lock conflicts with requested {:?}",
                            existing.kind, intent.kind
                        ),
                    });
                }
            }
        }

        if matches!(intent.kind, IntentKind::Write | IntentKind::Delete) {
            for related_unit in self
                .graph
                .callers_of(&intent.target)
                .into_iter()
                .chain(self.graph.deps_of(&intent.target))
            {
                if let Some(existing_locks) = self.locks.get(&related_unit.id) {
                    for existing in existing_locks.iter().filter(|lock| {
                        lock.agent_id != intent.agent_id
                            && matches!(lock.kind, IntentKind::Write | IntentKind::Delete)
                    }) {
                        conflicts.push(LockConflict {
                            held_by: existing.agent_id,
                            target: related_unit.id,
                            reason: format!(
                                "transitive conflict: {:?} lock on related unit `{}`",
                                existing.kind, related_unit.name
                            ),
                        });
                    }
                }
            }
        }

        if conflicts.is_empty() {
            self.locks
                .entry(intent.target)
                .or_default()
                .push(SemanticLock {
                    agent_id: intent.agent_id,
                    target: intent.target,
                    kind: intent.kind,
                });
            LockResult::Granted
        } else {
            LockResult::Denied { conflicts }
        }
    }

    /// Release a specific lock held by an agent on a target.
    ///
    /// Returns `Err(CodexError::Coordination(...))` if the agent does not
    /// hold a lock on the given target.
    pub fn release_lock(&mut self, agent_id: &AgentId, target: &SemanticId) -> CodexResult<()> {
        let Some(existing_locks) = self.locks.get_mut(target) else {
            return Err(CodexError::Coordination(
                "agent does not hold a lock on this unit".to_string(),
            ));
        };

        let before_len = existing_locks.len();
        existing_locks.retain(|lock| &lock.agent_id != agent_id);
        let removed = existing_locks.len() != before_len;
        let should_remove_entry = existing_locks.is_empty();

        if should_remove_entry {
            self.locks.remove(target);
        }

        if removed {
            Ok(())
        } else {
            Err(CodexError::Coordination(
                "agent does not hold a lock on this unit".to_string(),
            ))
        }
    }

    /// Release all locks held by an agent across all targets.
    pub fn release_all(&mut self, agent_id: &AgentId) {
        self.locks.retain(|_, locks| {
            locks.retain(|lock| &lock.agent_id != agent_id);
            !locks.is_empty()
        });
    }

    /// Get all active locks across all targets and agents.
    pub fn active_locks(&self) -> Vec<&SemanticLock> {
        self.locks.values().flat_map(|locks| locks.iter()).collect()
    }

    /// Get all locks on a specific target unit.
    pub fn locks_for_unit(&self, target: &SemanticId) -> Vec<&SemanticLock> {
        self.locks
            .get(target)
            .map(|locks| locks.iter().collect())
            .unwrap_or_default()
    }

    /// Get all locks held by a specific agent (across all targets).
    pub fn locks_for_agent(&self, agent_id: &AgentId) -> Vec<&SemanticLock> {
        self.locks
            .values()
            .flat_map(|locks| locks.iter())
            .filter(|lock| &lock.agent_id == agent_id)
            .collect()
    }

    /// Look up a semantic unit in the underlying graph.
    pub fn get_unit(&self, id: &SemanticId) -> Option<&SemanticUnit> {
        self.graph.get(id)
    }

    /// Return all semantic units currently known to the graph.
    pub fn units(&self) -> Vec<&SemanticUnit> {
        self.graph.units()
    }

    /// Return all callers of the given target.
    pub fn callers_of(&self, target: &SemanticId) -> Vec<&SemanticUnit> {
        self.graph.callers_of(target)
    }

    /// Return all dependencies of the given target.
    pub fn deps_of(&self, target: &SemanticId) -> Vec<&SemanticUnit> {
        self.graph.deps_of(target)
    }

    /// Export all active locks for persistence.
    ///
    /// Returns a cloned vec of every `SemanticLock` in the lock table.
    /// Used by the CLI pipeline to serialize lock state to disk.
    pub fn export_locks(&self) -> Vec<SemanticLock> {
        self.locks
            .values()
            .flat_map(|locks| locks.iter().cloned())
            .collect()
    }

    /// Import locks from a persisted state, replacing the current lock table.
    ///
    /// Clears all existing locks, then populates the internal HashMap from
    /// the provided vec. This bypasses `request_lock` validation — it directly
    /// inserts locks. Use only for restoring previously granted locks from disk.
    pub fn import_locks(&mut self, locks: Vec<SemanticLock>) {
        self.locks.clear();
        for lock in locks {
            self.locks.entry(lock.target).or_default().push(lock);
        }
    }

    /// Get a snapshot of the coordination state.
    ///
    /// `agent_count` is the number of distinct agents with at least one active lock.
    pub fn state(&self) -> CoordinationState {
        let all_locks: Vec<SemanticLock> = self
            .locks
            .values()
            .flat_map(|locks| locks.iter().cloned())
            .collect();
        let agent_count = all_locks
            .iter()
            .map(|lock| lock.agent_id)
            .collect::<HashSet<_>>()
            .len();

        CoordinationState {
            locks: all_locks,
            agent_count,
        }
    }
}

fn compatible(a: IntentKind, b: IntentKind) -> bool {
    matches!((a, b), (IntentKind::Read, IntentKind::Read))
}
