# Prompt 005 — nex-coord Coordination Engine

## Context

You are implementing the **Phase 2 coordination engine** for Nexum Graph.
The engine manages a semantic lock table that allows multiple AI agents to
safely declare intents (Read/Write/Delete) on semantic units and receive
immediate feedback about conflicts — both direct (same target) and
transitive (related units connected by dependency edges in the CodeGraph).

Phase 0 (semantic diff) and Phase 1 (conflict detection) are complete.
The coordination engine builds on `CodeGraph` from nex-graph for its
transitive conflict detection.

## Crate Layout

```
crates/nex-coord/
  src/
    lib.rs          — re-exports CoordinationEngine (DO NOT MODIFY)
    coordinator.rs  — YOUR IMPLEMENTATION (fill in todo!() stubs)
    detector.rs     — Phase 1, do not touch
```

## File You Must Implement

### `crates/nex-coord/src/coordinator.rs`

Fill in the `todo!()` bodies for these 8 methods. The struct definition
(fields `locks` and `graph`) is already provided — do not change it.

#### `CoordinationEngine::new(graph: CodeGraph) -> Self`

Initialize the engine with an empty lock table and the provided graph.
```rust
Self {
    locks: HashMap::new(),
    graph,
}
```

#### `CoordinationEngine::request_lock(intent: Intent) -> LockResult`

This is the core method. Follow this algorithm exactly:

**Step 1: Same-agent duplicate check**
If the requesting agent already holds ANY lock on `intent.target`,
return `Denied` with a single `LockConflict`:
```rust
LockConflict {
    held_by: intent.agent_id,
    target: intent.target,
    reason: "agent already holds a lock on this unit".to_string(),
}
```

**Step 2: Direct compatibility check**
For each existing lock on `intent.target` from a DIFFERENT agent,
check if the kinds are compatible. The compatibility rule is simple:
**only Read + Read is compatible. Everything else conflicts.**

```rust
fn compatible(a: IntentKind, b: IntentKind) -> bool {
    matches!((a, b), (IntentKind::Read, IntentKind::Read))
}
```

For each incompatible lock, collect a `LockConflict`:
```rust
LockConflict {
    held_by: existing.agent_id,
    target: existing.target,
    reason: format!("{:?} lock conflicts with requested {:?}", existing.kind, intent.kind),
}
```

**Step 3: Transitive conflict check (Write/Delete only)**
This step only applies if `intent.kind` is `Write` or `Delete`.
Read requests skip this step entirely.

Get the directly related units:
- `self.graph.callers_of(&intent.target)` — units that depend on the target
- `self.graph.deps_of(&intent.target)` — units the target depends on

For each related unit, check if a **different agent** holds a `Write`
or `Delete` lock on it. If so, collect a `LockConflict`:
```rust
LockConflict {
    held_by: existing.agent_id,
    target: related_unit.id,
    reason: format!(
        "transitive conflict: {:?} lock on related unit `{}`",
        existing.kind, related_unit.name
    ),
}
```

**IMPORTANT**: Skip related units where the lock is held by the
**same agent** as the requester. Same-agent transitive locks are
always allowed (an agent can safely modify related units it controls).

**Step 4: Grant or Deny**
If `conflicts` is empty → insert a new `SemanticLock` into
`self.locks` and return `LockResult::Granted`.
Otherwise → return `LockResult::Denied { conflicts }`.

When inserting, use the entry API:
```rust
self.locks
    .entry(intent.target)
    .or_default()
    .push(SemanticLock {
        agent_id: intent.agent_id,
        target: intent.target,
        kind: intent.kind,
    });
```

#### `release_lock(agent_id, target) -> CodexResult<()>`

Look up `self.locks.get_mut(target)`. If found, search for a lock
with matching `agent_id`. Remove it (use `retain()` or `swap_remove()`).
If the vec becomes empty, remove the key from the map.

If no matching lock was found, return:
```rust
Err(CodexError::Coordination(
    "agent does not hold a lock on this unit".to_string(),
))
```

#### `release_all(agent_id)`

Iterate all entries in `self.locks`. For each vec, `retain()` only
locks whose `agent_id != agent_id`. Remove any entries with empty vecs.

Use `self.locks.retain(|_, locks| { ... ; !locks.is_empty() })`.

#### `active_locks() -> Vec<&SemanticLock>`

Flatten all lock vecs into a single Vec of references:
```rust
self.locks.values().flat_map(|locks| locks.iter()).collect()
```

#### `locks_for_unit(target) -> Vec<&SemanticLock>`

