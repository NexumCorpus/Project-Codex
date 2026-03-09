//! Integration tests for the validation engine.
//!
//! These tests exercise the Prompt 007 validation workflow:
//! - Lock coverage checking (modification and deletion)
//! - Broken reference detection
//! - Stale caller detection
//! - Mixed issue scenarios
//!
//! IMPORTANT: These are the acceptance criteria for Prompt 007.
//! Do NOT modify these tests — Codex must write code that passes them.

use nex_core::{
    DepKind, IntentKind, SemanticLock, SemanticUnit, Severity, UnitKind, ValidationKind,
};
use nex_graph::CodeGraph;
use nex_validate::ValidationEngine;
use std::path::PathBuf;

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Create an AgentId from a name (BLAKE3 truncated to 16 bytes).
fn agent_id(name: &str) -> [u8; 16] {
    let hash = blake3::hash(name.as_bytes());
    let mut id = [0u8; 16];
    id.copy_from_slice(&hash.as_bytes()[..16]);
    id
}

/// Create a SemanticUnit with the given name and kind.
/// Uses BLAKE3 of qualified_name as the SemanticId.
fn make_unit(
    name: &str,
    kind: UnitKind,
    file: &str,
    sig_hash: u64,
    body_hash: u64,
) -> SemanticUnit {
    let qualified_name = name.to_string();
    let id = blake3::hash(format!("{qualified_name}:{file}:{body_hash}").as_bytes());
    SemanticUnit {
        id: *id.as_bytes(),
        kind,
        name: name.split("::").last().unwrap_or(name).to_string(),
        qualified_name,
        file_path: PathBuf::from(file),
        byte_range: 0..100,
        signature_hash: sig_hash,
        body_hash,
        dependencies: Vec::new(),
    }
}

/// Build a simple graph from units and optional dependency edges.
fn build_graph(units: Vec<SemanticUnit>, deps: Vec<(usize, usize)>) -> CodeGraph {
    let mut graph = CodeGraph::new();
    let ids: Vec<_> = units.iter().map(|u| u.id).collect();
    for unit in units {
        graph.add_unit(unit);
    }
    for (from, to) in deps {
        graph.add_dep(ids[from], ids[to], DepKind::Calls);
    }
    graph
}

// ─────────────────────────────────────────────────────────────────────────────
// Section 1: Clean validation
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn validate_no_changes_no_issues() {
    let validate = make_unit("validate", UnitKind::Function, "handler.ts", 100, 200);
    let graph = build_graph(vec![validate], vec![]);
    // Same graph before and after — no diff, no issues.
    let report = ValidationEngine::validate(&graph, &graph, "alice", agent_id("alice"), &[]);
    assert!(report.issues.is_empty());
    assert_eq!(report.exit_code(), 0);
}

#[test]
fn validate_modification_with_write_lock() {
    let validate_v1 = make_unit("validate", UnitKind::Function, "handler.ts", 100, 200);
    let validate_v2 = make_unit("validate", UnitKind::Function, "handler.ts", 100, 300);
    let graph_before = build_graph(vec![validate_v1.clone()], vec![]);
    let graph_after = build_graph(vec![validate_v2], vec![]);

    let alice = agent_id("alice");
    let locks = vec![SemanticLock {
        agent_id: alice,
        target: validate_v1.id,
        kind: IntentKind::Write,
    }];

    let report = ValidationEngine::validate(&graph_before, &graph_after, "alice", alice, &locks);
    assert!(report.issues.is_empty());
    assert_eq!(report.exit_code(), 0);
}

// ─────────────────────────────────────────────────────────────────────────────
// Section 2: Lock coverage errors
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn validate_modification_without_lock() {
    let validate_v1 = make_unit("validate", UnitKind::Function, "handler.ts", 100, 200);
    let validate_v2 = make_unit("validate", UnitKind::Function, "handler.ts", 100, 300);
    let graph_before = build_graph(vec![validate_v1], vec![]);
    let graph_after = build_graph(vec![validate_v2], vec![]);

    let report =
        ValidationEngine::validate(&graph_before, &graph_after, "alice", agent_id("alice"), &[]);
    assert_eq!(report.issues.len(), 1);
    assert_eq!(report.issues[0].severity, Severity::Error);
    assert!(matches!(
        report.issues[0].kind,
        ValidationKind::UnlockedModification { .. }
    ));
    assert_eq!(report.exit_code(), 1);
}

#[test]
fn validate_deletion_without_lock() {
    let validate = make_unit("validate", UnitKind::Function, "handler.ts", 100, 200);
    let graph_before = build_graph(vec![validate], vec![]);
    let graph_after = build_graph(vec![], vec![]);

    let report =
        ValidationEngine::validate(&graph_before, &graph_after, "alice", agent_id("alice"), &[]);
    assert_eq!(report.issues.len(), 1);
    assert_eq!(report.issues[0].severity, Severity::Error);
    assert!(matches!(
        report.issues[0].kind,
        ValidationKind::UnlockedDeletion { .. }
    ));
}

