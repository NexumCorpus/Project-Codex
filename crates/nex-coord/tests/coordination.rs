//! Integration tests for the nex-coord coordination engine.
//!
//! These tests exercise the Phase 2 semantic lock table:
//! - Basic lock acquisition (Read, Write)
//! - Lock compatibility matrix (Read+Read OK, everything else denied)
//! - Same-agent rules (no duplicate locks, but related units OK)
//! - Transitive conflict detection via CodeGraph edges
//! - Lock release (specific, all, error on nonexistent)
//! - State snapshot queries
//! - Export/Import for persistence (Prompt 006)
//!
//! Sections 1-6: Acceptance criteria for Prompt 005. Do NOT modify.
//! Section 7: Acceptance criteria for Prompt 006.

use nex_coord::CoordinationEngine;
use nex_core::{
    AgentId, DepKind, Intent, IntentKind, LockResult, SemanticLock, SemanticUnit, UnitKind,
};
use nex_graph::CodeGraph;

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Create a deterministic AgentId from a seed byte.
fn make_agent(seed: u8) -> AgentId {
    [seed; 16]
}

/// Create a SemanticUnit with a deterministic BLAKE3 id.
fn make_function(name: &str, file: &str) -> SemanticUnit {
    let id_input = format!("{name}:{file}");
    let id = *blake3::hash(id_input.as_bytes()).as_bytes();
    SemanticUnit {
        id,
        kind: UnitKind::Function,
        name: name.to_string(),
        qualified_name: name.to_string(),
        file_path: file.into(),
        byte_range: 0..100,
        signature_hash: 0,
        body_hash: 0,
        dependencies: vec![],
    }
}

/// Build a CoordinationEngine with a dependency graph:
///
/// ```text
///   processRequest ──calls──> validate
///   formatDate (isolated, no edges)
/// ```
///
/// Returns (engine, processRequest, validate, formatDate).
fn engine_with_deps() -> (CoordinationEngine, SemanticUnit, SemanticUnit, SemanticUnit) {
    let caller = make_function("processRequest", "handler.ts");
    let callee = make_function("validate", "auth.ts");
    let independent = make_function("formatDate", "utils.ts");

    let mut graph = CodeGraph::new();
    graph.add_unit(caller.clone());
    graph.add_unit(callee.clone());
    graph.add_unit(independent.clone());
    graph.add_dep(caller.id, callee.id, DepKind::Calls);

    (CoordinationEngine::new(graph), caller, callee, independent)
}

// ─────────────────────────────────────────────────────────────────────────────
// 1. Basic lock acquisition
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn grant_write_on_free_unit() {
    let (mut engine, caller, _, _) = engine_with_deps();
    let agent_a = make_agent(1);

    let result = engine.request_lock(Intent {
        agent_id: agent_a,
        target: caller.id,
        kind: IntentKind::Write,
    });

    assert!(matches!(result, LockResult::Granted));
    assert_eq!(engine.active_locks().len(), 1);
}

#[test]
fn grant_read_on_free_unit() {
    let (mut engine, caller, _, _) = engine_with_deps();
    let agent_a = make_agent(1);

    let result = engine.request_lock(Intent {
        agent_id: agent_a,
        target: caller.id,
        kind: IntentKind::Read,
    });

    assert!(matches!(result, LockResult::Granted));
}

// ─────────────────────────────────────────────────────────────────────────────
// 2. Lock compatibility
// ─────────────────────────────────────────────────────────────────────────────

/// Multiple readers on the same unit should be allowed.
#[test]
fn multiple_readers_compatible() {
    let (mut engine, caller, _, _) = engine_with_deps();
    let agent_a = make_agent(1);
    let agent_b = make_agent(2);

    let r1 = engine.request_lock(Intent {
        agent_id: agent_a,
        target: caller.id,
        kind: IntentKind::Read,
    });
    let r2 = engine.request_lock(Intent {
        agent_id: agent_b,
        target: caller.id,
        kind: IntentKind::Read,
    });

    assert!(matches!(r1, LockResult::Granted));
    assert!(matches!(r2, LockResult::Granted));
    assert_eq!(engine.locks_for_unit(&caller.id).len(), 2);
}

/// An existing Write lock should block a new Read request from another agent.
#[test]
fn write_blocks_read() {
    let (mut engine, caller, _, _) = engine_with_deps();
    let agent_a = make_agent(1);
    let agent_b = make_agent(2);

    engine.request_lock(Intent {
        agent_id: agent_a,
        target: caller.id,
        kind: IntentKind::Write,
    });

    let result = engine.request_lock(Intent {
        agent_id: agent_b,
        target: caller.id,
        kind: IntentKind::Read,
    });

    assert!(matches!(result, LockResult::Denied { .. }));
}

