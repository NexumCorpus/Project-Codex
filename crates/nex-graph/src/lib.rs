//! nex-graph: petgraph-based semantic code graph with diff capability.
//!
//! This crate owns:
//! - `CodeGraph` - a directed graph of semantic units with dependency edges
//! - Graph construction from `SemanticUnit` vectors
//! - Graph diff algorithm (match by `qualified_name`, compare hashes)
//! - Caller/dependency queries
//!
//! The `CodeGraph` API is specified in the Implementation Specification and
//! constitutes a contract. Codex implements the function bodies.

use nex_core::{
    ChangeKind, DepKind, ModifiedUnit, MovedUnit, SemanticDiff, SemanticId, SemanticUnit,
};
use petgraph::Direction;
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
    /// Secondary index: qualified_name -> NodeIndex for diff matching.
    name_index: HashMap<String, NodeIndex>,
}

impl CodeGraph {
    /// Create an empty code graph.
    pub fn new() -> Self {
        Self {
            graph: DiGraph::new(),
            index: HashMap::new(),
            name_index: HashMap::new(),
        }
    }

    /// Add a semantic unit as a node. Returns its NodeIndex.
    pub fn add_unit(&mut self, unit: SemanticUnit) -> NodeIndex {
        let qualified_name = unit.qualified_name.clone();
        let id = unit.id;
        let index = self.graph.add_node(SemanticNode { unit });
        self.index.insert(id, index);
        self.name_index.insert(qualified_name, index);
        index
    }

    /// Add a dependency edge between two units.
    pub fn add_dep(&mut self, from: SemanticId, to: SemanticId, kind: DepKind) {
        let Some(from_index) = self.index.get(&from).copied() else {
            return;
        };
        let Some(to_index) = self.index.get(&to).copied() else {
            return;
        };

        self.graph.add_edge(from_index, to_index, DepEdge { kind });
    }

    /// Look up a unit by its SemanticId.
    pub fn get(&self, id: &SemanticId) -> Option<&SemanticUnit> {
        let index = self.index.get(id)?;
        self.graph.node_weight(*index).map(|node| &node.unit)
    }

    /// Find all units that call/depend on the given unit (incoming edges).
    pub fn callers_of(&self, id: &SemanticId) -> Vec<&SemanticUnit> {
        let Some(index) = self.index.get(id).copied() else {
            return Vec::new();
        };

        self.graph
            .neighbors_directed(index, Direction::Incoming)
            .filter_map(|neighbor| self.graph.node_weight(neighbor).map(|node| &node.unit))
            .collect()
    }

    /// Find all units that the given unit depends on (outgoing edges).
    pub fn deps_of(&self, id: &SemanticId) -> Vec<&SemanticUnit> {
        let Some(index) = self.index.get(id).copied() else {
            return Vec::new();
        };

        self.graph
            .neighbors_directed(index, Direction::Outgoing)
            .filter_map(|neighbor| self.graph.node_weight(neighbor).map(|node| &node.unit))
            .collect()
    }

    /// Return all units in the graph.
    pub fn units(&self) -> Vec<&SemanticUnit> {
        self.graph.node_weights().map(|node| &node.unit).collect()
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
    /// - Same name + different `signature_hash` -> SignatureChanged
    /// - Same signature + different `body_hash` -> BodyChanged
    /// - Same `body_hash` + different `file_path` -> Moved
    /// - Unmatched in `other` -> Added
    /// - Unmatched in `self` -> Removed
    pub fn diff(&self, other: &CodeGraph) -> SemanticDiff {
        let self_by_name = self.units_by_name();
        let other_by_name = other.units_by_name();

        let mut added = Vec::new();
        let mut removed = Vec::new();
        let mut modified = Vec::new();
        let mut moved = Vec::new();

        for (name, other_unit) in &other_by_name {
            if let Some(self_unit) = self_by_name.get(name) {
                if self_unit.body_hash == other_unit.body_hash
                    && self_unit.signature_hash == other_unit.signature_hash
                {
                    if self_unit.file_path != other_unit.file_path {
                        moved.push(MovedUnit {
                            unit: (*other_unit).clone(),
                            old_path: self_unit.file_path.clone(),
                            new_path: other_unit.file_path.clone(),
                        });
                    }
                    continue;
                }

                let mut changes = Vec::new();
                if self_unit.signature_hash != other_unit.signature_hash {
                    changes.push(ChangeKind::SignatureChanged);
                }
                if self_unit.body_hash != other_unit.body_hash {
                    changes.push(ChangeKind::BodyChanged);
                }

                modified.push(ModifiedUnit {
                    before: (*self_unit).clone(),
                    after: (*other_unit).clone(),
                    changes,
                });
            } else {
                added.push((*other_unit).clone());
            }
        }

        for (name, self_unit) in &self_by_name {
            if !other_by_name.contains_key(name) {
                removed.push((*self_unit).clone());
            }
        }

        SemanticDiff {
            added,
            removed,
            modified,
            moved,
        }
    }

    fn units_by_name(&self) -> HashMap<&str, &SemanticUnit> {
        self.name_index
            .iter()
            .filter_map(|(name, index)| {
                self.graph
                    .node_weight(*index)
                    .map(|node| (name.as_str(), &node.unit))
            })
            .collect()
    }
}

impl Default for CodeGraph {
    fn default() -> Self {
        Self::new()
    }
}
