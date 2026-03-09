//! Coordination pipeline: git -> parse -> graph -> lock/unlock/list.
//!
//! This module implements the CLI coordination workflow:
//! 1. Parse the codebase at HEAD to build a CodeGraph
//! 2. Load/save lock state from `.codex/locks.json`
//! 3. Convert human-readable agent names and target names to IDs
//! 4. Delegate to CoordinationEngine for lock operations

use codex_coord::CoordinationEngine;
use codex_core::{
    AgentId, CodexError, CodexResult, IntentKind, LockResult, SemanticId, SemanticUnit,
};
use codex_graph::CodeGraph;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// A persisted lock entry with human-readable metadata.
///
/// Wraps the raw `SemanticLock` IDs with the original agent name and
/// target `qualified_name` so that `codex locks` can display readable output
/// without rebuilding the CodeGraph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockEntry {
    /// Human-readable agent name (e.g. "alice").
    pub agent_name: String,
    /// Deterministic AgentId derived from `agent_name` via BLAKE3.
    pub agent_id: AgentId,
    /// Human-readable target name (qualified_name of the semantic unit).
    pub target_name: String,
    /// SemanticId of the target unit.
    pub target: SemanticId,
    /// The kind of lock held.
    pub kind: IntentKind,
}

/// Convert a human-readable agent name to a deterministic `AgentId`.
///
/// Uses BLAKE3 hash of the UTF-8 name bytes, truncated to 16 bytes.
/// Deterministic: the same name always produces the same `AgentId`.
///
/// ```rust,ignore
/// let hash = blake3::hash(name.as_bytes());
/// let mut id = [0u8; 16];
/// id.copy_from_slice(&hash.as_bytes()[..16]);
/// id
/// ```
pub fn agent_name_to_id(name: &str) -> AgentId {
    let hash = blake3::hash(name.as_bytes());
    let mut id = [0u8; 16];
    id.copy_from_slice(&hash.as_bytes()[..16]);
    id
}

/// Parse an intent kind string to `IntentKind`.
///
/// Accepts (case-insensitive): `"read"`, `"write"`, `"delete"`.
/// Returns `Err(CodexError::Coordination(...))` for unrecognized strings.
pub fn parse_intent_kind(s: &str) -> CodexResult<IntentKind> {
    match s.to_lowercase().as_str() {
        "read" => Ok(IntentKind::Read),
        "write" => Ok(IntentKind::Write),
        "delete" => Ok(IntentKind::Delete),
        _ => Err(CodexError::Coordination(format!(
            "unknown intent kind: {s}"
        ))),
    }
}

/// Load persisted lock entries from `.codex/locks.json`.
///
/// Returns an empty vec if the file does not exist.
/// Returns `Err` if the file exists but contains malformed JSON.
///
/// Path: `{repo_path}/.codex/locks.json`
pub fn load_locks(repo_path: &Path) -> CodexResult<Vec<LockEntry>> {
    let lock_path = repo_path.join(".codex").join("locks.json");
    if !lock_path.exists() {
        return Ok(Vec::new());
    }

    let content = std::fs::read_to_string(&lock_path)?;
    let entries: Vec<LockEntry> = serde_json::from_str(&content)?;
    Ok(entries)
}

/// Save lock entries to `.codex/locks.json`.
///
/// Creates the `.codex/` directory if it does not exist.
/// Writes pretty-printed JSON via `serde_json::to_string_pretty`.
///
/// Path: `{repo_path}/.codex/locks.json`
pub fn save_locks(repo_path: &Path, entries: &[LockEntry]) -> CodexResult<()> {
    let codex_dir = repo_path.join(".codex");
    std::fs::create_dir_all(&codex_dir)?;
    let content = serde_json::to_string_pretty(entries)?;
    std::fs::write(codex_dir.join("locks.json"), content)?;
    Ok(())
}

/// Build a `CodeGraph` from HEAD of the git repository.
///
/// Algorithm:
/// 1. Open the repository at `repo_path` via `git2::Repository::open`
/// 2. Create the built-in extractor set: `codex_parse::default_extractors()`
/// 3. Collect source files from HEAD using `crate::pipeline::collect_files_at_ref(&repo, "HEAD", &extractors)`
/// 4. Build the graph using `crate::pipeline::build_graph(&files, &extractors)`
/// 5. Return the graph
///
/// Map `git2` errors to `CodexError::Git(...)`.
pub fn build_graph_from_head(repo_path: &Path) -> CodexResult<CodeGraph> {
    let repo = git2::Repository::open(repo_path).map_err(|err| CodexError::Git(err.to_string()))?;
    let extractors: Vec<Box<dyn codex_parse::SemanticExtractor>> =
        codex_parse::default_extractors();
    let files = crate::pipeline::collect_files_at_ref(&repo, "HEAD", &extractors)?;
    crate::pipeline::build_graph(&files, &extractors)
}

/// Find a semantic unit by name in the graph.
///
/// Search strategy (first match wins):
/// 1. Exact match on `qualified_name` (iterate `graph.units()`)
/// 2. Exact match on `name` (iterate `graph.units()`)
///
/// Returns a clone of the matching `SemanticUnit`.
/// Returns `Err(CodexError::Coordination("unknown target: {name}"))` if no match.
pub fn find_unit_by_name(graph: &CodeGraph, name: &str) -> CodexResult<SemanticUnit> {
    let units = graph.units();

    if let Some(unit) = units.iter().find(|unit| unit.qualified_name == name) {
        return Ok((*unit).clone());
    }

    if let Some(unit) = units.iter().find(|unit| unit.name == name) {
        return Ok((*unit).clone());
    }

    Err(CodexError::Coordination(format!("unknown target: {name}")))
}

