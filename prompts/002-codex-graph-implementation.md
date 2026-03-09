# Codex Prompt 002: Implement codex-graph CodeGraph

## Role
You are implementing the `codex-graph` crate for Project Codex. This crate provides a petgraph-based semantic code graph with construction, query, and diff capabilities. It is the core data structure that `codex diff` operates on.

## Constraints (MANDATORY — violations will be rejected)
- Do NOT alter any type signatures in `codex-core`. They are authoritative.
- Do NOT alter the public method signatures on `CodeGraph`. They are contracts.
- Do NOT alter `Cargo.toml` dependency versions.
- `cargo clippy -p codex-graph -- -D warnings` must pass.
- `cargo fmt -p codex-graph --check` must pass.
- `cargo test -p codex-graph` must pass ALL tests in `tests/code_graph.rs`.
- No `unsafe` blocks without documented justification.

## What You Own
You own the implementation of all `todo!()` function bodies in:
- `crates/codex-graph/src/lib.rs`

You may add private helper functions or internal modules if needed, but the public API must not change.

## Architecture Context
```
codex-core (types) ← codex-graph (this crate) ← codex-coord, codex-cli
```
`codex-graph` depends only on `codex-core` for types and `petgraph` for the graph structure.

## Type Definitions You Consume (from codex-core — READ ONLY)

```rust
pub type SemanticId = [u8; 32];

pub struct SemanticUnit {
    pub id: SemanticId,
    pub kind: UnitKind,
    pub name: String,
    pub qualified_name: String,
    pub file_path: PathBuf,
    pub byte_range: Range<usize>,
    pub signature_hash: u64,
    pub body_hash: u64,
    pub dependencies: Vec<SemanticId>,
}

pub struct SemanticDiff {
    pub added: Vec<SemanticUnit>,
    pub removed: Vec<SemanticUnit>,
    pub modified: Vec<ModifiedUnit>,
    pub moved: Vec<MovedUnit>,
}

pub struct ModifiedUnit {
    pub before: SemanticUnit,
    pub after: SemanticUnit,
    pub changes: Vec<ChangeKind>,
}

pub enum ChangeKind { SignatureChanged, BodyChanged, DocChanged }

pub struct MovedUnit {
    pub unit: SemanticUnit,
    pub old_path: PathBuf,
    pub new_path: PathBuf,
}

pub enum DepKind { Calls, Imports, Inherits, Implements, Uses }
```

## Types Defined in This Crate

```rust
pub struct DepEdge { pub kind: DepKind }
pub struct SemanticNode { pub unit: SemanticUnit }

pub struct CodeGraph {
    graph: DiGraph<SemanticNode, DepEdge>,
    index: HashMap<SemanticId, NodeIndex>,
    name_index: HashMap<String, NodeIndex>,  // qualified_name → NodeIndex
}
```

## Methods to Implement

### `CodeGraph::new()` → `Self`
Initialize empty `DiGraph`, empty `index` HashMap, empty `name_index` HashMap.

### `CodeGraph::add_unit(unit: SemanticUnit)` → `NodeIndex`
1. Clone `qualified_name` before moving `unit`.
2. Add node to `graph` with `SemanticNode { unit }`.
3. Insert into `index` with the unit's `id`.
4. Insert into `name_index` with the `qualified_name`.
5. Return the `NodeIndex`.

### `CodeGraph::add_dep(from, to, kind)`
1. Look up `from` and `to` in `index`.
2. If both exist, add a directed edge from → to with `DepEdge { kind }`.
3. If either is missing, silently ignore (no panic).

### `CodeGraph::get(id)` → `Option<&SemanticUnit>`
Look up `id` in `index`, then get node weight from `graph`.

### `CodeGraph::callers_of(id)` → `Vec<&SemanticUnit>`
Look up `id` in `index`. Use `petgraph::Direction::Incoming` to find all neighbors with incoming edges. Return their units.

