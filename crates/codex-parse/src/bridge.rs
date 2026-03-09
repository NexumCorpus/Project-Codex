//! Tree-sitter -> Rowan CST bridge infrastructure.
//!
//! Phase 0 only needs the syntax kind registry so downstream bridge work can
//! assign stable `rowan::SyntaxKind` values to tree-sitter node kinds.

use rowan::SyntaxKind;
use std::collections::HashMap;

/// Bidirectional registry for tree-sitter kind strings and rowan syntax kinds.
#[derive(Debug, Clone, Default)]
pub struct KindMap {
    by_name: HashMap<String, SyntaxKind>,
    by_kind: Vec<String>,
}

impl KindMap {
    /// Create an empty kind registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create an empty kind registry with reserved capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            by_name: HashMap::with_capacity(capacity),
            by_kind: Vec::with_capacity(capacity),
        }
    }

    /// Return the number of interned kinds.
    pub fn len(&self) -> usize {
        self.by_kind.len()
    }

    /// Returns true when no kinds have been interned yet.
    pub fn is_empty(&self) -> bool {
        self.by_kind.is_empty()
    }

    /// Look up the rowan kind for a tree-sitter kind string.
    pub fn get(&self, kind: &str) -> Option<SyntaxKind> {
        self.by_name.get(kind).copied()
    }

    /// Look up the original tree-sitter kind string for a rowan kind.
    pub fn name(&self, kind: SyntaxKind) -> Option<&str> {
        self.by_kind.get(kind.0 as usize).map(String::as_str)
    }

    /// Intern a tree-sitter kind string, returning a stable rowan kind.
    pub fn intern(&mut self, kind: impl AsRef<str>) -> SyntaxKind {
        let kind = kind.as_ref();
        if let Some(existing) = self.get(kind) {
            return existing;
        }

        let raw_kind =
            u16::try_from(self.by_kind.len()).expect("KindMap supports at most u16::MAX kinds");
        let syntax_kind = SyntaxKind(raw_kind);
        let owned = kind.to_owned();
        self.by_name.insert(owned.clone(), syntax_kind);
        self.by_kind.push(owned);
        syntax_kind
    }

    /// Iterate over all registered syntax kinds in insertion order.
    pub fn iter(&self) -> impl Iterator<Item = (SyntaxKind, &str)> {
        self.by_kind
            .iter()
            .enumerate()
            .map(|(index, name)| (SyntaxKind(index as u16), name.as_str()))
    }
}
