//! codex-graph: petgraph-based semantic code graph with diff capability.
//!
//! This crate owns:
//! - `CodeGraph` — a directed graph of semantic units with dependency edges
//! - Graph construction from `SemanticUnit` vectors
//! - Graph diff algorithm (match by `qualified_name`, compare hashes)
//! - Caller/dependency queries
//!
//! The `CodeGraph` API is specified in the Implementation Specification and
//! constitutes a contract. Codex implements the function bodies.

use codex_core::{
    ChangeKind, DepKind, ModifiedUnit, MovedUnit, SemanticDiff, SemanticId, SemanticUnit,
};
use petgraph::graph::{DiGraph, NodeIndex};
use std::collections::HashMap;

/// A dependency edge in the code graph.
#[derive(Debug, Clone)]
pub struct DepEdge {
    pub kind: DepKind,
}

/// A node wrapper in the code graph.
#[derive(Debug, Clone)]
pub struct SemanticNode {
    pub unit: SemanticUnit,
}

/// Semantic code graph backed by petgraph.
///
/// Provides O(1) lookups by `SemanticId`, directed dependency edges,
/// and a `diff()` method that compares two graphs to produce a `SemanticDiff`.
pub struct CodeGraph {
    graph: DiGraph<SemanticNode, DepEdge>,
    index: HashMap<SemanticId, NodeIndex>,
    /// Secondary index: qualified_name → NodeIndex for diff matching.
    name_index: HashMap<String, NodeIndex>,
}

impl CodeGraph {
    /// Create an empty code graph.
    pub fn new() -> Self {
        todo!("Codex: initialize empty graph + indexes")
    }

    /// Add a semantic unit as a node. Returns its NodeIndex.
    pub fn add_unit(&mut self, unit: SemanticUnit) -> NodeIndex {
        todo!("Codex: insert node, update both indexes")
    }

    /// Add a dependency edge between two units.
    pub fn add_dep(&mut self, from: SemanticId, to: SemanticId, kind: DepKind) {
        todo!("Codex: look up NodeIndexes, add edge")
    }

    /// Look up a unit by its SemanticId.
    pub fn get(&self, id: &SemanticId) -> Option<&SemanticUnit> {
        todo!("Codex: index lookup → node weight")
    }

    /// Find all units that call/depend on the given unit (incoming edges).
    pub fn callers_of(&self, id: &SemanticId) -> Vec<&SemanticUnit> {
        todo!("Codex: walk incoming edges")
    }

    /// Find all units that the given unit depends on (outgoing edges).
    pub fn deps_of(&self, id: &SemanticId) -> Vec<&SemanticUnit> {
        todo!("Codex: walk outgoing edges")
    }

    /// Return all units in the graph.
    pub fn units(&self) -> Vec<&SemanticUnit> {
        todo!("Codex: iterate all node weights")
    }

    /// Return the number of units in the graph.
    pub fn unit_count(&self) -> usize {
        self.graph.node_count()
    }

    /// Return the number of dependency edges in the graph.
    pub fn edge_count(&self) -> usize {
        self.graph.edge_count()
    }

    /// Compute the semantic diff between `self` (before) and `other` (after).
    ///
    /// Algorithm (from spec):
    /// - Match units by `qualified_name`
    /// - Same name + different `signature_hash` → SignatureChanged
    /// - Same signature + different `body_hash` → BodyChanged
    /// - Same `body_hash` + different `file_path` → Moved
    /// - Unmatched in `other` → Added
    /// - Unmatched in `self` → Removed
    pub fn diff(&self, other: &CodeGraph) -> SemanticDiff {
        todo!("Codex: implement graph diff algorithm per spec")
    }
}

impl Default for CodeGraph {
    fn default() -> Self {
        Self::new()
    }
}
