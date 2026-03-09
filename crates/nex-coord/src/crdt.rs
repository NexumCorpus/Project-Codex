//! Loro-backed coordination state for lock and intent convergence.
//!
//! This module keeps the distributed representation deliberately narrow:
//! - lock snapshots used by the CLI persistence layer
//! - active intent records used by the coordination service
//!
//! Each record is stored as a JSON string inside a Loro map keyed by a
//! stable identifier, which gives us CRDT merge semantics at the record
//! level without duplicating the full business logic of the lock engine.

use crate::protocol::IntentPayload;
use chrono::{DateTime, Utc};
use loro::{ExportMode, LoroDoc, ToJson};
use nex_core::{AgentId, CodexError, CodexResult, IntentKind, SemanticId};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::collections::HashMap;
use std::path::Path;
use uuid::Uuid;

const LOCKS_MAP: &str = "locks";
const INTENTS_MAP: &str = "intents";

/// Replicated lock record used by CLI persistence and CRDT snapshots.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CrdtLockEntry {
    /// Human-readable agent name.
    pub agent_name: String,
    /// Deterministic hashed agent id.
    pub agent_id: AgentId,
    /// Human-readable semantic target label.
    pub target_name: String,
    /// Target semantic id.
    pub target: SemanticId,
    /// Lock strength at the engine level.
    pub kind: IntentKind,
}

/// Replicated held-lock entry for an active intent.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CrdtHeldLock {
    /// Locked target semantic id.
    pub target: SemanticId,
    /// Lock strength required for the target.
    pub kind: IntentKind,
}

/// Replicated active-intent record stored in the CRDT document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrdtIntentRecord {
    /// Stable intent id.
    pub intent_id: Uuid,
    /// Original client payload.
    pub payload: IntentPayload,
    /// Deterministic hashed agent id used by the lock engine.
    pub hashed_agent_id: AgentId,
    /// Lock token required for commit/abort.
    pub lock_token: Uuid,
    /// Acquisition time.
    pub acquired: DateTime<Utc>,
    /// Expiration time.
    pub expires: DateTime<Utc>,
    /// Locks currently held by the intent.
    pub held_locks: Vec<CrdtHeldLock>,
}

/// Loro document wrapper for distributed coordination state.
pub struct CoordinationDocument {
    doc: LoroDoc,
}

impl CoordinationDocument {
    /// Create an empty coordination document for a replica peer.
    pub fn new(peer_id: u64) -> CodexResult<Self> {
        let doc = LoroDoc::new();
        doc.set_peer_id(peer_id)
            .map_err(|err| CodexError::Coordination(err.to_string()))?;
        Ok(Self { doc })
    }

    /// Load a coordination document from previously exported bytes.
    pub fn from_bytes(peer_id: u64, bytes: &[u8]) -> CodexResult<Self> {
        let document = Self::new(peer_id)?;
        document.merge_bytes(bytes)?;
        Ok(document)
    }

    /// Load a coordination document from disk, or create an empty one.
    pub fn load_from_path(path: &Path, peer_id: u64) -> CodexResult<Self> {
        if !path.exists() {
            return Self::new(peer_id);
        }

        let bytes = std::fs::read(path)?;
        Self::from_bytes(peer_id, &bytes)
    }

