# Codex Prompt 001: Implement codex-parse TypeScript Extractor

## Role
You are implementing the `codex-parse` crate for Project Codex — a semantic code coordination system. You are filling in function bodies for a TypeScript semantic extractor that uses tree-sitter 0.26.

## Constraints (MANDATORY — violations will be rejected)
- Do NOT alter any type signatures in `codex-core`. They are authoritative.
- Do NOT alter the `SemanticExtractor` trait signature. It is a contract.
- Do NOT alter `Cargo.toml` dependency versions. They are pinned.
- `cargo clippy -- -D warnings` must pass.
- `cargo fmt --check` must pass.
- `cargo test -p codex-parse` must pass ALL tests in `tests/typescript_extractor.rs`.
- No `unsafe` blocks without documented justification.

## What You Own
You own the implementation details inside these files:
1. `crates/codex-parse/src/typescript.rs` — Replace all `todo!()` calls with working implementations.
2. `crates/codex-parse/src/bridge.rs` — Implement the tree-sitter → rowan bridge (optional for this prompt; the TypeScript extractor can work without rowan if needed for Phase 0, but the bridge module should at least contain the `KindMap` infrastructure).

## Architecture Context
```
codex-core (types) ← codex-parse (this crate) ← codex-graph ← codex-cli
```
`codex-parse` depends on `codex-core` for types. Nothing depends on `codex-parse` yet except tests.

## Type Definitions (from codex-core — READ ONLY)

```rust
pub type SemanticId = [u8; 32]; // BLAKE3 256-bit

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

pub enum UnitKind { Function, Method, Class, Struct, Interface, Trait, Enum, Module, Constant }
pub enum DepKind { Calls, Imports, Inherits, Implements, Uses }
```

## Trait to Implement

```rust
pub trait SemanticExtractor: Send + Sync {
    fn extensions(&self) -> &[&str];
    fn extract(&self, path: &Path, source: &[u8]) -> CodexResult<Vec<SemanticUnit>>;
    fn dependencies(&self, units: &[SemanticUnit], source: &[u8])
        -> CodexResult<Vec<(SemanticId, SemanticId, DepKind)>>;
}
```

## Tree-sitter API (0.26 — CRITICAL)

Grammar loading MUST use the 0.26 pattern:
```rust
use tree_sitter_language::LanguageFn;

// TypeScript crate exports LANGUAGE_TYPESCRIPT and LANGUAGE_TSX
parser.set_language(
    &tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()
).unwrap();
```

Do NOT use the old `fn language() -> Language` pattern. It does not exist in 0.26.

## Extraction Targets (TypeScript)

Extract these tree-sitter node types:
| Node Type | UnitKind |
|---|---|
| `function_declaration` | Function |
| `class_declaration` | Class |
| `method_definition` | Method |
| `interface_declaration` | Interface |
| `arrow_function` (when assigned to a named `const`) | Function |

## Hashing Specification

### SemanticId (BLAKE3)
Hash over: `format!("{}:{}", qualified_name, file_path.display())` encoded as UTF-8 bytes.

### signature_hash (u64)
Hash the parameter list text + return type annotation text using BLAKE3, then truncate to u64 (first 8 bytes, little-endian). This means:
- For `function add(a: number, b: number): number` → hash `"(a: number, b: number): number"`
- If no return type annotation, hash just the parameter list text.

### body_hash (u64)
Hash the **normalized** body AST. "Normalized" means:
- Walk the body subtree in tree-sitter
- Collect leaf token text, **skipping whitespace and comment tokens**
- Concatenate with single-space separator
- BLAKE3 hash the result, truncate to u64
- This ensures whitespace-only changes do NOT change the body_hash.

## Qualified Name Rules
- Top-level function: `"functionName"`
- Class: `"ClassName"`
- Method inside class: `"ClassName::methodName"`
- Named arrow function: `"variableName"`

## Dependency Extraction Rules
- `Calls`: Function A's body contains a call_expression whose function name matches function B's name.
- `Imports`: An import_statement in the file. Create edges from any function that uses the imported identifier to a synthetic external unit.
- `Inherits`: class A `extends` class B → edge from A to B.
- `Implements`: class A `implements` interface B → edge from A to B.

## Tests You Must Pass

The test file is at `crates/codex-parse/tests/typescript_extractor.rs`. Key tests:
1. `extracts_top_level_function` — basic function extraction
2. `extracts_class_with_methods` — class + method hierarchy
3. `extracts_interface` — interface extraction
4. `extracts_named_arrow_function` — const arrow function detection
5. `same_source_produces_same_hashes` — determinism
6. `body_change_detected_signature_stable` — body-only change detection
7. `signature_change_detected` — param change detection
8. `whitespace_changes_do_not_affect_body_hash` — normalized AST hashing
9. `extracts_call_dependencies` — Calls edge detection
10. `extracts_import_dependencies` — Imports edge detection
11. `extracts_inheritance_dependency` — Inherits edge detection
12. `extractor_reports_correct_extensions` — trait contract
13. `empty_file_produces_no_units` — edge case
14. `syntax_error_does_not_panic` — resilience
15. `extracts_mixed_constructs` — multi-construct file
16. `method_qualified_name_includes_class` — qualified_name correctness

## Implementation Strategy (suggested, not required)

1. Initialize `TypeScriptExtractor::new()` with a `tree_sitter::Parser` configured for TypeScript.
2. In `extract()`:
   a. Parse source with tree-sitter.
   b. Walk the root node's children.
   c. For each target node type, extract name, compute hashes, build SemanticUnit.
   d. For classes, recurse into the class body to find method_definition children.
3. In `dependencies()`:
   a. Re-parse (or accept the tree from a cached parse).
   b. Walk call_expression nodes, match callee names against known units.
   c. Walk import_statement nodes.
   d. Check class heritage clauses for extends/implements.

## Deliverables
1. `crates/codex-parse/src/typescript.rs` — fully implemented TypeScriptExtractor
2. `crates/codex-parse/src/bridge.rs` — at minimum, the `KindMap` type (full bridge is Phase 0 stretch)
3. All 16 tests passing: `cargo test -p codex-parse`
4. Clean clippy: `cargo clippy -p codex-parse -- -D warnings`
5. Formatted: `cargo fmt -p codex-parse --check`