Look up `self.locks.get(target)` and return references, or empty vec.

#### `locks_for_agent(agent_id) -> Vec<&SemanticLock>`

Flatten all lock vecs, filter by `agent_id`:
```rust
self.locks
    .values()
    .flat_map(|locks| locks.iter())
    .filter(|lock| &lock.agent_id == agent_id)
    .collect()
```

#### `state() -> CoordinationState`

```rust
use std::collections::HashSet;

let all_locks: Vec<SemanticLock> = self.locks
    .values()
    .flat_map(|locks| locks.iter().cloned())
    .collect();
let agent_count = all_locks
    .iter()
    .map(|lock| lock.agent_id)
    .collect::<HashSet<_>>()
    .len();

CoordinationState { locks: all_locks, agent_count }
```

## Type Signatures (FROZEN — Do NOT Modify)

These types are in `nex-core/src/coordination.rs`:

```rust
pub type AgentId = [u8; 16];

pub enum IntentKind { Read, Write, Delete }

pub struct Intent {
    pub agent_id: AgentId,
    pub target: SemanticId,
    pub kind: IntentKind,
}

pub struct SemanticLock {
    pub agent_id: AgentId,
    pub target: SemanticId,
    pub kind: IntentKind,
}

pub enum LockResult {
    Granted,
    Denied { conflicts: Vec<LockConflict> },
}

pub struct LockConflict {
    pub held_by: AgentId,
    pub target: SemanticId,
    pub reason: String,
}

pub struct CoordinationState {
    pub locks: Vec<SemanticLock>,
    pub agent_count: usize,
}
```

Also in `nex-core/src/error.rs`:
```rust
pub enum CodexError {
    // ... existing variants ...
    #[error("coordination error: {0}")]
    Coordination(String),
}
```

Also frozen: `SemanticId`, `SemanticUnit`, `CodeGraph` API (`callers_of`, `deps_of`).

## Imports You Will Need

In `coordinator.rs` (already present in the stub):
```rust
use nex_core::{
    AgentId, CodexError, CodexResult, CoordinationState, Intent, IntentKind,
    LockConflict, LockResult, SemanticId, SemanticLock,
};
use nex_graph::CodeGraph;
use std::collections::HashMap;
```

You may also need `std::collections::HashSet` for the `state()` method.

## Acceptance Criteria

All 16 tests in `crates/nex-coord/tests/coordination.rs` must pass:

```
cargo test -p nex-coord
```

Expected output (25 total: 9 from Phase 1 + 16 new):
```
test clean_merge_no_conflicts ... ok
test conflict_report_counts_and_exit_codes ... ok
test detects_concurrent_body_edit ... ok
test detects_concurrent_signature_edit_as_error ... ok
test detects_deleted_dependency ... ok
test detects_naming_collision ... ok
test detects_signature_mismatch ... ok
test merge_base_is_populated_in_report ... ok
test multiple_conflicts_in_single_check ... ok
test grant_write_on_free_unit ... ok
test grant_read_on_free_unit ... ok
test multiple_readers_compatible ... ok
test write_blocks_read ... ok
test read_blocks_write ... ok
test write_blocks_write ... ok
test same_agent_cannot_double_lock ... ok
test same_agent_can_lock_related_units ... ok
test transitive_conflict_via_dependency ... ok
test transitive_conflict_via_caller ... ok
test no_transitive_conflict_for_independent_units ... ok
test read_lock_no_transitive_conflict ... ok
test release_specific_lock ... ok
test release_all_agent_locks ... ok
test release_nonexistent_returns_error ... ok
test state_snapshot ... ok
```

Additionally:
```
cargo test -p nex-cli          # Phase 0 tests must still pass (17/17)
cargo clippy -p nex-coord -- -D warnings    # No warnings
cargo fmt -p nex-coord --check              # Formatted
```

## Constraints

1. Do NOT modify any file in `nex-core/` — types are frozen
2. Do NOT modify `nex-coord/src/lib.rs` — module structure is set
3. Do NOT modify `nex-coord/src/detector.rs` — Phase 1 is done
4. Do NOT modify test files — tests are acceptance criteria
5. Do NOT add new dependencies to any Cargo.toml
6. The ONLY file you modify is `crates/nex-coord/src/coordinator.rs`
7. Map coordination errors to `CodexError::Coordination(String)`
8. The `compatible()` function should be a private helper, not a method on `IntentKind`

## File Checklist

| File | Action |
|------|--------|
| `crates/nex-coord/src/coordinator.rs` | Fill 8 `todo!()` bodies |
| Everything else | DO NOT MODIFY |
