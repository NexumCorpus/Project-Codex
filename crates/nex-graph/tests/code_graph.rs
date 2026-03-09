//! Integration tests for CodeGraph: construction, queries, and diff.
//!
//! These tests encode the Phase 0 acceptance criteria for the graph layer.
//! Written BEFORE implementation — Codex must write code that passes them.
//!
//! Acceptance criteria tested:
//! - Graph construction from SemanticUnits
//! - O(1) lookup by SemanticId
//! - Dependency edge insertion and queries (callers_of, deps_of)
//! - Diff algorithm: Added, Removed, Modified (signature vs body), Moved
//! - Moved functions detected as moves, not add+delete
//! - Signature changes distinguished from body-only changes

use nex_core::{ChangeKind, DepKind, SemanticUnit, UnitKind};
use nex_graph::CodeGraph;
use std::ops::Range;
use std::path::PathBuf;

// ─────────────────────────────────────────────────────────────────────────────
// Test Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Build a SemanticUnit with explicit control over all hash fields.
fn make_unit(
    name: &str,
    qualified_name: &str,
    kind: UnitKind,
    file_path: &str,
    signature_hash: u64,
    body_hash: u64,
) -> SemanticUnit {
    let id_input = format!("{qualified_name}:{file_path}");
    let id = *blake3::hash(id_input.as_bytes()).as_bytes();
    SemanticUnit {
        id,
        kind,
        name: name.to_string(),
        qualified_name: qualified_name.to_string(),
        file_path: PathBuf::from(file_path),
        byte_range: Range { start: 0, end: 100 },
        signature_hash,
        body_hash,
        dependencies: Vec::new(),
    }
}

fn make_function(name: &str, file: &str, sig_hash: u64, body_hash: u64) -> SemanticUnit {
    make_unit(name, name, UnitKind::Function, file, sig_hash, body_hash)
}

fn make_method(name: &str, class: &str, file: &str, sig_hash: u64, body_hash: u64) -> SemanticUnit {
    let qn = format!("{class}::{name}");
    make_unit(name, &qn, UnitKind::Method, file, sig_hash, body_hash)
}

// ─────────────────────────────────────────────────────────────────────────────
// Construction and Lookup
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn empty_graph() {
    let graph = CodeGraph::new();
    assert_eq!(graph.unit_count(), 0);
    assert_eq!(graph.edge_count(), 0);
}

#[test]
fn add_and_get_unit() {
    let mut graph = CodeGraph::new();
    let unit = make_function("greet", "greet.ts", 100, 200);
    let id = unit.id;
    graph.add_unit(unit);

    assert_eq!(graph.unit_count(), 1);

    let retrieved = graph.get(&id);
    assert!(retrieved.is_some());
    assert_eq!(retrieved.unwrap().name, "greet");
    assert_eq!(retrieved.unwrap().qualified_name, "greet");
}

#[test]
fn add_multiple_units() {
    let mut graph = CodeGraph::new();
    let a = make_function("foo", "a.ts", 1, 2);
    let b = make_function("bar", "b.ts", 3, 4);
    let c = make_function("baz", "c.ts", 5, 6);
    let id_a = a.id;
    let id_b = b.id;
    let id_c = c.id;

    graph.add_unit(a);
    graph.add_unit(b);
    graph.add_unit(c);

    assert_eq!(graph.unit_count(), 3);
    assert_eq!(graph.get(&id_a).unwrap().name, "foo");
    assert_eq!(graph.get(&id_b).unwrap().name, "bar");
    assert_eq!(graph.get(&id_c).unwrap().name, "baz");
}

#[test]
fn get_nonexistent_returns_none() {
    let graph = CodeGraph::new();
    let fake_id = [0u8; 32];
    assert!(graph.get(&fake_id).is_none());
}

#[test]
fn units_returns_all() {
    let mut graph = CodeGraph::new();
    graph.add_unit(make_function("a", "a.ts", 1, 1));
    graph.add_unit(make_function("b", "b.ts", 2, 2));
    graph.add_unit(make_function("c", "c.ts", 3, 3));

    let all = graph.units();
    assert_eq!(all.len(), 3);

    let names: Vec<&str> = all.iter().map(|u| u.name.as_str()).collect();
    assert!(names.contains(&"a"));
    assert!(names.contains(&"b"));
    assert!(names.contains(&"c"));
}

// ─────────────────────────────────────────────────────────────────────────────
// Dependency Edges and Queries
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn add_dep_and_query_deps_of() {
    let mut graph = CodeGraph::new();
    let caller = make_function("processRequest", "handler.ts", 10, 20);
    let callee = make_function("validateToken", "auth.ts", 30, 40);
    let caller_id = caller.id;
    let callee_id = callee.id;

    graph.add_unit(caller);
    graph.add_unit(callee);
    graph.add_dep(caller_id, callee_id, DepKind::Calls);

    assert_eq!(graph.edge_count(), 1);

    // processRequest depends on validateToken
    let deps = graph.deps_of(&caller_id);
    assert_eq!(deps.len(), 1);
    assert_eq!(deps[0].name, "validateToken");
}