#[test]
fn validate_deletion_with_delete_lock() {
    let validate = make_unit("validate", UnitKind::Function, "handler.ts", 100, 200);
    let alice = agent_id("alice");
    let locks = vec![SemanticLock {
        agent_id: alice,
        target: validate.id,
        kind: IntentKind::Delete,
    }];
    let graph_before = build_graph(vec![validate], vec![]);
    let graph_after = build_graph(vec![], vec![]);

    let report = ValidationEngine::validate(&graph_before, &graph_after, "alice", alice, &locks);
    // Deletion is covered by Delete lock, but there may be no broken reference
    // since we have no callers in graph_before.
    let lock_issues: Vec<_> = report
        .issues
        .iter()
        .filter(|i| matches!(i.kind, ValidationKind::UnlockedDeletion { .. }))
        .collect();
    assert!(lock_issues.is_empty());
}

#[test]
fn validate_wrong_agent_lock_not_sufficient() {
    let validate_v1 = make_unit("validate", UnitKind::Function, "handler.ts", 100, 200);
    let validate_v2 = make_unit("validate", UnitKind::Function, "handler.ts", 100, 300);
    let graph_before = build_graph(vec![validate_v1.clone()], vec![]);
    let graph_after = build_graph(vec![validate_v2], vec![]);

    let bob = agent_id("bob");
    let locks = vec![SemanticLock {
        agent_id: bob,
        target: validate_v1.id,
        kind: IntentKind::Write,
    }];

    // Alice doesn't hold the lock — bob does.
    let report = ValidationEngine::validate(
        &graph_before,
        &graph_after,
        "alice",
        agent_id("alice"),
        &locks,
    );
    assert_eq!(report.issues.len(), 1);
    assert!(matches!(
        report.issues[0].kind,
        ValidationKind::UnlockedModification { .. }
    ));
}

// ─────────────────────────────────────────────────────────────────────────────
// Section 3: Reference integrity
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn validate_broken_reference_deleted_unit_still_called() {
    let validate = make_unit("validate", UnitKind::Function, "handler.ts", 100, 200);
    let process = make_unit("processRequest", UnitKind::Function, "handler.ts", 300, 400);
    // processRequest calls validate.
    let graph_before = build_graph(vec![validate.clone(), process.clone()], vec![(1, 0)]);
    // validate is deleted, processRequest remains unchanged.
    let graph_after = build_graph(vec![process], vec![]);

    let alice = agent_id("alice");
    let locks = vec![SemanticLock {
        agent_id: alice,
        target: validate.id,
        kind: IntentKind::Delete,
    }];

    let report = ValidationEngine::validate(&graph_before, &graph_after, "alice", alice, &locks);
    let broken: Vec<_> = report
        .issues
        .iter()
        .filter(|i| matches!(i.kind, ValidationKind::BrokenReference { .. }))
        .collect();
    assert_eq!(broken.len(), 1);
    assert_eq!(broken[0].severity, Severity::Error);
    assert!(broken[0].unit_name.contains("processRequest"));
}

