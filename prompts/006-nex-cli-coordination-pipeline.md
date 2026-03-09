# Prompt 006 — CLI Coordination Pipeline

## Context

You are implementing the **Phase 2 CLI coordination pipeline** for Nexum Graph.
The coordination engine (Prompt 005) manages a semantic lock table in memory.
This prompt adds:
1. **Persistence** — `export_locks` / `import_locks` on `CoordinationEngine`
2. **CLI pipeline** — `nex lock`, `nex unlock`, `nex locks` commands
3. **Output formatting** — `format_lock_result`, `format_locks`

Phase 0 (semantic diff), Phase 1 (conflict detection), and the Phase 2
coordination engine are complete. The CLI commands (`Diff`, `Check`) and
the engine (`CoordinationEngine`) are working and tested.

## Crate Layout

```
crates/nex-coord/
  src/
    coordinator.rs  — ADD 2 methods (export_locks, import_locks)
    detector.rs     — Phase 1, do not touch
    lib.rs          — do not touch

crates/nex-cli/
  src/
    coordination_pipeline.rs  — YOUR IMPLEMENTATION (fill in todo!() stubs)
    output.rs                 — ADD 2 functions (fill in todo!() stubs)
    cli.rs                    — do not touch (Lock/Unlock/Locks commands pre-wired)
    main.rs                   — do not touch (dispatch pre-wired)
    pipeline.rs               — do not touch (but you CALL its functions)
    lib.rs                    — do not touch
```

## Files You Must Implement

### 1. `crates/nex-coord/src/coordinator.rs` — 2 new methods

Add these two methods inside the existing `impl CoordinationEngine` block.
Do NOT alter any existing methods.

#### `export_locks(&self) -> Vec<SemanticLock>`

Flatten all lock vecs into a single cloned vec:
```rust
self.locks.values().flat_map(|locks| locks.iter().cloned()).collect()
```

#### `import_locks(&mut self, locks: Vec<SemanticLock>)`

Clear the existing lock table and repopulate from the provided vec:
```rust
self.locks.clear();
for lock in locks {
    self.locks.entry(lock.target).or_default().push(lock);
}
```

### 2. `crates/nex-cli/src/coordination_pipeline.rs` — 8 functions

This is the main implementation file. Fill in all `todo!()` bodies.

#### `agent_name_to_id(name: &str) -> AgentId`

Hash the name with BLAKE3, take the first 16 bytes:
```rust
let hash = blake3::hash(name.as_bytes());
let mut id = [0u8; 16];
id.copy_from_slice(&hash.as_bytes()[..16]);
id
```

#### `parse_intent_kind(s: &str) -> CodexResult<IntentKind>`

Case-insensitive match:
```rust
match s.to_lowercase().as_str() {
    "read" => Ok(IntentKind::Read),
    "write" => Ok(IntentKind::Write),
    "delete" => Ok(IntentKind::Delete),
    _ => Err(CodexError::Coordination(format!("unknown intent kind: {s}"))),
}
```

#### `load_locks(repo_path: &Path) -> CodexResult<Vec<LockEntry>>`

Read `.nex/locks.json`. Return empty vec if file doesn't exist:
```rust
let lock_path = repo_path.join(".nex").join("locks.json");
if !lock_path.exists() {
    return Ok(Vec::new());
}
let content = std::fs::read_to_string(&lock_path)?;
let entries: Vec<LockEntry> = serde_json::from_str(&content)?;
Ok(entries)
```

Note: `std::io::Error` and `serde_json::Error` both have `From` impls
in `CodexError`, so `?` works directly.

#### `save_locks(repo_path: &Path, entries: &[LockEntry]) -> CodexResult<()>`

Write `.nex/locks.json`, creating the directory if needed:
```rust
let nex_dir = repo_path.join(".nex");
std::fs::create_dir_all(&nex_dir)?;
let content = serde_json::to_string_pretty(entries)?;
std::fs::write(nex_dir.join("locks.json"), content)?;
Ok(())
```

#### `build_graph_from_head(repo_path: &Path) -> CodexResult<CodeGraph>`

Reuse the existing pipeline functions:
```rust
let repo = git2::Repository::open(repo_path)
    .map_err(|e| CodexError::Git(e.to_string()))?;
let extractors: Vec<Box<dyn nex_parse::SemanticExtractor>> =
    vec![nex_parse::typescript_extractor()];
let files = crate::pipeline::collect_files_at_ref(&repo, "HEAD", &extractors)?;
crate::pipeline::build_graph(&files, &extractors)
```

