//! nex-eventlog: event sourcing, replay, and semantic rollback primitives.
//!
//! This crate implements the next implementation-plan slice after validation:
//! a backend-selecting event log that records semantic mutations, can replay
//! historical state, and can generate compensating rollback events when no
//! later intent touched the same semantic units.

pub mod event;
pub mod store;

pub use event::{Mutation, SemanticEvent};
pub use store::{EventLog, RollbackConflict, RollbackOutcome};