#[test]
fn query_callers_of() {
    let mut graph = CodeGraph::new();
    let caller = make_function("processRequest", "handler.ts", 10, 20);
    let callee = make_function("validateToken", "auth.ts", 30, 40);
    let caller_id = caller.id;
    let callee_id = callee.id;

    graph.add_unit(caller);
    graph.add_unit(callee);
    graph.add_dep(caller_id, callee_id, DepKind::Calls);

    // validateToken is called by processRequest
    let callers = graph.callers_of(&callee_id);
    assert_eq!(callers.len(), 1);
    assert_eq!(callers[0].name, "processRequest");
}

#[test]
fn multiple_callers() {
    let mut graph = CodeGraph::new();
    let callee = make_function("validate", "auth.ts", 1, 1);
    let caller1 = make_function("handler_a", "a.ts", 2, 2);
    let caller2 = make_function("handler_b", "b.ts", 3, 3);
    let callee_id = callee.id;
    let c1_id = caller1.id;
    let c2_id = caller2.id;

    graph.add_unit(callee);
    graph.add_unit(caller1);
    graph.add_unit(caller2);
    graph.add_dep(c1_id, callee_id, DepKind::Calls);
    graph.add_dep(c2_id, callee_id, DepKind::Calls);

    let callers = graph.callers_of(&callee_id);
    assert_eq!(callers.len(), 2);
    let names: Vec<&str> = callers.iter().map(|u| u.name.as_str()).collect();
    assert!(names.contains(&"handler_a"));
    assert!(names.contains(&"handler_b"));
}

#[test]
fn no_deps_returns_empty() {
    let mut graph = CodeGraph::new();
    let unit = make_function("lonely", "alone.ts", 1, 1);
    let id = unit.id;
    graph.add_unit(unit);

    assert!(graph.deps_of(&id).is_empty());
    assert!(graph.callers_of(&id).is_empty());
}

// ─────────────────────────────────────────────────────────────────────────────
// Diff: Added and Removed
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn diff_added_units() {
    let before = CodeGraph::new();
    let mut after = CodeGraph::new();
    after.add_unit(make_function("newFunc", "new.ts", 1, 1));

    let diff = before.diff(&after);
    assert_eq!(diff.added.len(), 1);
    assert_eq!(diff.added[0].name, "newFunc");
    assert!(diff.removed.is_empty());
    assert!(diff.modified.is_empty());
    assert!(diff.moved.is_empty());
}

#[test]
fn diff_removed_units() {
    let mut before = CodeGraph::new();
    before.add_unit(make_function("oldFunc", "old.ts", 1, 1));
    let after = CodeGraph::new();

    let diff = before.diff(&after);
    assert_eq!(diff.removed.len(), 1);
    assert_eq!(diff.removed[0].name, "oldFunc");
    assert!(diff.added.is_empty());
}

#[test]
fn diff_unchanged_units() {
    let mut before = CodeGraph::new();
    let mut after = CodeGraph::new();

    // Same function, same hashes, same path
    before.add_unit(make_function("stable", "stable.ts", 100, 200));
    after.add_unit(make_function("stable", "stable.ts", 100, 200));

    let diff = before.diff(&after);
    assert!(diff.added.is_empty());
    assert!(diff.removed.is_empty());
    assert!(diff.modified.is_empty());
    assert!(diff.moved.is_empty());
}

// ─────────────────────────────────────────────────────────────────────────────
// Diff: Modified (Signature vs Body)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn diff_body_changed_only() {
    let mut before = CodeGraph::new();
    let mut after = CodeGraph::new();

    // Same signature_hash, different body_hash
    before.add_unit(make_function("process", "proc.ts", 100, 200));
    after.add_unit(make_function("process", "proc.ts", 100, 999));

    let diff = before.diff(&after);
    assert_eq!(diff.modified.len(), 1);
    assert!(diff.modified[0].changes.contains(&ChangeKind::BodyChanged));
    assert!(
        !diff.modified[0]
            .changes
            .contains(&ChangeKind::SignatureChanged)
    );
}

#[test]
fn diff_signature_changed() {
    let mut before = CodeGraph::new();
    let mut after = CodeGraph::new();

    // Different signature_hash (param added)
    before.add_unit(make_function("validate", "auth.ts", 100, 200));
    after.add_unit(make_function("validate", "auth.ts", 999, 200));

    let diff = before.diff(&after);
    assert_eq!(diff.modified.len(), 1);
    assert!(
        diff.modified[0]
            .changes
            .contains(&ChangeKind::SignatureChanged)
    );
}