#### `find_unit_by_name(graph: &CodeGraph, name: &str) -> CodexResult<SemanticUnit>`

Search by qualified_name first, then by name:
```rust
let units = graph.units();

// Try exact match on qualified_name.
if let Some(unit) = units.iter().find(|u| u.qualified_name == name) {
    return Ok((*unit).clone());
}

// Fall back to exact match on name.
if let Some(unit) = units.iter().find(|u| u.name == name) {
    return Ok((*unit).clone());
}

Err(CodexError::Coordination(format!("unknown target: {name}")))
```

#### `run_lock(repo_path, agent_name, target_name, kind_str) -> CodexResult<LockResult>`

Full pipeline:
```rust
use nex_core::{Intent, SemanticLock};

// 1. Build graph from HEAD.
let graph = build_graph_from_head(repo_path)?;

// 2. Resolve the target unit.
let unit = find_unit_by_name(&graph, target_name)?;

// 3. Convert agent name and intent kind.
let agent_id = agent_name_to_id(agent_name);
let kind = parse_intent_kind(kind_str)?;

// 4. Load existing locks and create engine.
let mut entries = load_locks(repo_path)?;
let mut engine = CoordinationEngine::new(graph);

// 5. Import existing locks into engine.
let semantic_locks: Vec<SemanticLock> = entries
    .iter()
    .map(|e| SemanticLock {
        agent_id: e.agent_id,
        target: e.target,
        kind: e.kind,
    })
    .collect();
engine.import_locks(semantic_locks);

// 6. Request the lock.
let result = engine.request_lock(Intent {
    agent_id,
    target: unit.id,
    kind,
});

// 7. If granted, persist.
if matches!(result, LockResult::Granted) {
    entries.push(LockEntry {
        agent_name: agent_name.to_string(),
        agent_id,
        target_name: target_name.to_string(),
        target: unit.id,
        kind,
    });
    save_locks(repo_path, &entries)?;
}

Ok(result)
```

#### `run_unlock(repo_path, agent_name, target_name) -> CodexResult<()>`

Operate on persisted entries only (no graph needed):
```rust
let mut entries = load_locks(repo_path)?;
let before_len = entries.len();
entries.retain(|e| !(e.agent_name == agent_name && e.target_name == target_name));

if entries.len() == before_len {
    return Err(CodexError::Coordination(format!(
        "no lock held by {agent_name} on {target_name}"
    )));
}

save_locks(repo_path, &entries)?;
Ok(())
```

#### `run_locks(repo_path) -> CodexResult<Vec<LockEntry>>`

Just delegate:
```rust
load_locks(repo_path)
```

### 3. `crates/nex-cli/src/output.rs` — 2 new functions

Add these at the bottom of the file. The `use` statements for
`LockEntry` and `LockResult` are already present in the stub.

#### `format_lock_result(result, agent_name, target_name, format) -> String`

```rust
match format {
    "json" => serde_json::to_string_pretty(result)
        .unwrap_or_else(|err| format!("{{\"error\": \"{err}\"}}")),
    _ => match result {
        LockResult::Granted => {
            format!("Lock GRANTED: {agent_name} -> {target_name}\n")
        }
        LockResult::Denied { conflicts } => {
            let mut output = String::new();
            let _ = writeln!(output, "Lock DENIED: {agent_name} -> {target_name}");
            let _ = writeln!(output, "Conflicts:");
            for conflict in conflicts {
                let _ = writeln!(output, "  - {}", conflict.reason);
            }
            output
        }
    },
}
```

#### `format_locks(entries, format) -> String`

```rust
match format {
    "json" => serde_json::to_string_pretty(entries)
        .unwrap_or_else(|err| format!("{{\"error\": \"{err}\"}}")),
    _ => {
        let mut output = String::new();
        let _ = writeln!(output, "Active Locks ({})", entries.len());
        let _ = writeln!(output, "================");
        if entries.is_empty() {
            let _ = writeln!(output, "  (none)");
        } else {
            for entry in entries {
                let _ = writeln!(
                    output,
                    "  [{:?}] {} -> {}",
                    entry.kind, entry.agent_name, entry.target_name
                );
            }
        }
        output
    },
}
```

