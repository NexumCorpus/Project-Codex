//! In-memory coordination service built on top of `CoordinationEngine`.
//!
//! The engine owns low-level lock compatibility and transitive dependency
//! checks. This service adds the protocol-facing concepts that the Phase 2
//! server needs: intent ids, lock tokens, TTL expiry, graph queries, and
//! serializable lock snapshots.

use crate::coordinator::CoordinationEngine;
use crate::crdt::{CoordinationDocument, CrdtHeldLock, CrdtIntentRecord};
use crate::protocol::{
    GraphQuery, GraphQueryKind, IntentConflict, IntentPayload, IntentResult, LockEntry, LockKind,
    PlannedChange,
};
use chrono::{DateTime, Utc};
use nex_core::{
    AgentId, CodexError, CodexResult, Intent, IntentKind, LockConflict, LockResult, SemanticId,
    SemanticUnit,
};
use nex_graph::CodeGraph;
use std::collections::HashMap;
use uuid::Uuid;

#[derive(Debug, Clone)]
struct HeldLock {
    target: SemanticId,
    kind: IntentKind,
}

#[derive(Debug, Clone)]
struct ActiveIntent {
    payload: IntentPayload,
    hashed_agent_id: AgentId,
    lock_token: Uuid,
    acquired: DateTime<Utc>,
    expires: DateTime<Utc>,
    held_locks: Vec<HeldLock>,
}

/// Commit metadata returned after a successful intent commit.
#[derive(Debug, Clone)]
pub struct CommitContext {
    /// Committed intent id.
    pub intent_id: Uuid,
    /// Agent responsible for the commit.
    pub agent_id: String,
    /// Original intent description.
    pub description: String,
    /// Number of locks released by the commit.
    pub released_locks: usize,
}

/// Abort metadata returned after a successful intent abort.
#[derive(Debug, Clone)]
pub struct AbortContext {
    /// Aborted intent id.
    pub intent_id: Uuid,
    /// Agent responsible for the abort.
    pub agent_id: String,
    /// Number of locks released by the abort.
    pub released_locks: usize,
}

/// Metadata about intents released by TTL expiry.
#[derive(Debug, Clone)]
pub struct ExpiredIntent {
    /// Expired intent id.
    pub intent_id: Uuid,
    /// Expired agent identifier.
    pub agent_id: String,
    /// Number of locks released by expiry.
    pub released_locks: usize,
}

/// Protocol-facing coordination service.
pub struct CoordinationService {
    engine: CoordinationEngine,
    intents: HashMap<Uuid, ActiveIntent>,
    replica: CoordinationDocument,
}

impl CoordinationService {
    /// Create a new service from a code graph snapshot.
    pub fn new(graph: CodeGraph) -> Self {
        Self::new_with_peer(graph, random_peer_id())
    }

    /// Create a new service with an explicit CRDT peer id.
    pub fn new_with_peer(graph: CodeGraph, peer_id: u64) -> Self {
        Self {
            engine: CoordinationEngine::new(graph),
            intents: HashMap::new(),
            replica: CoordinationDocument::new(peer_id)
                .expect("coordination CRDT document should initialize"),
        }
    }

    /// Export the service CRDT state for replica sync.
    pub fn export_crdt(&self) -> CodexResult<Vec<u8>> {
        self.replica.export_bytes()
    }

    /// Merge remote CRDT updates and rebuild the in-memory lock engine.
    pub fn merge_crdt(&mut self, bytes: &[u8]) -> CodexResult<()> {
        self.replica.merge_bytes(bytes)?;
        self.rebuild_from_replica()?;
        let _ = self.expire_stale();
        Ok(())
    }