/// Request a lock via the CLI pipeline.
///
/// Full flow:
/// 1. Build CodeGraph from HEAD via `build_graph_from_head`
/// 2. Find the target unit by name via `find_unit_by_name`
/// 3. Convert `agent_name` to `AgentId` via `agent_name_to_id`
/// 4. Parse `kind_str` to `IntentKind` via `parse_intent_kind`
/// 5. Load existing lock entries from disk via `load_locks`
/// 6. Create a `CoordinationEngine` with the graph
/// 7. Convert existing `LockEntry` items to `SemanticLock` items and call `engine.import_locks(...)`
/// 8. Call `engine.request_lock(Intent { agent_id, target: unit.id, kind })`
/// 9. If `Granted`, append a new `LockEntry` to the entries and call `save_locks`
/// 10. Return the `LockResult`
///
/// When converting `LockEntry` to `SemanticLock` for import, use:
/// ```rust,ignore
/// SemanticLock { agent_id: entry.agent_id, target: entry.target, kind: entry.kind }
/// ```
pub fn run_lock(
    repo_path: &Path,
    agent_name: &str,
    target_name: &str,
    kind_str: &str,
) -> CodexResult<LockResult> {
    use codex_core::{Intent, SemanticLock};

    let graph = build_graph_from_head(repo_path)?;
    let unit = find_unit_by_name(&graph, target_name)?;
    let agent_id = agent_name_to_id(agent_name);
    let kind = parse_intent_kind(kind_str)?;

    let mut entries = load_locks(repo_path)?;
    let mut engine = CoordinationEngine::new(graph);

    let semantic_locks: Vec<SemanticLock> = entries
        .iter()
        .map(|entry| SemanticLock {
            agent_id: entry.agent_id,
            target: entry.target,
            kind: entry.kind,
        })
        .collect();
    engine.import_locks(semantic_locks);

    let result = engine.request_lock(Intent {
        agent_id,
        target: unit.id,
        kind,
    });

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
}

/// Release a specific lock via the CLI pipeline.
///
/// This does NOT require rebuilding the graph. It operates purely on
/// the persisted lock entries.
///
/// Flow:
/// 1. Load existing lock entries via `load_locks`
/// 2. Find the entry where `agent_name` and `target_name` both match
/// 3. If not found, return `Err(CodexError::Coordination("no lock held by {agent_name} on {target_name}"))`
/// 4. Remove the matching entry (use `retain` or `remove`)
/// 5. Save updated entries via `save_locks`
/// 6. Return `Ok(())`
pub fn run_unlock(repo_path: &Path, agent_name: &str, target_name: &str) -> CodexResult<()> {
    let mut entries = load_locks(repo_path)?;
    let before_len = entries.len();
    entries.retain(|entry| !(entry.agent_name == agent_name && entry.target_name == target_name));

    if entries.len() == before_len {
        return Err(CodexError::Coordination(format!(
            "no lock held by {agent_name} on {target_name}"
        )));
    }

    save_locks(repo_path, &entries)?;
    Ok(())
}

/// List all active locks from disk.
///
/// Simply loads and returns the persisted lock entries via `load_locks`.
/// Does NOT rebuild the graph.
pub fn run_locks(repo_path: &Path) -> CodexResult<Vec<LockEntry>> {
    load_locks(repo_path)
}

/// Run pre-commit validation via the CLI pipeline.
///
/// Full flow:
/// 1. Open the git repository at `repo_path`
/// 2. Create the built-in extractor set
/// 3. Build CodeGraph at `base_ref` via `collect_files_at_ref` + `build_graph`
/// 4. Build CodeGraph at HEAD via `collect_files_at_ref` + `build_graph`
/// 5. Convert `agent_name` to `AgentId` via `agent_name_to_id`
/// 6. Load existing lock entries from disk via `load_locks`
/// 7. Convert `LockEntry` items to `SemanticLock` items
/// 8. Call `codex_validate::ValidationEngine::validate(...)`
/// 9. Return the `ValidationReport`
///
/// When converting `LockEntry` to `SemanticLock`, use:
/// ```rust,ignore
/// SemanticLock { agent_id: entry.agent_id, target: entry.target, kind: entry.kind }
/// ```
pub fn run_validate(
    repo_path: &Path,
    agent_name: &str,
    base_ref: &str,
) -> CodexResult<codex_core::ValidationReport> {
    let repo = git2::Repository::open(repo_path).map_err(|err| CodexError::Git(err.to_string()))?;
    let extractors: Vec<Box<dyn codex_parse::SemanticExtractor>> =
        codex_parse::default_extractors();

    let files_before = crate::pipeline::collect_files_at_ref(&repo, base_ref, &extractors)?;
    let files_after = crate::pipeline::collect_files_at_ref(&repo, "HEAD", &extractors)?;
    let graph_before = crate::pipeline::build_graph(&files_before, &extractors)?;
    let graph_after = crate::pipeline::build_graph(&files_after, &extractors)?;

    let agent_id = agent_name_to_id(agent_name);
    let entries = load_locks(repo_path)?;
    let semantic_locks: Vec<codex_core::SemanticLock> = entries
        .iter()
        .map(|entry| codex_core::SemanticLock {
            agent_id: entry.agent_id,
            target: entry.target,
            kind: entry.kind,
        })
        .collect();

    Ok(codex_validate::ValidationEngine::validate(
        &graph_before,
        &graph_after,
        agent_name,
        agent_id,
        &semantic_locks,
    ))
}
