use chrono::{DateTime, Utc};
use nex_core::{SemanticId, SemanticUnit};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use uuid::Uuid;

/// An append-only semantic event emitted by an approved intent.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SemanticEvent {
    pub id: Uuid,
    pub timestamp: DateTime<Utc>,
    pub intent_id: Uuid,
    pub agent_id: String,
    pub description: String,
    pub mutations: Vec<Mutation>,
    pub parent_event: Option<Uuid>,
    pub tags: Vec<String>,
}

impl SemanticEvent {
    /// Construct a new semantic event with generated id and current timestamp.
    pub fn new(
        intent_id: Uuid,
        agent_id: impl Into<String>,
        description: impl Into<String>,
        mutations: Vec<Mutation>,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            timestamp: Utc::now(),
            intent_id,
            agent_id: agent_id.into(),
            description: description.into(),
            mutations,
            parent_event: None,
            tags: Vec::new(),
        }
    }

    /// All semantic unit ids touched by this event.
    pub fn touched_units(&self) -> Vec<SemanticId> {
        let mut ids = Vec::new();
        for mutation in &self.mutations {
            for id in mutation.touched_units() {
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
        }
        ids
    }
}

/// A semantic mutation captured in the event log.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum Mutation {
    AddUnit {
        unit: SemanticUnit,
    },
    RemoveUnit {
        id: SemanticId,
        snapshot: SemanticUnit,
    },
    ModifyUnit {
        id: SemanticId,
        before: SemanticUnit,
        after: SemanticUnit,
    },
    MoveUnit {
        id: SemanticId,
        from: PathBuf,
        to: PathBuf,
    },
    RenameUnit {
        id: SemanticId,
        from: String,
        to: String,
    },
}

impl Mutation {
    /// Generate the compensating mutation that undoes this one.
    pub fn compensate(&self) -> Mutation {
        match self {
            Mutation::AddUnit { unit } => Mutation::RemoveUnit {
                id: unit.id,
                snapshot: unit.clone(),
            },
            Mutation::RemoveUnit { snapshot, .. } => Mutation::AddUnit {
                unit: snapshot.clone(),
            },
            Mutation::ModifyUnit { id, before, after } => Mutation::ModifyUnit {
                id: *id,
                before: after.clone(),
                after: before.clone(),
            },
            Mutation::MoveUnit { id, from, to } => Mutation::MoveUnit {
                id: *id,
                from: to.clone(),
                to: from.clone(),
            },
            Mutation::RenameUnit { id, from, to } => Mutation::RenameUnit {
                id: *id,
                from: to.clone(),
                to: from.clone(),
            },
        }
    }

    /// All semantic ids touched by this mutation.
    pub fn touched_units(&self) -> Vec<SemanticId> {
        match self {
            Mutation::AddUnit { unit } => vec![unit.id],
            Mutation::RemoveUnit { id, snapshot } => {
                if *id == snapshot.id {
                    vec![*id]
                } else {
                    vec![*id, snapshot.id]
                }
            }
            Mutation::ModifyUnit { id, before, after } => {
                let mut ids = vec![*id];
                if before.id != *id && !ids.contains(&before.id) {
                    ids.push(before.id);
                }
                if after.id != *id && !ids.contains(&after.id) {
                    ids.push(after.id);
                }
                ids
            }
            Mutation::MoveUnit { id, .. } => vec![*id],
            Mutation::RenameUnit { id, .. } => vec![*id],
        }
    }

    /// Apply the mutation to a semantic-unit state map.
    pub fn apply(&self, units: &mut HashMap<SemanticId, SemanticUnit>) {
        match self {
            Mutation::AddUnit { unit } => {
                units.insert(unit.id, unit.clone());
            }
            Mutation::RemoveUnit { id, .. } => {
                units.remove(id);
            }
            Mutation::ModifyUnit { id, after, .. } => {
                units.remove(id);
                units.insert(after.id, after.clone());
            }
            Mutation::MoveUnit { id, to, .. } => {
                if let Some(unit) = units.get_mut(id) {
                    unit.file_path = to.clone();
                }
            }
            Mutation::RenameUnit { id, to, .. } => {
                if let Some(unit) = units.get_mut(id) {
                    unit.qualified_name = to.clone();
                    unit.name = to.rsplit("::").next().unwrap_or(to.as_str()).to_string();
                }
            }
        }
    }
}