    /// Persist the current CRDT document to disk.
    pub fn save_to_path(&self, path: &Path) -> CodexResult<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        std::fs::write(path, self.export_bytes()?)?;
        Ok(())
    }

    /// Merge remote updates into the local document.
    pub fn merge_bytes(&self, bytes: &[u8]) -> CodexResult<()> {
        if bytes.is_empty() {
            return Ok(());
        }

        self.doc
            .import(bytes)
            .map_err(|err| CodexError::Coordination(err.to_string()))?;
        Ok(())
    }

    /// Export the full update log for downstream replicas.
    pub fn export_bytes(&self) -> CodexResult<Vec<u8>> {
        self.doc
            .export(ExportMode::all_updates())
            .map_err(|err| CodexError::Coordination(err.to_string()))
    }

    /// Replace the lock snapshot map while preserving other CRDT state.
    pub fn replace_lock_entries(&self, entries: &[CrdtLockEntry]) -> CodexResult<()> {
        let desired = entries
            .iter()
            .map(|entry| Ok((lock_key(entry), serde_json::to_string(entry)?)))
            .collect::<CodexResult<HashMap<_, _>>>()?;
        sync_string_map(&self.doc, LOCKS_MAP, desired)
    }

    /// Read all replicated lock entries from the document.
    pub fn lock_entries(&self) -> CodexResult<Vec<CrdtLockEntry>> {
        let mut entries: Vec<CrdtLockEntry> = read_records(&self.doc, LOCKS_MAP)?;
        entries.sort_by(|left, right| {
            left.target_name
                .cmp(&right.target_name)
                .then_with(|| left.agent_name.cmp(&right.agent_name))
                .then_with(|| lock_kind_label(left.kind).cmp(lock_kind_label(right.kind)))
        });
        Ok(entries)
    }

    /// Upsert a replicated active-intent record.
    pub fn upsert_intent(&self, record: &CrdtIntentRecord) -> CodexResult<()> {
        self.doc
            .get_map(INTENTS_MAP)
            .insert(
                &record.intent_id.to_string(),
                serde_json::to_string(record)?,
            )
            .map_err(|err| CodexError::Coordination(err.to_string()))?;
        Ok(())
    }

    /// Remove an active intent from the CRDT document.
    pub fn remove_intent(&self, intent_id: Uuid) -> CodexResult<()> {
        self.doc
            .get_map(INTENTS_MAP)
            .delete(&intent_id.to_string())
            .map_err(|err| CodexError::Coordination(err.to_string()))?;
        Ok(())
    }

    /// Read all active-intent records from the document.
    pub fn intent_records(&self) -> CodexResult<Vec<CrdtIntentRecord>> {
        let mut records: Vec<CrdtIntentRecord> = read_records(&self.doc, INTENTS_MAP)?;
        records.sort_by(|left, right| left.intent_id.cmp(&right.intent_id));
        Ok(records)
    }
}

fn sync_string_map(
    doc: &LoroDoc,
    map_name: &str,
    desired: HashMap<String, String>,
) -> CodexResult<()> {
    let map = doc.get_map(map_name);
    let existing_keys: Vec<String> = map.keys().map(|key| key.to_string()).collect();

    for key in existing_keys {
        if !desired.contains_key(&key) {
            map.delete(&key)
                .map_err(|err| CodexError::Coordination(err.to_string()))?;
        }
    }

    for (key, value) in desired {
        map.insert(&key, value)
            .map_err(|err| CodexError::Coordination(err.to_string()))?;
    }

    Ok(())
}

fn read_records<T: DeserializeOwned>(doc: &LoroDoc, map_name: &str) -> CodexResult<Vec<T>> {
    let json = doc.get_map(map_name).get_deep_value().to_json_value();
    let Some(entries) = json.as_object() else {
        return Ok(Vec::new());
    };

    let mut records = Vec::with_capacity(entries.len());
    for value in entries.values() {
        let encoded = value.as_str().ok_or_else(|| {
            CodexError::Coordination(format!("CRDT map `{map_name}` contains a non-string value"))
        })?;
        records.push(serde_json::from_str(encoded)?);
    }

    Ok(records)
}

fn lock_key(entry: &CrdtLockEntry) -> String {
    format!(
        "{}:{}:{:?}",
        hex_bytes(&entry.agent_id),
        hex_bytes(&entry.target),
        entry.kind
    )
}

fn hex_bytes<const N: usize>(bytes: &[u8; N]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn lock_kind_label(kind: IntentKind) -> &'static str {
    match kind {
        IntentKind::Read => "read",
        IntentKind::Write => "write",
        IntentKind::Delete => "delete",
    }
}
