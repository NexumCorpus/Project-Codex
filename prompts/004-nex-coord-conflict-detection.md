# Prompt 004 — nex-coord Conflict Detection Implementation

## Context

You are implementing the **Phase 1 conflict detection engine** for Nexum Graph.
The engine performs a three-way semantic diff: given two branch refs, it finds
their merge base, builds CodeGraphs for all three states, diffs them, and
cross-references the changes to detect semantic conflicts.

Phase 0 (`nex diff`) is already complete and working. You will reuse its
patterns (git2 tree walking, nex-parse extraction, nex-graph construction).

## Crate Layout

```
crates/nex-coord/
  src/
    lib.rs          — re-exports ConflictDetector (DO NOT MODIFY)
    detector.rs     — YOUR IMPLEMENTATION (fill in todo!() stubs)

crates/nex-cli/
  src/
    output.rs       — add format_report() implementation (fill in todo!() stub)
```

## Files You Must Implement

### 1. `crates/nex-coord/src/detector.rs`

Fill in the `todo!()` bodies for these functions:

#### `ConflictDetector::detect(repo_path, branch_a, branch_b) -> CodexResult<ConflictReport>`

Orchestrator. Steps:
1. Open the git repo at `repo_path` using `git2::Repository::open()`
2. Call `find_merge_base()` to get the common ancestor OID
3. Create extractors: `vec![nex_parse::typescript_extractor()]`
4. Call `build_graph_at_ref()` three times: for the merge base, branch_a, branch_b
5. Diff: `graph_base.diff(&graph_a)` and `graph_base.diff(&graph_b)`
6. Call `cross_reference(diff_a, diff_b, graph_a, graph_b)` to find conflicts
7. Return a `ConflictReport` with the conflicts, branch names, and merge base hex string

#### `find_merge_base(repo, ref_a, ref_b) -> CodexResult<git2::Oid>`

1. `repo.revparse_single(ref_a)` → peel to commit → get OID
2. `repo.revparse_single(ref_b)` → peel to commit → get OID
3. `repo.merge_base(oid_a, oid_b)` → return the result
4. Map git2 errors to `CodexError::Git(err.to_string())`

#### `build_graph_at_ref(repo, refspec, extractors) -> CodexResult<CodeGraph>`

This is nearly identical to `nex-cli/src/pipeline.rs` functions. Reuse the
same pattern:
1. `repo.revparse_single(refspec)` → peel to commit → get tree
2. Walk the tree with `TreeWalkMode::PreOrder`, collecting blobs whose
   extension matches an extractor
3. For each file, call `extractor.extract()` and `extractor.dependencies()`
4. Add units and edges to a `CodeGraph`
5. Return the graph

**Reference implementation** (from `crates/nex-cli/src/pipeline.rs`):
```rust
pub fn collect_files_at_ref(
    repo: &git2::Repository,
    refspec: &str,
    extractors: &[Box<dyn SemanticExtractor>],
) -> CodexResult<Vec<(String, Vec<u8>)>> {
    let commit = repo
        .revparse_single(refspec)
        .and_then(|object| object.peel_to_commit())
        .map_err(|err| CodexError::Git(err.to_string()))?;
    let tree = commit
        .tree()
        .map_err(|err| CodexError::Git(err.to_string()))?;

    let mut files = Vec::new();
    let mut walk_error: Option<CodexError> = None;

    tree.walk(TreeWalkMode::PreOrder, |root, entry| {
        if entry.kind() != Some(ObjectType::Blob) {
            return TreeWalkResult::Ok;
        }
        let Some(name) = entry.name() else {
            return TreeWalkResult::Ok;
        };
        let full_path = format!("{root}{name}");
        let ext = Path::new(&full_path)
            .extension()
            .and_then(|ext| ext.to_str());
        let is_supported = ext.is_some_and(|ext| {
            extractors
                .iter()
                .any(|extractor| extractor.extensions().contains(&ext))
        });
        if !is_supported {
            return TreeWalkResult::Ok;
        }
        match repo.find_blob(entry.id()) {
            Ok(blob) => {
                files.push((full_path, blob.content().to_vec()));
                TreeWalkResult::Ok
            }
            Err(err) => {
                walk_error = Some(CodexError::Git(err.to_string()));
                TreeWalkResult::Abort
            }
        }
    })
    .map_err(|err| {
        walk_error
            .take()
            .unwrap_or_else(|| CodexError::Git(err.to_string()))
    })?;

    if let Some(err) = walk_error {
        return Err(err);
    }
    Ok(files)
}

pub fn build_graph(
    files: &[(String, Vec<u8>)],
    extractors: &[Box<dyn SemanticExtractor>],
) -> CodexResult<CodeGraph> {
    let mut graph = CodeGraph::new();
    for (path, content) in files {
        let Some(ext) = Path::new(path).extension().and_then(|ext| ext.to_str()) else {
            continue;
        };
        let Some(extractor) = extractors
            .iter()
            .find(|extractor| extractor.extensions().contains(&ext))
        else {
            continue;
        };
        let units = extractor.extract(Path::new(path), content)?;
        let dependencies = extractor.dependencies(&units, content)?;
        for unit in units {
            graph.add_unit(unit);
        }
        for (from_id, to_id, kind) in dependencies {
            graph.add_dep(from_id, to_id, kind);
        }
    }
    Ok(graph)
}
```

