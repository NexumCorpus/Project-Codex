# Prompt 003 — nex-cli: `nex diff` Pipeline Implementation

## Role

You are implementing the `nex diff` CLI pipeline for Nexum Graph. This is the
integration crate that wires together `nex-parse` (extraction), `nex-graph`
(graph construction + diff), `git2` (file collection), and output formatting.

## Scope

Fill in the `todo!()` bodies in three files:

1. `crates/nex-cli/src/pipeline.rs` — git file collection, graph building, diff orchestration
2. `crates/nex-cli/src/output.rs` — JSON, text, and GitHub markdown formatters

Do NOT modify:
- `crates/nex-cli/src/cli.rs` (clap definitions — complete)
- `crates/nex-cli/src/lib.rs` (module re-exports — complete)
- `crates/nex-cli/src/main.rs` (binary entry point — complete)
- `crates/nex-core/src/` (authoritative types — FROZEN)
- `crates/nex-parse/src/` (extractor implementation — FROZEN)
- `crates/nex-graph/src/` (graph implementation — FROZEN)
- `crates/nex-cli/tests/diff_pipeline.rs` (test file — FROZEN)

## Crate Dependencies Available

```toml
nex-core = { workspace = true }
nex-parse = { workspace = true }
nex-graph = { workspace = true }
clap = { workspace = true }
git2 = { workspace = true }
serde_json = { workspace = true }
thiserror = { workspace = true }
tracing = { workspace = true }
```

## File 1: `pipeline.rs`

### `collect_files_at_ref(repo, refspec, extractors) -> CodexResult<Vec<(String, Vec<u8>)>>`

**Algorithm:**
1. Resolve `refspec` to a commit: `repo.revparse_single(refspec)?.peel_to_commit()?`
2. Get the tree: `commit.tree()?`
3. Walk the tree recursively using `tree.walk(TreeWalkMode::PreOrder, callback)`
4. For each blob entry, extract the file path from the tree walk callback's `(root, entry)` args:
   - `root` is the directory prefix (e.g., "src/")
   - `entry.name()` is the filename
   - Full relative path = `format!("{}{}", root, entry.name().unwrap())`
5. Check if the file extension matches any extractor's `extensions()`:
   - Extract extension: `Path::new(&full_path).extension().and_then(|e| e.to_str())`
   - Check against each extractor: `extractors.iter().any(|e| e.extensions().contains(&ext))`
6. If matched, read the blob content: `repo.find_blob(entry.id())?.content().to_vec()`
7. Collect into `Vec<(String, Vec<u8>)>`

**Important git2 details:**
- `tree.walk()` callback returns `TreeWalkResult::Ok` to continue, `TreeWalkResult::Skip` to skip
- Only process entries where `entry.kind() == Some(ObjectType::Blob)`
- The callback receives `(&str, &TreeEntry)` — root includes trailing `/` except for top-level
- Use `entry.id()` to get the blob OID, then `repo.find_blob(oid)` to read content
- Convert git2 errors to `CodexError::Git(e.to_string())`

### `build_graph(files, extractors) -> CodexResult<CodeGraph>`

**Algorithm:**
1. Create a new `CodeGraph::new()`
2. For each `(path, content)` in `files`:
   a. Extract the file extension from `path`
   b. Find the matching extractor: `extractors.iter().find(|e| e.extensions().contains(&ext))`
   c. If no extractor matches, skip this file
   d. Call `extractor.extract(Path::new(&path), &content)?` to get `Vec<SemanticUnit>`
   e. For each unit, call `graph.add_unit(unit)`
   f. Call `extractor.dependencies(&units, &content)?` to get dependency edges
   g. For each `(from_id, to_id, kind)`, call `graph.add_dep(from_id, to_id, kind)`
3. Return the graph

**Important:** Pass `Path::new(&path)` to `extract()` — the extractor uses this for `SemanticUnit.file_path`.

### `run_diff(repo_path, ref_a, ref_b) -> CodexResult<SemanticDiff>`

**Algorithm:**
1. Open the repository: `git2::Repository::open(repo_path).map_err(|e| CodexError::Git(e.to_string()))?`
2. Create the extractor list: `vec![nex_parse::typescript_extractor()]`
   (Phase 0 only supports TypeScript; more extractors added in future phases)
3. Collect files at ref_a: `collect_files_at_ref(&repo, ref_a, &extractors)?`
4. Collect files at ref_b: `collect_files_at_ref(&repo, ref_b, &extractors)?`
5. Build graph_a: `build_graph(&files_a, &extractors)?`
6. Build graph_b: `build_graph(&files_b, &extractors)?`
7. Diff: `graph_a.diff(&graph_b)`
8. Return the diff

## File 2: `output.rs`

### `format_json(diff) -> String`

Simply: `serde_json::to_string_pretty(diff).unwrap_or_else(|e| format!("{{\"error\": \"{e}\"}}"))`

### `format_text(diff) -> String`

Human-readable summary format:

