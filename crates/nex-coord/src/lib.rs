//! nex-coord: Semantic conflict detection and coordination.
//!
//! Phase 1: `ConflictDetector` — three-way semantic diff with
//! cross-reference analysis for pre-merge conflict detection.
//!
//! Phase 2: `CoordinationEngine` — intent-based semantic lock table
//! with dependency-aware transitive conflict detection.
//!
//! Phase 2 server layer: `CoordinationService` — intent declaration,
//! TTL expiry, graph queries, and protocol-facing lock snapshots.

pub mod coordinator;
pub mod crdt;
pub mod detector;
pub mod protocol;
pub mod service;

pub use coordinator::CoordinationEngine;
pub use crdt::{CoordinationDocument, CrdtHeldLock, CrdtIntentRecord, CrdtLockEntry};
pub use detector::ConflictDetector;
pub use protocol::{
    CoordEvent, GraphQuery, GraphQueryKind, IntentConflict, IntentPayload, IntentResult, LockEntry,
    LockKind, PlannedChange,
};
pub use service::{AbortContext, CommitContext, CoordinationService, ExpiredIntent};