You can either inline this logic into `build_graph_at_ref()` or factor it into
collect + build helper functions within `detector.rs`. Do NOT import from
`nex-cli` — the coordination crate should not depend on the CLI crate.

#### `cross_reference(diff_a, diff_b, graph_a, graph_b) -> Vec<SemanticConflict>`

This is the core conflict detection algorithm. Follow these rules from the spec:

**Rule 1: Deleted units with dependents**
For each unit in `diff_a.removed`:
- Look it up in `graph_b` by `qualified_name`
- If found, get `graph_b.callers_of(that_unit.id)`
- For each caller → emit `SemanticConflict` with:
  - `kind: ConflictKind::DeletedDependency { deleted: removed_unit.id, dependent: caller.id }`
  - `severity: Severity::Error`
  - `unit_a`: the removed unit from diff_a
  - `unit_b`: the caller from graph_b
  - `description`: e.g. "Branch A deleted `{name}` but branch B depends on it via `{caller_name}`"
  - `suggestion`: Some("Consider keeping `{name}` or updating callers in branch B")

Symmetrically, for each unit in `diff_b.removed`, check `graph_a.callers_of()`.

**Rule 2: Signature-changed units with callers**
For each unit in `diff_a.modified` where `changes` contains `SignatureChanged`:
- Look up the `after.qualified_name` in `graph_b`
- If found, get `graph_b.callers_of(that_unit.id)`
- For each caller → emit `SemanticConflict` with:
  - `kind: ConflictKind::SignatureMismatch { function: modified.after.id, caller: caller.id }`
  - `severity: Severity::Error`
  - `unit_a`: `modified.after` (the changed function)
  - `unit_b`: the caller from graph_b
  - `description`: e.g. "Branch A changed signature of `{name}` but branch B calls it"
  - `suggestion`: Some("Update callers in branch B to match new signature")

Symmetrically for `diff_b.modified` vs `graph_a`.

**Rule 3: Concurrent modifications (same qualified_name modified in both diffs)**
Build a HashMap of `qualified_name -> &ModifiedUnit` for each diff. For names
that appear in both:
- If both have `SignatureChanged` → `ConcurrentBodyEdit { unit: modified_a.after.id }` with `Severity::Error`
- If both have `BodyChanged` (and not both signature) → `ConcurrentBodyEdit { unit: modified_a.after.id }` with `Severity::Warning`
- `description`: e.g. "Both branches modified `{name}`"
- `suggestion`: Some("Manual merge required")

**Rule 4: Naming collisions**
Build a HashSet of `qualified_name` for each diff's `added` units. For names
that appear in both `diff_a.added` and `diff_b.added`:
- Emit `ConflictKind::NamingCollision { name: qualified_name.clone() }`
- `severity: Severity::Error`
- `unit_a`: the added unit from diff_a
- `unit_b`: the added unit from diff_b
- `description`: e.g. "Both branches added `{name}`"
- `suggestion`: Some("Rename one of the functions to avoid collision")

**Important**: To look up a unit in a graph by `qualified_name`, use:
```rust
graph.units().iter().find(|u| u.qualified_name == name)
```
Then use that unit's `id` to call `graph.callers_of(&id)`.

### 2. `crates/nex-cli/src/output.rs`

Fill in the `todo!()` for `format_report()`:

```rust
pub fn format_report(report: &ConflictReport, format: &str) -> String
```

Pattern it after the existing `format_diff()`. Dispatch on format string:
- `"json"` → `serde_json::to_string_pretty(report)`
- `"github"` → GitHub markdown with summary table + per-conflict details
- `_` (default) → human-readable text