#[test]
fn diff_both_signature_and_body_changed() {
    let mut before = CodeGraph::new();
    let mut after = CodeGraph::new();

    // Both hashes differ
    before.add_unit(make_function("transform", "util.ts", 100, 200));
    after.add_unit(make_function("transform", "util.ts", 999, 888));

    let diff = before.diff(&after);
    assert_eq!(diff.modified.len(), 1);
    assert!(
        diff.modified[0]
            .changes
            .contains(&ChangeKind::SignatureChanged)
    );
    assert!(diff.modified[0].changes.contains(&ChangeKind::BodyChanged));
}

// ─────────────────────────────────────────────────────────────────────────────
// Diff: Moved (CRITICAL acceptance criterion)
// "Moved functions detected as moves, not add+delete"
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn diff_moved_function_detected_as_move() {
    let mut before = CodeGraph::new();
    let mut after = CodeGraph::new();

    // Same qualified_name, same hashes, different file path
    before.add_unit(make_function("helper", "old/utils.ts", 100, 200));
    after.add_unit(make_function("helper", "new/utils.ts", 100, 200));

    let diff = before.diff(&after);

    // CRITICAL: must be detected as a move, NOT as add+delete
    assert!(
        diff.added.is_empty(),
        "moved function should NOT appear as added"
    );
    assert!(
        diff.removed.is_empty(),
        "moved function should NOT appear as removed"
    );
    assert_eq!(diff.moved.len(), 1, "should detect exactly one move");
    assert_eq!(diff.moved[0].unit.name, "helper");
    assert_eq!(diff.moved[0].old_path, PathBuf::from("old/utils.ts"));
    assert_eq!(diff.moved[0].new_path, PathBuf::from("new/utils.ts"));
}

#[test]
fn diff_moved_and_modified() {
    let mut before = CodeGraph::new();
    let mut after = CodeGraph::new();

    // Moved AND body changed
    before.add_unit(make_function("transform", "old/transform.ts", 100, 200));
    after.add_unit(make_function("transform", "new/transform.ts", 100, 999));

    let diff = before.diff(&after);

    // Should be classified as modified (body changed) with the move noted.
    // The spec says: same body_hash + different path = Moved.
    // Different body_hash + different path = Modified (not Moved).
    // So this should be modified, not moved.
    assert_eq!(diff.modified.len(), 1);
    assert!(diff.modified[0].changes.contains(&ChangeKind::BodyChanged));
    assert!(diff.moved.is_empty());
}

// ─────────────────────────────────────────────────────────────────────────────
// Diff: Mixed scenario
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn diff_mixed_changes() {
    let mut before = CodeGraph::new();
    let mut after = CodeGraph::new();

    // Unchanged
    before.add_unit(make_function("stable", "a.ts", 1, 1));
    after.add_unit(make_function("stable", "a.ts", 1, 1));

    // Removed
    before.add_unit(make_function("deleted", "b.ts", 2, 2));

    // Added
    after.add_unit(make_function("brand_new", "c.ts", 3, 3));

    // Modified (body only)
    before.add_unit(make_function("tweaked", "d.ts", 4, 40));
    after.add_unit(make_function("tweaked", "d.ts", 4, 99));

    // Moved
    before.add_unit(make_function("relocated", "old/e.ts", 5, 5));
    after.add_unit(make_function("relocated", "new/e.ts", 5, 5));

    let diff = before.diff(&after);

    assert_eq!(diff.added.len(), 1, "one added");
    assert_eq!(diff.added[0].name, "brand_new");

    assert_eq!(diff.removed.len(), 1, "one removed");
    assert_eq!(diff.removed[0].name, "deleted");

    assert_eq!(diff.modified.len(), 1, "one modified");
    assert_eq!(diff.modified[0].after.name, "tweaked");

    assert_eq!(diff.moved.len(), 1, "one moved");
    assert_eq!(diff.moved[0].unit.name, "relocated");
}

// ─────────────────────────────────────────────────────────────────────────────
// Diff: Method inside class
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn diff_method_with_qualified_name() {
    let mut before = CodeGraph::new();
    let mut after = CodeGraph::new();

    before.add_unit(make_method("validate", "Auth", "auth.ts", 10, 20));
    after.add_unit(make_method("validate", "Auth", "auth.ts", 10, 99));

    let diff = before.diff(&after);
    assert_eq!(diff.modified.len(), 1);
    assert_eq!(diff.modified[0].before.qualified_name, "Auth::validate");
    assert!(diff.modified[0].changes.contains(&ChangeKind::BodyChanged));
}

// ─────────────────────────────────────────────────────────────────────────────
// Edge case: empty diff
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn diff_both_empty() {
    let before = CodeGraph::new();
    let after = CodeGraph::new();
    let diff = before.diff(&after);

    assert!(diff.added.is_empty());
    assert!(diff.removed.is_empty());
    assert!(diff.modified.is_empty());
    assert!(diff.moved.is_empty());
}
