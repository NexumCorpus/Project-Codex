//! Custom `nex/*` LSP protocol types.

use nex_core::SemanticDiff;
use serde::{Deserialize, Serialize};
use tower_lsp::lsp_types::{self, DiagnosticSeverity, Range, Url};

/// Parameters for the `nex/semanticDiff` request.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticDiffParams {
    /// Base git ref to diff from. Defaults to the server config if absent.
    pub base_ref: Option<String>,
    /// Head git ref to diff to. Defaults to `HEAD` if absent.
    pub head_ref: Option<String>,
    /// Optional file URI to scope the semantic diff to a single document.
    pub uri: Option<Url>,
}

/// Lock annotation published through `nex/activeLocks`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ActiveLockAnnotation {
    /// Locked semantic unit.
    pub unit_name: String,
    /// Agent currently holding the lock.
    pub agent_name: String,
    /// Human-readable lock kind.
    pub lock_kind: String,
    /// Approximate in-document range for the unit.
    pub range: Range,
}

/// Notification payload for `nex/activeLocks`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ActiveLocksParams {
    /// Document being annotated.
    pub uri: Url,
    /// Active lock annotations for this document.
    pub locks: Vec<ActiveLockAnnotation>,
}

/// Agent-intent annotation published through `nex/agentIntent`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentIntentAnnotation {
    /// Agent identifier.
    pub agent_name: String,
    /// Target semantic unit.
    pub target_name: String,
    /// Human-readable summary shown by clients.
    pub message: String,
}

/// Notification payload for `nex/agentIntent`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentIntentParams {
    /// Document being annotated.
    pub uri: Url,
    /// Pending agent intent summaries.
    pub intents: Vec<AgentIntentAnnotation>,
}

/// Validation issue summary published through `nex/validationStatus`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ValidationIssueAnnotation {
    /// Severity that mirrors the LSP diagnostic severity.
    pub severity: DiagnosticSeverity,
    /// Issue message.
    pub message: String,
    /// Approximate in-document range for the issue.
    pub range: Range,
}

/// Notification payload for `nex/validationStatus`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ValidationStatusParams {
    /// Document being diagnosed.
    pub uri: Url,
    /// Current issue summaries for that document.
    pub issues: Vec<ValidationIssueAnnotation>,
}

/// Event summary published through `nex/eventStream`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EventStreamParams {
    /// Stable semantic event identifier.
    pub event_id: String,
    /// Event description.
    pub description: String,
    /// Agent that produced the event.
    pub agent_id: String,
    /// Free-form tags associated with the event.
    pub tags: Vec<String>,
    /// RFC3339 timestamp.
    pub timestamp: String,
}

/// Custom request trait for `nex/semanticDiff`.
pub enum CodexSemanticDiff {}

impl lsp_types::request::Request for CodexSemanticDiff {
    type Params = SemanticDiffParams;
    type Result = SemanticDiff;
    const METHOD: &'static str = "nex/semanticDiff";
}

/// Custom notification trait for `nex/activeLocks`.
pub enum CodexActiveLocks {}

impl lsp_types::notification::Notification for CodexActiveLocks {
    type Params = ActiveLocksParams;
    const METHOD: &'static str = "nex/activeLocks";
}

/// Custom notification trait for `nex/agentIntent`.
pub enum CodexAgentIntent {}

impl lsp_types::notification::Notification for CodexAgentIntent {
    type Params = AgentIntentParams;
    const METHOD: &'static str = "nex/agentIntent";
}

/// Custom notification trait for `nex/validationStatus`.
pub enum CodexValidationStatus {}

impl lsp_types::notification::Notification for CodexValidationStatus {
    type Params = ValidationStatusParams;
    const METHOD: &'static str = "nex/validationStatus";
}

/// Custom notification trait for `nex/eventStream`.
pub enum CodexEventStream {}

impl lsp_types::notification::Notification for CodexEventStream {
    type Params = EventStreamParams;
    const METHOD: &'static str = "nex/eventStream";
}