    /// Declare an intent, acquire all required locks, and return the result.
    pub fn declare_intent(&mut self, payload: IntentPayload) -> CodexResult<IntentResult> {
        let _ = self.expire_stale();

        if self.intents.contains_key(&payload.id) {
            return Err(CodexError::Coordination(format!(
                "intent already exists: {}",
                payload.id
            )));
        }

        let hashed_agent_id = hash_agent_name(&payload.agent_id);
        let expires = Utc::now()
            + chrono::Duration::from_std(payload.ttl).map_err(|err| {
                CodexError::Coordination(format!("invalid ttl for {}: {err}", payload.id))
            })?;
        let held_locks = planned_locks(&payload);

        if held_locks.is_empty() {
            return Err(CodexError::Coordination(format!(
                "intent {} does not target any semantic units",
                payload.id
            )));
        }

        let mut acquired = Vec::new();
        for lock in &held_locks {
            match self.engine.request_lock(Intent {
                agent_id: hashed_agent_id,
                target: lock.target,
                kind: lock.kind,
            }) {
                LockResult::Granted => acquired.push(lock.clone()),
                LockResult::Denied { conflicts } => {
                    for rollback in &acquired {
                        let _ = self.engine.release_lock(&hashed_agent_id, &rollback.target);
                    }
                    return Ok(IntentResult::Rejected {
                        conflicts: self.translate_conflicts(conflicts),
                    });
                }
            }
        }

        let lock_token = Uuid::new_v4();
        let active = ActiveIntent {
            payload,
            hashed_agent_id,
            lock_token,
            acquired: Utc::now(),
            expires,
            held_locks: acquired,
        };

        if let Err(err) = self.replica.upsert_intent(&active_to_record(&active)) {
            for rollback in &active.held_locks {
                let _ = self
                    .engine
                    .release_lock(&active.hashed_agent_id, &rollback.target);
            }
            return Err(err);
        }

        self.intents.insert(active.payload.id, active);

        Ok(IntentResult::Approved {
            lock_token,
            expires,
        })
    }

    /// Commit an active intent, releasing its locks.
    pub fn commit_intent(
        &mut self,
        intent_id: Uuid,
        lock_token: Uuid,
    ) -> CodexResult<CommitContext> {
        let active = self.take_active_intent(intent_id, lock_token)?;
        if let Err(err) = self.replica.remove_intent(intent_id) {
            self.intents.insert(intent_id, active);
            return Err(err);
        }
        let released_locks = match release_active_intent(&mut self.engine, &active) {
            Ok(released_locks) => released_locks,
            Err(err) => {
                let _ = self.replica.upsert_intent(&active_to_record(&active));
                self.intents.insert(intent_id, active);
                return Err(err);
            }
        };
        Ok(CommitContext {
            intent_id,
            agent_id: active.payload.agent_id,
            description: active.payload.description,
            released_locks,
        })
    }

    /// Abort an active intent, releasing its locks without recording work.
    pub fn abort_intent(&mut self, intent_id: Uuid, lock_token: Uuid) -> CodexResult<AbortContext> {
        let active = self.take_active_intent(intent_id, lock_token)?;
        if let Err(err) = self.replica.remove_intent(intent_id) {
            self.intents.insert(intent_id, active);
            return Err(err);
        }
        let released_locks = match release_active_intent(&mut self.engine, &active) {
            Ok(released_locks) => released_locks,
            Err(err) => {
                let _ = self.replica.upsert_intent(&active_to_record(&active));
                self.intents.insert(intent_id, active);
                return Err(err);
            }
        };
        Ok(AbortContext {
            intent_id,
            agent_id: active.payload.agent_id,
            released_locks,
        })
    }

    /// Query the underlying semantic graph by name.
    pub fn query_graph(&self, query: &GraphQuery) -> CodexResult<Vec<SemanticUnit>> {
        match query.kind {
            GraphQueryKind::UnitsNamed => Ok(vec![self.find_unit_by_name(&query.value)?]),
            GraphQueryKind::CallersOf => {
                let target = self.find_unit_by_name(&query.value)?;
                let mut units: Vec<_> = self
                    .engine
                    .callers_of(&target.id)
                    .into_iter()
                    .cloned()
                    .collect();
                units.sort_by(|left, right| left.qualified_name.cmp(&right.qualified_name));
                Ok(units)
            }
            GraphQueryKind::DepsOf => {
                let target = self.find_unit_by_name(&query.value)?;
                let mut units: Vec<_> = self
                    .engine
                    .deps_of(&target.id)
                    .into_iter()
                    .cloned()
                    .collect();
                units.sort_by(|left, right| left.qualified_name.cmp(&right.qualified_name));
                Ok(units)
            }
        }
    }