### `CodeGraph::deps_of(id)` → `Vec<&SemanticUnit>`
Look up `id` in `index`. Use `petgraph::Direction::Outgoing` to find all neighbors with outgoing edges. Return their units.

### `CodeGraph::units()` → `Vec<&SemanticUnit>`
Iterate all node weights in `graph`, return references to their `unit` fields.

### `CodeGraph::diff(other: &CodeGraph)` → `SemanticDiff` (CRITICAL)

This is the core algorithm from the spec. The match key is `qualified_name`, NOT `SemanticId` (because the id includes the file path, which changes on moves).

**Algorithm:**

1. Build a map: `self_by_name: HashMap<&str, &SemanticUnit>` from `self.name_index`.
2. Build a map: `other_by_name: HashMap<&str, &SemanticUnit>` from `other.name_index`.
3. Create empty vectors: `added`, `removed`, `modified`, `moved`.

4. For each `(name, other_unit)` in `other_by_name`:
   - If `name` exists in `self_by_name` (let `self_unit`):
     - If `self_unit.body_hash == other_unit.body_hash` AND `self_unit.signature_hash == other_unit.signature_hash`:
       - If `self_unit.file_path != other_unit.file_path` → **Moved**
       - Else → **Unchanged** (skip)
     - Else → **Modified**:
       - Build `changes: Vec<ChangeKind>`:
         - If `signature_hash` differs → push `SignatureChanged`
         - If `body_hash` differs → push `BodyChanged`
       - Push `ModifiedUnit { before: self_unit.clone(), after: other_unit.clone(), changes }`
   - If `name` NOT in `self_by_name` → **Added** (push `other_unit.clone()`)

5. For each `(name, self_unit)` in `self_by_name`:
   - If `name` NOT in `other_by_name` → **Removed** (push `self_unit.clone()`)

6. Return `SemanticDiff { added, removed, modified, moved }`.

## Tests You Must Pass

The test file is at `crates/codex-graph/tests/code_graph.rs`. 18 tests:

**Construction & Lookup (5):**
1. `empty_graph` — new graph has 0 nodes, 0 edges
2. `add_and_get_unit` — add one unit, retrieve by id
3. `add_multiple_units` — add 3 units, all retrievable
4. `get_nonexistent_returns_none` — missing id returns None
5. `units_returns_all` — all 3 names present

**Dependency Queries (4):**
6. `add_dep_and_query_deps_of` — outgoing edge query
7. `query_callers_of` — incoming edge query
8. `multiple_callers` — 2 callers of same function
9. `no_deps_returns_empty` — isolated node has no deps

**Diff: Added/Removed (3):**
10. `diff_added_units` — new function appears in Added
11. `diff_removed_units` — deleted function appears in Removed
12. `diff_unchanged_units` — identical functions produce empty diff

**Diff: Modified (3):**
13. `diff_body_changed_only` — same sig, diff body → BodyChanged only
14. `diff_signature_changed` — diff sig, same body → SignatureChanged
15. `diff_both_signature_and_body_changed` — both change kinds

**Diff: Moved (2) — CRITICAL:**
16. `diff_moved_function_detected_as_move` — same name+hashes, diff path → Moved (NOT add+delete)
17. `diff_moved_and_modified` — diff path + diff body → Modified (not Moved)

**Diff: Mixed + Edge Cases (2):**
18. `diff_mixed_changes` — one added, one removed, one modified, one moved in same diff
19. `diff_method_with_qualified_name` — method qualified_name match
20. `diff_both_empty` — two empty graphs produce empty diff

## Deliverables
1. `crates/codex-graph/src/lib.rs` — all `todo!()` replaced with implementations
2. All 20 tests passing: `cargo test -p codex-graph`
3. Clean clippy: `cargo clippy -p codex-graph -- -D warnings`
4. Formatted: `cargo fmt -p codex-graph --check`