```
Semantic Diff Summary
=====================
Added:    {count}
Removed:  {count}
Modified: {count}
Moved:    {count}

[If added > 0]
Added:
  + {unit.kind:?} {unit.qualified_name} ({unit.file_path})
  ...

[If removed > 0]
Removed:
  - {unit.kind:?} {unit.qualified_name} ({unit.file_path})
  ...

[If modified > 0]
Modified:
  ~ {mod.after.kind:?} {mod.after.qualified_name} [{changes joined with ", "}]
  ...

[If moved > 0]
Moved:
  → {unit.kind:?} {unit.qualified_name}: {old_path} → {new_path}
  ...
```

For changes display, map `ChangeKind` variants to strings:
- `SignatureChanged` → "signature"
- `BodyChanged` → "body"
- `DocChanged` → "docs"

Use `file_path.display()` for path rendering.

### `format_github(diff) -> String`

GitHub-flavored markdown:

```markdown
# Semantic Diff

| Category | Count |
|----------|-------|
| Added | {count} |
| Removed | {count} |
| Modified | {count} |
| Moved | {count} |

[If added > 0]
## Added
| Kind | Name | File |
|------|------|------|
| {kind:?} | `{qualified_name}` | `{file_path}` |
...

[If removed > 0]
## Removed
| Kind | Name | File |
|------|------|------|
| {kind:?} | `{qualified_name}` | `{file_path}` |
...

[If modified > 0]
## Modified
| Kind | Name | Changes |
|------|------|---------|
| {kind:?} | `{qualified_name}` | {changes} |
...

[If moved > 0]
## Moved
| Kind | Name | From | To |
|------|------|------|-----|
| {kind:?} | `{qualified_name}` | `{old_path}` | `{new_path}` |
...
```

## Type Reference

The types you'll work with (from `nex-core`, do NOT modify):

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
```

## CodeGraph API (from `nex-graph`, already implemented):

```rust
impl CodeGraph {
    pub fn new() -> Self;
    pub fn add_unit(&mut self, unit: SemanticUnit) -> NodeIndex;
    pub fn add_dep(&mut self, from: SemanticId, to: SemanticId, kind: DepKind);
    pub fn diff(&self, other: &CodeGraph) -> SemanticDiff;
    pub fn unit_count(&self) -> usize;
    pub fn edge_count(&self) -> usize;
}
```

## SemanticExtractor trait (from `nex-parse`):

```rust
pub trait SemanticExtractor: Send + Sync {
    fn extensions(&self) -> &[&str];
    fn extract(&self, path: &Path, source: &[u8]) -> CodexResult<Vec<SemanticUnit>>;
    fn dependencies(&self, units: &[SemanticUnit], source: &[u8])
        -> CodexResult<Vec<(SemanticId, SemanticId, DepKind)>>;
}
```

## Tests That Must Pass

Run with: `cargo test -p nex-cli`

### Pipeline unit tests (5):
1. `build_graph_single_function` — Single TS function → 1 unit in graph
2. `build_graph_class_with_methods` — Class with 2 methods → 3 units
3. `build_graph_multiple_files` — 2 files → 2 units
4. `build_graph_skips_non_ts_files` — `.py` file skipped, only `.ts` parsed
5. `build_graph_with_dependencies` — Function calling another → edge in graph

### Git integration tests (7):
6. `diff_detects_added_function` — New function in v2 appears in `diff.added`
7. `diff_detects_removed_function` — Deleted function appears in `diff.removed`
8. `diff_detects_body_change` — Changed function body → `ChangeKind::BodyChanged`
9. `diff_detects_signature_change` — Changed param types → `ChangeKind::SignatureChanged`
10. `diff_detects_moved_function` — Function moved between files → appears in `diff.moved`
11. `diff_unchanged_produces_empty_diff` — Same content at both refs → empty diff
12. `diff_multiple_files_mixed_changes` — Add + remove + modify across files

### Git file collection tests (2):
13. `collect_files_returns_only_supported_extensions` — Only `.ts` files collected
14. `collect_files_reads_correct_content_at_ref` — Content at v1 vs v2 differs correctly

### Output formatting tests (3):
15. `format_json_produces_valid_json` — Output parses as valid JSON with expected keys
16. `format_text_includes_counts` — Text output includes "0" or "no"
17. `format_github_produces_markdown` — GitHub output contains `#` headers

## Acceptance Criteria

1. `cargo test -p nex-cli` — all 17 tests pass
2. `cargo clippy -p nex-cli -- -D warnings` — clean
3. `cargo fmt -p nex-cli --check` — clean
4. No modifications to frozen files
5. All git2 errors mapped to `CodexError::Git(e.to_string())`
6. `run_diff` correctly orchestrates the full pipeline

## Implementation Notes

- The `tree.walk()` callback in git2 has a specific signature. Use a mutable `Vec` outside
  the closure and push results in. The callback uses `git2::TreeWalkResult`.
- When walking trees, `entry.kind()` returns `Option<ObjectType>`. Check for `ObjectType::Blob`.
- For `collect_files_at_ref`, the root parameter in the callback is `""` for top-level files
  and includes a trailing `/` for subdirectories (e.g., `"src/"`).
- The `format_diff` public function already dispatches to the private formatters — just
  implement the three private functions.
- Use `{:?}` Debug formatting for `UnitKind` in output — it produces clean strings like
  `Function`, `Method`, `Class`.