    /// Snapshot all currently active locks.
    pub fn locks(&self) -> Vec<LockEntry> {
        let mut locks: Vec<_> = self
            .intents
            .values()
            .flat_map(|intent| {
                intent.held_locks.iter().map(|lock| LockEntry {
                    target: lock.target,
                    target_name: self.unit_label(lock.target),
                    holder: intent.payload.agent_id.clone(),
                    intent_id: intent.payload.id,
                    lock_kind: to_lock_kind(lock.kind),
                    acquired: intent.acquired,
                    expires: intent.expires,
                })
            })
            .collect();
        locks.sort_by(|left, right| {
            left.target_name
                .cmp(&right.target_name)
                .then_with(|| left.holder.cmp(&right.holder))
        });
        locks
    }

    /// Expire stale intents and release their locks.
    pub fn expire_stale(&mut self) -> Vec<ExpiredIntent> {
        let now = Utc::now();
        let expired_ids: Vec<_> = self
            .intents
            .iter()
            .filter_map(|(intent_id, intent)| (intent.expires <= now).then_some(*intent_id))
            .collect();

        let mut expired = Vec::new();
        for intent_id in expired_ids {
            if let Some(active) = self.intents.remove(&intent_id) {
                let released_locks = release_active_intent(&mut self.engine, &active).unwrap_or(0);
                let _ = self.replica.remove_intent(intent_id);
                expired.push(ExpiredIntent {
                    intent_id,
                    agent_id: active.payload.agent_id,
                    released_locks,
                });
            }
        }

        expired.sort_by(|left, right| left.intent_id.cmp(&right.intent_id));
        expired
    }

    fn take_active_intent(
        &mut self,
        intent_id: Uuid,
        lock_token: Uuid,
    ) -> CodexResult<ActiveIntent> {
        let Some(active) = self.intents.remove(&intent_id) else {
            return Err(CodexError::Coordination(format!(
                "unknown intent: {intent_id}"
            )));
        };

        if active.lock_token != lock_token {
            self.intents.insert(intent_id, active);
            return Err(CodexError::Coordination(format!(
                "invalid lock token for intent: {intent_id}"
            )));
        }

        Ok(active)
    }

    fn find_unit_by_name(&self, name: &str) -> CodexResult<SemanticUnit> {
        if let Some(unit) = self
            .engine
            .units()
            .into_iter()
            .find(|unit| unit.qualified_name == name)
        {
            return Ok(unit.clone());
        }

        if let Some(unit) = self
            .engine
            .units()
            .into_iter()
            .find(|unit| unit.name == name)
        {
            return Ok(unit.clone());
        }

        Err(CodexError::Coordination(format!("unknown target: {name}")))
    }

    fn translate_conflicts(&self, conflicts: Vec<LockConflict>) -> Vec<IntentConflict> {
        conflicts
            .into_iter()
            .map(|conflict| {
                let blocking = self
                    .intents
                    .values()
                    .find(|intent| {
                        intent.hashed_agent_id == conflict.held_by
                            && intent
                                .held_locks
                                .iter()
                                .any(|lock| lock.target == conflict.target)
                    })
                    .map(|intent| (intent.payload.id, intent.payload.agent_id.clone()))
                    .unwrap_or((Uuid::nil(), format_agent_id(conflict.held_by)));

                IntentConflict {
                    blocking_intent: blocking.0,
                    blocking_agent: blocking.1,
                    contested_unit: conflict.target,
                    reason: conflict.reason,
                }
            })
            .collect()
    }

    fn unit_label(&self, target: SemanticId) -> String {
        self.engine
            .get_unit(&target)
            .map(|unit| unit.qualified_name.clone())
            .unwrap_or_else(|| hex_semantic_id(&target))
    }