## Imports You Will Need

### In `coordinator.rs` (already present)
No new imports needed. `SemanticLock` is already in scope.

### In `coordination_pipeline.rs` (already present in the stub)
```rust
use nex_core::{AgentId, CodexError, CodexResult, IntentKind, LockResult, SemanticId, SemanticUnit};
use nex_coord::CoordinationEngine;
use nex_graph::CodeGraph;
use serde::{Deserialize, Serialize};
use std::path::Path;
```

You will also need these in the `run_lock` function body:
```rust
use nex_core::{Intent, SemanticLock};
```

### In `output.rs` (already present in the stub)
```rust
use crate::coordination_pipeline::LockEntry;
use nex_core::LockResult;
```

## Type Definitions (FROZEN — Do NOT Modify)

`LockEntry` is defined in `coordination_pipeline.rs` (pre-written):
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockEntry {
    pub agent_name: String,
    pub agent_id: AgentId,
    pub target_name: String,
    pub target: SemanticId,
    pub kind: IntentKind,
}
```

All types from `nex-core` (`AgentId`, `Intent`, `IntentKind`,
`SemanticLock`, `LockResult`, `LockConflict`, `CoordinationState`,
`CodexError`, `SemanticUnit`, `SemanticId`) are frozen.

## Acceptance Criteria

All tests in `nex-coord` must pass (28 total: 9 Phase 1 + 16 Phase 2 + 3 new):
```
cargo test -p nex-coord
```

New export/import tests:
```
test export_empty_engine ... ok
test export_returns_all_locks ... ok
test import_restores_and_replaces_locks ... ok
```

All tests in `nex-cli` must pass (28 total: 17 Phase 0 + 11 new):
```
cargo test -p nex-cli
```

New coordination CLI tests:
```
test agent_name_produces_deterministic_id ... ok
test parse_intent_kind_accepts_valid_kinds ... ok
test parse_intent_kind_rejects_invalid ... ok
test load_locks_returns_empty_when_no_file ... ok
test save_and_load_round_trip ... ok
test lock_grants_on_free_unit ... ok
test lock_denied_same_agent_double_lock ... ok
test lock_persists_and_lists ... ok
test unlock_removes_lock ... ok
test unlock_nonexistent_returns_error ... ok
test lock_denied_write_conflict ... ok
test lock_unknown_target_returns_error ... ok
```

Additionally:
```
cargo clippy -p nex-coord -p nex-cli -- -D warnings    # No warnings
cargo fmt -p nex-coord -p nex-cli --check              # Formatted
```

## Constraints

1. Do NOT modify any file in `nex-core/` — types are frozen
2. Do NOT modify `nex-coord/src/lib.rs` or `nex-coord/src/detector.rs`
3. Do NOT modify `nex-cli/src/cli.rs`, `nex-cli/src/main.rs`, or `nex-cli/src/lib.rs`
4. Do NOT modify `nex-cli/src/pipeline.rs`
5. Do NOT modify test files — tests are acceptance criteria
6. Do NOT add new dependencies to any Cargo.toml
7. The ONLY files you modify are:
   - `crates/nex-coord/src/coordinator.rs` (add 2 methods)
   - `crates/nex-cli/src/coordination_pipeline.rs` (fill 8 `todo!()` bodies)
   - `crates/nex-cli/src/output.rs` (fill 2 `todo!()` bodies)
8. Do NOT modify the `LockEntry` struct definition — it is pre-written
9. Do NOT modify existing functions in `output.rs` — only fill the new stubs
10. Map errors as follows:
    - `git2` errors → `CodexError::Git(e.to_string())`
    - `std::io` errors → use `?` (From impl exists)
    - `serde_json` errors → use `?` (From impl exists)
    - Coordination logic errors → `CodexError::Coordination(String)`

## File Checklist

| File | Action |
|------|--------|
| `crates/nex-coord/src/coordinator.rs` | Fill 2 `todo!()` bodies (`export_locks`, `import_locks`) |
| `crates/nex-cli/src/coordination_pipeline.rs` | Fill 8 `todo!()` bodies |
| `crates/nex-cli/src/output.rs` | Fill 2 `todo!()` bodies (`format_lock_result`, `format_locks`) |
| Everything else | DO NOT MODIFY |