/// An existing Read lock should block a new Write request from another agent.
#[test]
fn read_blocks_write() {
    let (mut engine, caller, _, _) = engine_with_deps();
    let agent_a = make_agent(1);
    let agent_b = make_agent(2);

    engine.request_lock(Intent {
        agent_id: agent_a,
        target: caller.id,
        kind: IntentKind::Read,
    });

    let result = engine.request_lock(Intent {
        agent_id: agent_b,
        target: caller.id,
        kind: IntentKind::Write,
    });

    assert!(matches!(result, LockResult::Denied { .. }));
}

/// Two Write locks on the same unit from different agents should conflict.
#[test]
fn write_blocks_write() {
    let (mut engine, caller, _, _) = engine_with_deps();
    let agent_a = make_agent(1);
    let agent_b = make_agent(2);

    engine.request_lock(Intent {
        agent_id: agent_a,
        target: caller.id,
        kind: IntentKind::Write,
    });

    let result = engine.request_lock(Intent {
        agent_id: agent_b,
        target: caller.id,
        kind: IntentKind::Write,
    });

    assert!(matches!(result, LockResult::Denied { .. }));
}

// ─────────────────────────────────────────────────────────────────────────────
// 3. Same-agent rules
// ─────────────────────────────────────────────────────────────────────────────

/// An agent should not be able to hold two locks on the same target.
#[test]
fn same_agent_cannot_double_lock() {
    let (mut engine, caller, _, _) = engine_with_deps();
    let agent_a = make_agent(1);

    engine.request_lock(Intent {
        agent_id: agent_a,
        target: caller.id,
        kind: IntentKind::Write,
    });

    let result = engine.request_lock(Intent {
        agent_id: agent_a,
        target: caller.id,
        kind: IntentKind::Read,
    });

    assert!(matches!(result, LockResult::Denied { .. }));
}