    fn rebuild_from_replica(&mut self) -> CodexResult<()> {
        let records = self.replica.intent_records()?;

        let mut intents = HashMap::with_capacity(records.len());
        let mut locks = Vec::new();
        for record in records {
            locks.extend(record.held_locks.iter().map(|held| nex_core::SemanticLock {
                agent_id: record.hashed_agent_id,
                target: held.target,
                kind: held.kind,
            }));
            intents.insert(record.intent_id, record_to_active(record));
        }

        self.engine.import_locks(locks);
        self.intents = intents;
        Ok(())
    }
}

fn planned_locks(payload: &IntentPayload) -> Vec<HeldLock> {
    let mut requested = HashMap::new();

    for target in &payload.target_units {
        requested.insert(*target, IntentKind::Write);
    }

    for change in &payload.estimated_changes {
        match change {
            PlannedChange::ModifyBody { unit }
            | PlannedChange::ModifySignature { unit, .. }
            | PlannedChange::MoveUnit { unit, .. }
            | PlannedChange::RenameUnit { unit, .. } => {
                merge_kind(&mut requested, *unit, IntentKind::Write);
            }
            PlannedChange::RemoveUnit { unit } => {
                merge_kind(&mut requested, *unit, IntentKind::Delete);
            }
            PlannedChange::AddUnit { parent, .. } => {
                merge_kind(&mut requested, *parent, IntentKind::Read);
            }
        }
    }

    let mut locks: Vec<_> = requested
        .into_iter()
        .map(|(target, kind)| HeldLock { target, kind })
        .collect();
    locks.sort_by(|left, right| left.target.cmp(&right.target));
    locks
}

fn merge_kind(
    requested: &mut HashMap<SemanticId, IntentKind>,
    target: SemanticId,
    incoming: IntentKind,
) {
    let merged = match requested.get(&target).copied() {
        Some(IntentKind::Delete) | Some(_) if incoming == IntentKind::Delete => IntentKind::Delete,
        Some(IntentKind::Write) | Some(_) if incoming == IntentKind::Write => IntentKind::Write,
        Some(existing) => existing,
        None => incoming,
    };
    requested.insert(target, merged);
}

fn release_active_intent(
    engine: &mut CoordinationEngine,
    active: &ActiveIntent,
) -> CodexResult<usize> {
    for held in &active.held_locks {
        engine.release_lock(&active.hashed_agent_id, &held.target)?;
    }
    Ok(active.held_locks.len())
}

fn to_lock_kind(kind: IntentKind) -> LockKind {
    match kind {
        IntentKind::Read => LockKind::Shared,
        IntentKind::Write | IntentKind::Delete => LockKind::Exclusive,
    }
}

fn hash_agent_name(name: &str) -> AgentId {
    let hash = blake3::hash(name.as_bytes());
    let mut id = [0u8; 16];
    id.copy_from_slice(&hash.as_bytes()[..16]);
    id
}

fn format_agent_id(agent_id: AgentId) -> String {
    agent_id.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn hex_semantic_id(id: &SemanticId) -> String {
    id.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn active_to_record(active: &ActiveIntent) -> CrdtIntentRecord {
    CrdtIntentRecord {
        intent_id: active.payload.id,
        payload: active.payload.clone(),
        hashed_agent_id: active.hashed_agent_id,
        lock_token: active.lock_token,
        acquired: active.acquired,
        expires: active.expires,
        held_locks: active
            .held_locks
            .iter()
            .map(|held| CrdtHeldLock {
                target: held.target,
                kind: held.kind,
            })
            .collect(),
    }
}

fn record_to_active(record: CrdtIntentRecord) -> ActiveIntent {
    ActiveIntent {
        payload: record.payload,
        hashed_agent_id: record.hashed_agent_id,
        lock_token: record.lock_token,
        acquired: record.acquired,
        expires: record.expires,
        held_locks: record
            .held_locks
            .into_iter()
            .map(|held| HeldLock {
                target: held.target,
                kind: held.kind,
            })
            .collect(),
    }
}

fn random_peer_id() -> u64 {
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&Uuid::new_v4().as_bytes()[..8]);
    u64::from_le_bytes(bytes)
}