#[test]
fn validate_no_broken_reference_when_caller_also_modified() {
    let validate = make_unit("validate", UnitKind::Function, "handler.ts", 100, 200);
    let process_v1 = make_unit("processRequest", UnitKind::Function, "handler.ts", 300, 400);
    let process_v2 = make_unit("processRequest", UnitKind::Function, "handler.ts", 300, 500);
    // processRequest calls validate.
    let graph_before = build_graph(vec![validate.clone(), process_v1.clone()], vec![(1, 0)]);
    // validate is deleted, processRequest was modified (body changed -> presumably updated).
    let graph_after = build_graph(vec![process_v2], vec![]);

    let alice = agent_id("alice");
    let locks = vec![
        SemanticLock {
            agent_id: alice,
            target: validate.id,
            kind: IntentKind::Delete,
        },
        SemanticLock {
            agent_id: alice,
            target: process_v1.id,
            kind: IntentKind::Write,
        },
    ];

    let report = ValidationEngine::validate(&graph_before, &graph_after, "alice", alice, &locks);
    let broken: Vec<_> = report
        .issues
        .iter()
        .filter(|i| matches!(i.kind, ValidationKind::BrokenReference { .. }))
        .collect();
    assert!(
        broken.is_empty(),
        "modified caller should not be flagged as broken reference"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Section 4: Stale callers
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn validate_stale_caller_after_signature_change() {
    let validate_v1 = make_unit("validate", UnitKind::Function, "handler.ts", 100, 200);
    let validate_v2 = make_unit("validate", UnitKind::Function, "handler.ts", 999, 300);
    let process = make_unit("processRequest", UnitKind::Function, "handler.ts", 300, 400);

    // processRequest calls validate.
    let graph_before = build_graph(vec![validate_v1.clone(), process.clone()], vec![(1, 0)]);
    // validate signature changed (100 -> 999), processRequest unchanged.
    let graph_after = build_graph(vec![validate_v2, process], vec![(1, 0)]);

    let alice = agent_id("alice");
    let locks = vec![SemanticLock {
        agent_id: alice,
        target: validate_v1.id,
        kind: IntentKind::Write,
    }];

    let report = ValidationEngine::validate(&graph_before, &graph_after, "alice", alice, &locks);
    let stale: Vec<_> = report
        .issues
        .iter()
        .filter(|i| matches!(i.kind, ValidationKind::StaleCallers { .. }))
        .collect();
    assert_eq!(stale.len(), 1);
    assert_eq!(stale[0].severity, Severity::Warning);
    assert!(stale[0].unit_name.contains("processRequest"));
}

#[test]
fn validate_no_stale_caller_when_caller_also_modified() {
    let validate_v1 = make_unit("validate", UnitKind::Function, "handler.ts", 100, 200);
    let validate_v2 = make_unit("validate", UnitKind::Function, "handler.ts", 999, 300);
    let process_v1 = make_unit("processRequest", UnitKind::Function, "handler.ts", 300, 400);
    let process_v2 = make_unit("processRequest", UnitKind::Function, "handler.ts", 300, 500);

    let graph_before = build_graph(vec![validate_v1.clone(), process_v1.clone()], vec![(1, 0)]);
    // Both validate and processRequest modified.
    let graph_after = build_graph(vec![validate_v2, process_v2], vec![(1, 0)]);

    let alice = agent_id("alice");
    let locks = vec![
        SemanticLock {
            agent_id: alice,
            target: validate_v1.id,
            kind: IntentKind::Write,
        },
        SemanticLock {
            agent_id: alice,
            target: process_v1.id,
            kind: IntentKind::Write,
        },
    ];

    let report = ValidationEngine::validate(&graph_before, &graph_after, "alice", alice, &locks);
    let stale: Vec<_> = report
        .issues
        .iter()
        .filter(|i| matches!(i.kind, ValidationKind::StaleCallers { .. }))
        .collect();
    assert!(
        stale.is_empty(),
        "modified caller should not be flagged as stale"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Section 5: Mixed scenarios
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn validate_multiple_issues() {
    let validate_v1 = make_unit("validate", UnitKind::Function, "handler.ts", 100, 200);
    let validate_v2 = make_unit("validate", UnitKind::Function, "handler.ts", 100, 300);
    let format_date = make_unit("formatDate", UnitKind::Function, "utils.ts", 500, 600);
    let process = make_unit("processRequest", UnitKind::Function, "handler.ts", 300, 400);

    // processRequest calls formatDate.
    let graph_before = build_graph(
        vec![validate_v1, format_date.clone(), process.clone()],
        vec![(2, 1)],
    );
    // validate was modified (no lock), formatDate was deleted (no lock),
    // processRequest still calls formatDate but formatDate is gone.
    let graph_after = build_graph(vec![validate_v2, process], vec![]);

    let report =
        ValidationEngine::validate(&graph_before, &graph_after, "alice", agent_id("alice"), &[]);

    // Should have:
    // 1. UnlockedModification for validate (no Write lock)
    // 2. UnlockedDeletion for formatDate (no Delete lock)
    // 3. BrokenReference for processRequest (still calls deleted formatDate)
    assert!(report.issues.len() >= 3);
    assert_eq!(report.exit_code(), 1);

    let kinds: Vec<_> = report.issues.iter().map(|i| &i.kind).collect();
    assert!(
        kinds
            .iter()
            .any(|k| matches!(k, ValidationKind::UnlockedModification { .. })),
        "expected UnlockedModification"
    );
    assert!(
        kinds
            .iter()
            .any(|k| matches!(k, ValidationKind::UnlockedDeletion { .. })),
        "expected UnlockedDeletion"
    );
    assert!(
        kinds
            .iter()
            .any(|k| matches!(k, ValidationKind::BrokenReference { .. })),
        "expected BrokenReference"
    );
}

#[test]
fn validate_units_checked_count() {
    let validate_v1 = make_unit("validate", UnitKind::Function, "handler.ts", 100, 200);
    let validate_v2 = make_unit("validate", UnitKind::Function, "handler.ts", 100, 300);
    let helper = make_unit("helper", UnitKind::Function, "utils.ts", 500, 600);
    let new_fn = make_unit("newFunction", UnitKind::Function, "new.ts", 700, 800);

    let graph_before = build_graph(vec![validate_v1, helper], vec![]);
    // validate modified, helper deleted, newFunction added.
    let graph_after = build_graph(vec![validate_v2, new_fn], vec![]);

    let report =
        ValidationEngine::validate(&graph_before, &graph_after, "alice", agent_id("alice"), &[]);
    // units_checked = modified(1) + removed(1) + added(1) = 3
    assert_eq!(report.units_checked, 3);
}