/// The same agent should be able to lock related units (no self-conflict).
/// Transitive checks only flag conflicts between *different* agents.
#[test]
fn same_agent_can_lock_related_units() {
    let (mut engine, caller, callee, _) = engine_with_deps();
    let agent_a = make_agent(1);

    // Lock callee (validate) first
    engine.request_lock(Intent {
        agent_id: agent_a,
        target: callee.id,
        kind: IntentKind::Write,
    });

    // Lock caller (processRequest) which depends on callee — should be OK
    // because the same agent holds both locks.
    let result = engine.request_lock(Intent {
        agent_id: agent_a,
        target: caller.id,
        kind: IntentKind::Write,
    });

    assert!(
        matches!(result, LockResult::Granted),
        "same agent should be able to lock related units"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// 4. Transitive conflict detection
// ─────────────────────────────────────────────────────────────────────────────

/// Agent A writes to validate (callee).
/// Agent B tries to write processRequest (caller of validate).
/// processRequest depends on validate -> transitive conflict.
#[test]
fn transitive_conflict_via_dependency() {
    let (mut engine, caller, callee, _) = engine_with_deps();
    let agent_a = make_agent(1);
    let agent_b = make_agent(2);

    engine.request_lock(Intent {
        agent_id: agent_a,
        target: callee.id,
        kind: IntentKind::Write,
    });

    let result = engine.request_lock(Intent {
        agent_id: agent_b,
        target: caller.id,
        kind: IntentKind::Write,
    });

    assert!(matches!(result, LockResult::Denied { .. }));
    if let LockResult::Denied { conflicts } = result {
        assert!(
            !conflicts.is_empty(),
            "should report at least one transitive conflict"
        );
    }
}

/// Agent A writes to processRequest (caller).
/// Agent B tries to write validate (callee of processRequest).
/// validate is called by processRequest -> transitive conflict.
#[test]
fn transitive_conflict_via_caller() {
    let (mut engine, caller, callee, _) = engine_with_deps();
    let agent_a = make_agent(1);
    let agent_b = make_agent(2);

    engine.request_lock(Intent {
        agent_id: agent_a,
        target: caller.id,
        kind: IntentKind::Write,
    });

    let result = engine.request_lock(Intent {
        agent_id: agent_b,
        target: callee.id,
        kind: IntentKind::Write,
    });

    assert!(matches!(result, LockResult::Denied { .. }));
}

/// Agent A writes to processRequest.
/// Agent B writes to formatDate (isolated, no edges to processRequest).
/// No transitive conflict -> should be granted.
#[test]
fn no_transitive_conflict_for_independent_units() {
    let (mut engine, caller, _, independent) = engine_with_deps();
    let agent_a = make_agent(1);
    let agent_b = make_agent(2);

    engine.request_lock(Intent {
        agent_id: agent_a,
        target: caller.id,
        kind: IntentKind::Write,
    });

    let result = engine.request_lock(Intent {
        agent_id: agent_b,
        target: independent.id,
        kind: IntentKind::Write,
    });

    assert!(matches!(result, LockResult::Granted));
}

/// Read locks should NOT trigger transitive conflict detection.
/// Agent A writes to validate.
/// Agent B reads processRequest (which depends on validate) -> Granted.
#[test]
fn read_lock_no_transitive_conflict() {
    let (mut engine, caller, callee, _) = engine_with_deps();
    let agent_a = make_agent(1);
    let agent_b = make_agent(2);

    engine.request_lock(Intent {
        agent_id: agent_a,
        target: callee.id,
        kind: IntentKind::Write,
    });

    let result = engine.request_lock(Intent {
        agent_id: agent_b,
        target: caller.id,
        kind: IntentKind::Read,
    });

    assert!(
        matches!(result, LockResult::Granted),
        "Read requests should not trigger transitive conflict checks"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// 5. Lock release
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn release_specific_lock() {
    let (mut engine, caller, _, _) = engine_with_deps();
    let agent_a = make_agent(1);

    engine.request_lock(Intent {
        agent_id: agent_a,
        target: caller.id,
        kind: IntentKind::Write,
    });

    assert_eq!(engine.active_locks().len(), 1);
    engine
        .release_lock(&agent_a, &caller.id)
        .expect("release should succeed");
    assert_eq!(engine.active_locks().len(), 0);
}

#[test]
fn release_all_agent_locks() {
    let (mut engine, caller, _, independent) = engine_with_deps();
    let agent_a = make_agent(1);

    engine.request_lock(Intent {
        agent_id: agent_a,
        target: caller.id,
        kind: IntentKind::Write,
    });
    engine.request_lock(Intent {
        agent_id: agent_a,
        target: independent.id,
        kind: IntentKind::Read,
    });

    assert_eq!(engine.locks_for_agent(&agent_a).len(), 2);
    engine.release_all(&agent_a);
    assert_eq!(engine.locks_for_agent(&agent_a).len(), 0);
    assert_eq!(engine.active_locks().len(), 0);
}

#[test]
fn release_nonexistent_returns_error() {
    let (mut engine, caller, _, _) = engine_with_deps();
    let agent_a = make_agent(1);

    let result = engine.release_lock(&agent_a, &caller.id);
    assert!(
        result.is_err(),
        "releasing a lock you don't hold should error"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// 6. State queries
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn state_snapshot() {
    let (mut engine, caller, callee, _) = engine_with_deps();
    let agent_a = make_agent(1);
    let agent_b = make_agent(2);

    engine.request_lock(Intent {
        agent_id: agent_a,
        target: caller.id,
        kind: IntentKind::Write,
    });
    engine.request_lock(Intent {
        agent_id: agent_b,
        target: callee.id,
        kind: IntentKind::Read,
    });

    let state = engine.state();
    assert_eq!(state.locks.len(), 2);
    assert_eq!(state.agent_count, 2);
}

// ─────────────────────────────────────────────────────────────────────────────
// 7. Export / Import (persistence support for Prompt 006)
// ─────────────────────────────────────────────────────────────────────────────

/// Exporting from an engine with no locks should return an empty vec.
#[test]
fn export_empty_engine() {
    let graph = CodeGraph::new();
    let engine = CoordinationEngine::new(graph);
    assert!(engine.export_locks().is_empty());
}

/// Exporting should return all currently held locks.
#[test]
fn export_returns_all_locks() {
    let (mut engine, caller, callee, _) = engine_with_deps();
    let agent_a = make_agent(1);

    engine.request_lock(Intent {
        agent_id: agent_a,
        target: caller.id,
        kind: IntentKind::Write,
    });
    engine.request_lock(Intent {
        agent_id: agent_a,
        target: callee.id,
        kind: IntentKind::Read,
    });

    let exported = engine.export_locks();
    assert_eq!(exported.len(), 2);
}

/// Importing locks should replace the lock table and support conflict detection.
#[test]
fn import_restores_and_replaces_locks() {
    let caller = make_function("processRequest", "handler.ts");
    let callee = make_function("validate", "auth.ts");
    let independent = make_function("formatDate", "utils.ts");

    let mut graph = CodeGraph::new();
    graph.add_unit(caller.clone());
    graph.add_unit(callee.clone());
    graph.add_unit(independent.clone());
    graph.add_dep(caller.id, callee.id, DepKind::Calls);

    let agent_a = make_agent(1);
    let agent_b = make_agent(2);

    let mut engine = CoordinationEngine::new(graph);

    // Grant a lock normally.
    engine.request_lock(Intent {
        agent_id: agent_a,
        target: caller.id,
        kind: IntentKind::Write,
    });
    assert_eq!(engine.active_locks().len(), 1);

    // Import a DIFFERENT set of locks (should replace, not append).
    let imported = vec![SemanticLock {
        agent_id: agent_b,
        target: callee.id,
        kind: IntentKind::Read,
    }];
    engine.import_locks(imported);

    // Old lock on caller should be gone; new lock on callee should be present.
    assert_eq!(engine.active_locks().len(), 1);
    assert_eq!(engine.locks_for_unit(&caller.id).len(), 0);
    assert_eq!(engine.locks_for_unit(&callee.id).len(), 1);

    // Conflict detection should work against imported locks:
    // agent_a tries to Write callee — should conflict with agent_b's Read.
    let result = engine.request_lock(Intent {
        agent_id: agent_a,
        target: callee.id,
        kind: IntentKind::Write,
    });
    assert!(matches!(result, LockResult::Denied { .. }));
}