For **text** format, produce output like:
```
Conflict Check: branch_a vs branch_b
Merge base: abc123def456...
=====================================
Errors:   2
Warnings: 1

[ERROR] DeletedDependency: Branch A deleted `shared` but branch B depends on it via `useShared`
  Suggestion: Consider keeping `shared` or updating callers in branch B

[WARNING] ConcurrentBodyEdit: Both branches modified `process`
  Suggestion: Manual merge required

Exit code: 1
```

For **github** format, produce markdown:
```markdown
# Conflict Check

**Branches**: `branch_a` vs `branch_b`
**Merge base**: `abc123...`

| Severity | Count |
|----------|-------|
| Error | 2 |
| Warning | 1 |

## Conflicts

| # | Severity | Kind | Description |
|---|----------|------|-------------|
| 1 | Error | DeletedDependency | Branch A deleted `shared`... |
| 2 | Warning | ConcurrentBodyEdit | Both branches modified... |
```

## Type Signatures (FROZEN — Do NOT Modify)

These types are in `nex-core/src/conflict.rs` and must not be changed:

```rust
pub struct SemanticConflict {
    pub kind: ConflictKind,
    pub severity: Severity,
    pub unit_a: SemanticUnit,
    pub unit_b: SemanticUnit,
    pub description: String,
    pub suggestion: Option<String>,
}

pub enum ConflictKind {
    BrokenReference { caller: SemanticId, callee: SemanticId },
    ConcurrentBodyEdit { unit: SemanticId },
    SignatureMismatch { function: SemanticId, caller: SemanticId },
    DeletedDependency { deleted: SemanticId, dependent: SemanticId },
    NamingCollision { name: String },
    InterfaceDrift { interface_id: SemanticId, implementor: SemanticId },
}

pub enum Severity { Info, Warning, Error }

pub struct ConflictReport {
    pub conflicts: Vec<SemanticConflict>,
    pub branch_a: String,
    pub branch_b: String,
    pub merge_base: String,
}
```

Also frozen: `SemanticUnit`, `SemanticDiff`, `ModifiedUnit`, `ChangeKind`, `CodeGraph` API.

## Dependencies Already Configured

`nex-coord/Cargo.toml`:
```toml
nex-core = { workspace = true }
nex-parse = { workspace = true }
nex-graph = { workspace = true }
git2 = { workspace = true }
```

`nex-cli/Cargo.toml`:
```toml
nex-coord = { workspace = true }
```

## Imports You Will Need

In `detector.rs`:
```rust
use nex_core::{
    ChangeKind, CodexError, CodexResult, ConflictKind, ConflictReport,
    SemanticConflict, SemanticDiff, Severity,
};
use nex_graph::CodeGraph;
use nex_parse::SemanticExtractor;
use git2::{ObjectType, TreeWalkMode, TreeWalkResult};
use std::collections::{HashMap, HashSet};
use std::path::Path;
```

In `output.rs` (already has `use nex_core::{ChangeKind, ConflictReport, SemanticDiff};`):
```rust
// You may need to add:
use nex_core::Severity;
```

## Acceptance Criteria

All 9 tests in `crates/nex-coord/tests/conflict_detection.rs` must pass:

```
cargo test -p nex-coord
```

Expected output:
```
test clean_merge_no_conflicts ... ok
test detects_deleted_dependency ... ok
test detects_signature_mismatch ... ok
test detects_concurrent_body_edit ... ok
test detects_naming_collision ... ok
test detects_concurrent_signature_edit_as_error ... ok
test multiple_conflicts_in_single_check ... ok
test conflict_report_counts_and_exit_codes ... ok
test merge_base_is_populated_in_report ... ok
```

Additionally:
```
cargo test -p nex-cli          # Phase 0 tests must still pass (17/17)
cargo clippy -p nex-coord -p nex-cli -- -D warnings  # No warnings
cargo fmt -p nex-coord -p nex-cli --check             # Formatted
```

## Constraints

1. Do NOT modify any file in `nex-core/` — types are frozen
2. Do NOT modify `nex-coord/src/lib.rs` — module structure is set
3. Do NOT modify test files — tests are acceptance criteria
4. Do NOT modify `nex-cli/src/cli.rs` — CLI args are frozen
5. Do NOT import from `nex-cli` in `nex-coord` (no circular dependency)
6. Do NOT add new dependencies to any Cargo.toml
7. Use `std::collections::HashMap`/`HashSet` — no external map crates
8. Map all git2 errors to `CodexError::Git(err.to_string())`

## File Checklist

| File | Action |
|------|--------|
| `crates/nex-coord/src/detector.rs` | Fill 4 `todo!()` bodies |
| `crates/nex-cli/src/output.rs` | Fill 1 `todo!()` body (`format_report`) |
| Everything else | DO NOT MODIFY |
