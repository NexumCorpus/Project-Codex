//! `tower-lsp` backend for the Codex semantic shim.

use crate::config::CodexLspConfig;
use crate::protocol::{
    ActiveLockAnnotation, ActiveLocksParams, AgentIntentAnnotation, AgentIntentParams,
    CodexActiveLocks, CodexAgentIntent, CodexEventStream, CodexValidationStatus, EventStreamParams,
    SemanticDiffParams, ValidationIssueAnnotation, ValidationStatusParams,
};
use crate::upstream::UpstreamSession;
use git2::{ObjectType, TreeWalkMode, TreeWalkResult};
use nex_core::{ChangeKind, CodexError, CodexResult, IntentKind, SemanticDiff, SemanticUnit};
use nex_eventlog::{EventLog, SemanticEvent};
use nex_graph::CodeGraph;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tokio::sync::{Mutex, mpsc};
use tower_lsp::jsonrpc::{Error, ErrorCode, Result};
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, ClientSocket, LanguageServer, LspService};

#[derive(Debug, Clone)]
struct OpenDocument {
    version: i32,
    text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedLockEntry {
    agent_name: String,
    target_name: String,
    target: [u8; 32],
    kind: IntentKind,
}

struct SharedState {
    config: Mutex<CodexLspConfig>,
    open_documents: Mutex<HashMap<Url, OpenDocument>>,
    seen_events: Mutex<HashSet<String>>,
    local_diagnostics: Mutex<HashMap<Url, Vec<Diagnostic>>>,
    upstream_diagnostics: Mutex<HashMap<Url, Vec<Diagnostic>>>,
    upstream: Mutex<Option<Arc<UpstreamSession>>>,
    watcher_started: AtomicBool,
}

#[derive(Debug, Clone)]
struct ValidationFinding {
    severity: DiagnosticSeverity,
    message: String,
    range: Range,
}

/// LSP backend for Codex semantic coordination.
#[derive(Clone)]
pub struct CodexLspBackend {
    client: Client,
    shared: Arc<SharedState>,
}

impl CodexLspBackend {
    /// Create a new backend with the provided client handle and runtime config.
    pub fn new(client: Client, config: CodexLspConfig) -> Self {
        Self {
            client,
            shared: Arc::new(SharedState {
                config: Mutex::new(config),
                open_documents: Mutex::new(HashMap::new()),
                seen_events: Mutex::new(HashSet::new()),
                local_diagnostics: Mutex::new(HashMap::new()),
                upstream_diagnostics: Mutex::new(HashMap::new()),
                upstream: Mutex::new(None),
                watcher_started: AtomicBool::new(false),
            }),
        }
    }

    /// Custom `nex/semanticDiff` handler.
    pub async fn semantic_diff(&self, params: SemanticDiffParams) -> Result<SemanticDiff> {
        let repo_path = self
            .repo_path()
            .await
            .ok_or_else(|| Error::invalid_params("no repository path configured"))?;
        let base_ref = params.base_ref.unwrap_or_else(|| "HEAD~1".to_string());
        let head_ref = params.head_ref.unwrap_or_else(|| "HEAD".to_string());
        let full_diff = compute_diff_between_refs(&repo_path, &base_ref, &head_ref)
            .map_err(to_jsonrpc_error)?;

        if let Some(uri) = params.uri {
            let relative = relative_repo_path(&repo_path, &uri).map_err(to_jsonrpc_error)?;
            Ok(filter_diff_to_path(&full_diff, &relative))
        } else {
            Ok(full_diff)
        }
    }

    /// Compute active lock annotations for a document.
    pub async fn active_lock_annotations_for(
        &self,
        uri: &Url,
    ) -> CodexResult<Vec<ActiveLockAnnotation>> {
        self.active_lock_annotations(uri).await
    }

    /// Compute validation-status annotations for a document.
    pub async fn validation_status_for(
        &self,
        uri: &Url,
    ) -> CodexResult<Vec<ValidationIssueAnnotation>> {
        let findings = self.validation_findings(uri).await?;
        Ok(findings
            .iter()
            .map(|finding| ValidationIssueAnnotation {
                severity: finding.severity,
                message: finding.message.clone(),
                range: finding.range,
            })
            .collect())
    }

    /// Collect newly observed semantic event payloads.
    pub async fn collect_new_event_stream_params(&self) -> CodexResult<Vec<EventStreamParams>> {
        let Some(repo_path) = self.repo_path().await else {
            return Ok(Vec::new());
        };

        let events = EventLog::for_repo(&repo_path).list().await?;
        let mut seen_events = self.shared.seen_events.lock().await;
        let mut payloads = Vec::new();

        for event in events {
            let key = event.id.to_string();
            if seen_events.insert(key) {
                payloads.push(event_stream_params(&event));
            }
        }

        Ok(payloads)
    }

    async fn repo_path(&self) -> Option<PathBuf> {
        self.shared.config.lock().await.repo_path.clone()
    }

    async fn store_open_document(&self, uri: Url, version: i32, text: String) {
        self.shared
            .open_documents
            .lock()
            .await
            .insert(uri, OpenDocument { version, text });
    }

    async fn update_open_document(&self, uri: &Url, version: i32, text: String) {
        self.shared
            .open_documents
            .lock()
            .await
            .insert(uri.clone(), OpenDocument { version, text });
    }

    async fn remove_open_document(&self, uri: &Url) {
        self.shared.open_documents.lock().await.remove(uri);
    }

    async fn open_document(&self, uri: &Url) -> Option<OpenDocument> {
        self.shared.open_documents.lock().await.get(uri).cloned()
    }

    async fn open_document_uris(&self) -> Vec<Url> {
        self.shared
            .open_documents
            .lock()
            .await
            .keys()
            .cloned()
            .collect()
    }

    async fn configure_repo_from_initialize(&self, params: &InitializeParams) {
        let mut config = self.shared.config.lock().await;
        if config.repo_path.is_some() {
            return;
        }

        if let Some(root_uri) = &params.root_uri
            && let Ok(path) = root_uri.to_file_path()
        {
            config.repo_path = Some(path);
            return;
        }

        if let Some(workspaces) = &params.workspace_folders
            && let Some(folder) = workspaces.first()
            && let Ok(path) = folder.uri.to_file_path()
        {
            config.repo_path = Some(path);
        }
    }

    async fn upstream_session(&self) -> Option<Arc<UpstreamSession>> {
        self.shared.upstream.lock().await.clone()
    }

    async fn initialize_upstream(&self, params: &InitializeParams) -> Option<InitializeResult> {
        let config = self.shared.config.lock().await.clone();
        let command = config.upstream_command.clone()?;

        let repo_path = config.repo_path.clone();
        let (diagnostics_tx, mut diagnostics_rx) = mpsc::unbounded_channel();
        let session = match UpstreamSession::spawn(
            self.client.clone(),
            repo_path.as_deref(),
            &command,
            &config.upstream_args,
            diagnostics_tx,
        ) {
            Ok(session) => session,
            Err(err) => {
                self.client
                    .log_message(
                        MessageType::WARNING,
                        format!("failed to start upstream language server `{command}`: {err}"),
                    )
                    .await;
                return None;
            }
        };

        {
            let mut upstream = self.shared.upstream.lock().await;
            *upstream = Some(session.clone());
        }

        let backend = self.clone();
        tokio::spawn(async move {
            while let Some(params) = diagnostics_rx.recv().await {
                let _ = backend.handle_upstream_diagnostics(params).await;
            }
        });

        match session
            .request("initialize", params)
            .await
            .and_then(decode_upstream_response::<InitializeResult>)
        {
            Ok(result) => Some(result),
            Err(err) => {
                self.client
                    .log_message(
                        MessageType::WARNING,
                        format!("upstream initialize failed; continuing in local-only mode: {err}"),
                    )
                    .await;
                let mut upstream = self.shared.upstream.lock().await;
                upstream.take();
                None
            }
        }
    }

    async fn maybe_start_event_watcher(&self) {
        if self.shared.watcher_started.swap(true, Ordering::SeqCst) {
            return;
        }

        let backend = self.clone();
        tokio::spawn(async move {
            backend.watch_event_log().await;
        });
    }

    async fn watch_event_log(self) {
        loop {
            let poll_ms = self.shared.config.lock().await.event_poll_ms;
            let _ = self.publish_new_events().await;
            tokio::time::sleep(std::time::Duration::from_millis(poll_ms)).await;
        }
    }

    async fn publish_new_events(&self) -> CodexResult<()> {
        let payloads = self.collect_new_event_stream_params().await?;
        let published = payloads.len();
        for payload in payloads {
            self.client
                .send_notification::<CodexEventStream>(payload)
                .await;
        }

        if published > 0 {
            let _ = self.client.code_lens_refresh().await;
            for uri in self.open_document_uris().await {
                let _ = self.publish_lock_annotations(&uri).await;
            }
        }

        Ok(())
    }

    async fn refresh_document(&self, uri: &Url) -> CodexResult<()> {
        self.publish_lock_annotations(uri).await?;
        self.publish_validation(uri).await?;
        let _ = self.client.code_lens_refresh().await;
        Ok(())
    }

    async fn publish_lock_annotations(&self, uri: &Url) -> CodexResult<()> {
        let locks = self.active_lock_annotations(uri).await?;
        self.client
            .send_notification::<CodexActiveLocks>(ActiveLocksParams {
                uri: uri.clone(),
                locks: locks.clone(),
            })
            .await;

        let intents = locks
            .iter()
            .map(|lock| AgentIntentAnnotation {
                agent_name: lock.agent_name.clone(),
                target_name: lock.unit_name.clone(),
                message: format!(
                    "Agent {} is editing {} [{}]",
                    lock.agent_name, lock.unit_name, lock.lock_kind
                ),
            })
            .collect();

        self.client
            .send_notification::<CodexAgentIntent>(AgentIntentParams {
                uri: uri.clone(),
                intents,
            })
            .await;

        Ok(())
    }

    async fn publish_validation(&self, uri: &Url) -> CodexResult<()> {
        let findings = self.validation_findings(uri).await?;
        let version = self.open_document(uri).await.map(|doc| doc.version);
        let diagnostics: Vec<_> = findings.iter().map(diagnostic_from_finding).collect();
        let issues: Vec<_> = findings
            .iter()
            .map(|finding| ValidationIssueAnnotation {
                severity: finding.severity,
                message: finding.message.clone(),
                range: finding.range,
            })
            .collect();

        self.store_local_diagnostics(uri.clone(), diagnostics).await;
        self.publish_merged_diagnostics(uri.clone(), version).await;
        self.client
            .send_notification::<CodexValidationStatus>(ValidationStatusParams {
                uri: uri.clone(),
                issues,
            })
            .await;
        Ok(())
    }

    async fn handle_upstream_diagnostics(
        &self,
        params: PublishDiagnosticsParams,
    ) -> CodexResult<()> {
        let version = match params.version {
            Some(version) => Some(version),
            None => self
                .open_document(&params.uri)
                .await
                .map(|document| document.version),
        };
        self.store_upstream_diagnostics(params.uri.clone(), params.diagnostics)
            .await;
        self.publish_merged_diagnostics(params.uri, version).await;
        Ok(())
    }

    async fn publish_merged_diagnostics(&self, uri: Url, version: Option<i32>) {
        let local = self
            .shared
            .local_diagnostics
            .lock()
            .await
            .get(&uri)
            .cloned()
            .unwrap_or_default();
        let upstream = self
            .shared
            .upstream_diagnostics
            .lock()
            .await
            .get(&uri)
            .cloned()
            .unwrap_or_default();

        let mut merged = local;
        merged.extend(upstream);
        self.client.publish_diagnostics(uri, merged, version).await;
    }

    async fn store_local_diagnostics(&self, uri: Url, diagnostics: Vec<Diagnostic>) {
        let mut all = self.shared.local_diagnostics.lock().await;
        if diagnostics.is_empty() {
            all.remove(&uri);
        } else {
            all.insert(uri, diagnostics);
        }
    }

    async fn store_upstream_diagnostics(&self, uri: Url, diagnostics: Vec<Diagnostic>) {
        let mut all = self.shared.upstream_diagnostics.lock().await;
        if diagnostics.is_empty() {
            all.remove(&uri);
        } else {
            all.insert(uri, diagnostics);
        }
    }

    async fn active_lock_annotations(&self, uri: &Url) -> CodexResult<Vec<ActiveLockAnnotation>> {
        let Some(repo_path) = self.repo_path().await else {
            return Ok(Vec::new());
        };

        let text = read_document_text(self, uri).await?;
        let units = parse_document_units(uri, &text)?;
        let locks = load_lock_entries(&repo_path)?;

        let mut annotations = Vec::new();
        for lock in locks {
            if let Some(unit) = units.iter().find(|unit| matches_lock_target(unit, &lock)) {
                annotations.push(ActiveLockAnnotation {
                    unit_name: unit.qualified_name.clone(),
                    agent_name: lock.agent_name,
                    lock_kind: format!("{:?}", lock.kind),
                    range: range_from_bytes(&text, &unit.byte_range),
                });
            }
        }

        annotations.sort_by(|left, right| {
            left.range
                .start
                .line
                .cmp(&right.range.start.line)
                .then_with(|| left.agent_name.cmp(&right.agent_name))
        });

        Ok(annotations)
    }

    async fn validation_findings(&self, uri: &Url) -> CodexResult<Vec<ValidationFinding>> {
        let Some(repo_path) = self.repo_path().await else {
            return Ok(Vec::new());
        };

        let config = self.shared.config.lock().await.clone();
        let relative = relative_repo_path(&repo_path, uri)?;
        let before = build_graph_at_ref(&repo_path, &config.base_ref)?;
        let after = build_graph_from_worktree(&repo_path)?;
        let locks = load_lock_entries(&repo_path)?;

        Ok(validation_findings_for_path(
            &repo_path, &relative, &before, &after, &locks,
        ))
    }

    async fn clear_diagnostics(&self, uri: &Url) {
        self.shared.local_diagnostics.lock().await.remove(uri);
        self.shared.upstream_diagnostics.lock().await.remove(uri);
        self.client
            .publish_diagnostics(uri.clone(), Vec::new(), None)
            .await;
    }

    async fn forward_notification<P>(&self, method: &str, params: &P)
    where
        P: Serialize,
    {
        let Some(session) = self.upstream_session().await else {
            return;
        };
        let _ = session.notify(method, params);
    }

    async fn forward_request<P, R>(&self, method: &str, params: &P) -> Result<Option<R>>
    where
        P: Serialize,
        R: DeserializeOwned,
    {
        let Some(session) = self.upstream_session().await else {
            return Ok(None);
        };

        let value = session
            .request(method, params)
            .await
            .map_err(upstream_jsonrpc_error)?;
        decode_upstream_response(value)
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for CodexLspBackend {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        self.configure_repo_from_initialize(&params).await;
        let upstream = self.initialize_upstream(&params).await;
        let capabilities =
            merged_capabilities(upstream.as_ref().map(|result| &result.capabilities));
        let upstream_name = upstream
            .as_ref()
            .and_then(|result| result.server_info.as_ref())
            .map(|info| info.name.as_str());

        Ok(InitializeResult {
            server_info: Some(ServerInfo {
                name: match upstream_name {
                    Some(name) => format!("nex-lsp + {name}"),
                    None => "nex-lsp".to_string(),
                },
                version: Some(env!("CARGO_PKG_VERSION").to_string()),
            }),
            capabilities,
        })
    }

    async fn initialized(&self, params: InitializedParams) {
        self.client
            .log_message(MessageType::INFO, "nex-lsp initialized")
            .await;
        self.maybe_start_event_watcher().await;
        let _ = self.publish_new_events().await;
        self.forward_notification("initialized", &params).await;
    }

    async fn shutdown(&self) -> Result<()> {
        if let Some(session) = self.upstream_session().await {
            let _ = session.request("shutdown", &serde_json::Value::Null).await;
        }
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let document = params.text_document.clone();
        self.store_open_document(document.uri.clone(), document.version, document.text)
            .await;
        let _ = self.publish_lock_annotations(&document.uri).await;
        self.forward_notification("textDocument/didOpen", &params)
            .await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        if let Some(change) = params.content_changes.last() {
            self.update_open_document(
                &params.text_document.uri,
                params.text_document.version,
                change.text.clone(),
            )
            .await;
        }

        let text = self
            .open_document(&params.text_document.uri)
            .await
            .map(|document| document.text)
            .unwrap_or_default();
        self.forward_notification(
            "textDocument/didChange",
            &DidChangeTextDocumentParams {
                text_document: params.text_document.clone(),
                content_changes: vec![TextDocumentContentChangeEvent {
                    range: None,
                    range_length: None,
                    text,
                }],
            },
        )
        .await;
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        if let Some(text) = params.text {
            let version = self
                .open_document(&params.text_document.uri)
                .await
                .map(|doc| doc.version)
                .unwrap_or(0);
            self.update_open_document(&params.text_document.uri, version, text)
                .await;
        }

        let _ = self.refresh_document(&params.text_document.uri).await;
        self.forward_notification(
            "textDocument/didSave",
            &DidSaveTextDocumentParams {
                text_document: params.text_document,
                text: None,
            },
        )
        .await;
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        self.forward_notification("textDocument/didClose", &params)
            .await;
        self.remove_open_document(&params.text_document.uri).await;
        self.clear_diagnostics(&params.text_document.uri).await;
    }

    async fn code_lens(&self, params: CodeLensParams) -> Result<Option<Vec<CodeLens>>> {
        let annotations = self
            .active_lock_annotations(&params.text_document.uri)
            .await
            .map_err(to_jsonrpc_error)?;
        let lenses = annotations
            .into_iter()
            .map(|annotation| CodeLens {
                range: annotation.range,
                command: Some(Command {
                    title: format!("Agent {} is editing this function", annotation.agent_name),
                    command: "nex.nop".to_string(),
                    arguments: None,
                }),
                data: None,
            })
            .collect();
        Ok(Some(lenses))
    }

    async fn execute_command(&self, _: ExecuteCommandParams) -> Result<Option<serde_json::Value>> {
        Ok(None)
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        self.forward_request("textDocument/completion", &params)
            .await
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        self.forward_request("textDocument/hover", &params).await
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        self.forward_request("textDocument/definition", &params)
            .await
    }
}

/// Build a fully configured `tower-lsp` service for the Codex backend.
pub fn build_service(config: CodexLspConfig) -> (LspService<CodexLspBackend>, ClientSocket) {
    LspService::build(|client| CodexLspBackend::new(client, config.clone()))
        .custom_method("nex/semanticDiff", CodexLspBackend::semantic_diff)
        .finish()
}

fn event_stream_params(event: &SemanticEvent) -> EventStreamParams {
    EventStreamParams {
        event_id: event.id.to_string(),
        description: event.description.clone(),
        agent_id: event.agent_id.clone(),
        tags: event.tags.clone(),
        timestamp: event.timestamp.to_rfc3339(),
    }
}

fn to_jsonrpc_error(err: CodexError) -> Error {
    Error::invalid_params(err.to_string())
}

fn diagnostic_from_finding(finding: &ValidationFinding) -> Diagnostic {
    Diagnostic {
        range: finding.range,
        severity: Some(finding.severity),
        message: finding.message.clone(),
        source: Some("nex".to_string()),
        ..Diagnostic::default()
    }
}

fn merged_capabilities(upstream: Option<&ServerCapabilities>) -> ServerCapabilities {
    let mut capabilities = upstream.cloned().unwrap_or_default();
    capabilities.text_document_sync =
        Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL));
    capabilities.code_lens_provider = Some(CodeLensOptions {
        resolve_provider: Some(false),
    });
    capabilities.execute_command_provider = Some(merge_execute_command_provider(
        capabilities.execute_command_provider.take(),
    ));
    capabilities
}

fn merge_execute_command_provider(
    provider: Option<ExecuteCommandOptions>,
) -> ExecuteCommandOptions {
    let mut commands = provider
        .as_ref()
        .map(|options| options.commands.clone())
        .unwrap_or_default();
    if !commands.iter().any(|command| command == "nex.nop") {
        commands.push("nex.nop".to_string());
    }

    ExecuteCommandOptions {
        commands,
        work_done_progress_options: provider
            .map(|options| options.work_done_progress_options)
            .unwrap_or(WorkDoneProgressOptions {
                work_done_progress: None,
            }),
    }
}

fn decode_upstream_response<T>(value: serde_json::Value) -> std::result::Result<T, Error>
where
    T: DeserializeOwned,
{
    serde_json::from_value(value).map_err(upstream_jsonrpc_error)
}

fn upstream_jsonrpc_error(error: impl std::error::Error) -> Error {
    Error {
        code: ErrorCode::InternalError,
        message: error.to_string().into(),
        data: None,
    }
}

async fn read_document_text(backend: &CodexLspBackend, uri: &Url) -> CodexResult<String> {
    if let Some(document) = backend.open_document(uri).await {
        return Ok(document.text);
    }

    let path = uri
        .to_file_path()
        .map_err(|_| CodexError::Coordination(format!("unsupported document URI: {uri}")))?;
    Ok(std::fs::read_to_string(path)?)
}

fn parse_document_units(uri: &Url, text: &str) -> CodexResult<Vec<SemanticUnit>> {
    let path = uri
        .to_file_path()
        .map_err(|_| CodexError::Coordination(format!("unsupported document URI: {uri}")))?;
    let Some(extractor) = nex_parse::extractor_for_path(&path) else {
        return Ok(Vec::new());
    };
    extractor.extract(&path, text.as_bytes())
}

fn matches_lock_target(unit: &SemanticUnit, lock: &PersistedLockEntry) -> bool {
    unit.qualified_name == lock.target_name || unit.name == lock.target_name
}

fn range_from_bytes(text: &str, byte_range: &std::ops::Range<usize>) -> Range {
    Range {
        start: offset_to_position(text, byte_range.start),
        end: offset_to_position(text, byte_range.end),
    }
}

fn offset_to_position(text: &str, offset: usize) -> Position {
    let mut line = 0u32;
    let mut character = 0u32;

    for (index, byte) in text.bytes().enumerate() {
        if index >= offset {
            break;
        }
        if byte == b'\n' {
            line += 1;
            character = 0;
        } else {
            character += 1;
        }
    }

    Position { line, character }
}

fn relative_repo_path(repo_path: &Path, uri: &Url) -> CodexResult<PathBuf> {
    let path = uri
        .to_file_path()
        .map_err(|_| CodexError::Coordination(format!("unsupported document URI: {uri}")))?;
    path.strip_prefix(repo_path)
        .map(|path| path.to_path_buf())
        .map_err(|_| CodexError::Coordination(format!("document outside repo root: {uri}")))
}

fn collect_files_at_ref(
    repo: &git2::Repository,
    refspec: &str,
) -> CodexResult<Vec<(String, Vec<u8>)>> {
    let commit = repo
        .revparse_single(refspec)
        .and_then(|object| object.peel_to_commit())
        .map_err(|err| CodexError::Git(err.to_string()))?;
    let tree = commit
        .tree()
        .map_err(|err| CodexError::Git(err.to_string()))?;

    let mut files = Vec::new();
    let mut walk_error: Option<CodexError> = None;

    tree.walk(TreeWalkMode::PreOrder, |root, entry| {
        if entry.kind() != Some(ObjectType::Blob) {
            return TreeWalkResult::Ok;
        }

        let Some(name) = entry.name() else {
            return TreeWalkResult::Ok;
        };
        let full_path = format!("{root}{name}");
        let is_supported = Path::new(&full_path)
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(nex_parse::supports_extension);
        if !is_supported {
            return TreeWalkResult::Ok;
        }

        match repo.find_blob(entry.id()) {
            Ok(blob) => {
                files.push((full_path, blob.content().to_vec()));
                TreeWalkResult::Ok
            }
            Err(err) => {
                walk_error = Some(CodexError::Git(err.to_string()));
                TreeWalkResult::Abort
            }
        }
    })
    .map_err(|err| {
        walk_error
            .take()
            .unwrap_or_else(|| CodexError::Git(err.to_string()))
    })?;

    if let Some(err) = walk_error {
        return Err(err);
    }

    Ok(files)
}

fn collect_files_in_worktree(repo_path: &Path) -> CodexResult<Vec<(String, Vec<u8>)>> {
    let mut files = Vec::new();
    collect_files_recursive(repo_path, repo_path, &mut files)?;
    Ok(files)
}

fn collect_files_recursive(
    repo_root: &Path,
    dir: &Path,
    files: &mut Vec<(String, Vec<u8>)>,
) -> CodexResult<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let file_name = entry.file_name();
        let name = file_name.to_string_lossy();

        if path.is_dir() {
            if matches!(name.as_ref(), ".git" | ".nex" | "target") {
                continue;
            }
            collect_files_recursive(repo_root, &path, files)?;
            continue;
        }

        let is_supported = path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(nex_parse::supports_extension);
        if !is_supported {
            continue;
        }

        let relative = path
            .strip_prefix(repo_root)
            .map_err(|_| {
                CodexError::Coordination(format!("failed to relativize {}", path.display()))
            })?
            .to_string_lossy()
            .replace('\\', "/");
        files.push((relative, std::fs::read(&path)?));
    }

    Ok(())
}

fn build_graph_from_files(files: &[(String, Vec<u8>)]) -> CodexResult<CodeGraph> {
    let extractors: Vec<Box<dyn nex_parse::SemanticExtractor>> = nex_parse::default_extractors();
    let mut all_units = Vec::new();
    let mut file_contexts: Vec<(usize, &[u8])> = Vec::new();

    for (path, content) in files {
        let Some(ext) = Path::new(path).extension().and_then(|ext| ext.to_str()) else {
            continue;
        };
        let Some((extractor_index, extractor)) = extractors
            .iter()
            .enumerate()
            .find(|(_, extractor)| extractor.extensions().contains(&ext))
        else {
            continue;
        };

        all_units.extend(extractor.extract(Path::new(path), content)?);
        file_contexts.push((extractor_index, content.as_slice()));
    }

    let mut deps = Vec::new();
    for (extractor_index, content) in file_contexts {
        deps.extend(extractors[extractor_index].dependencies(&all_units, content)?);
    }
    let mut graph = CodeGraph::new();
    for unit in all_units {
        graph.add_unit(unit);
    }
    for (from, to, kind) in deps {
        graph.add_dep(from, to, kind);
    }
    Ok(graph)
}

fn build_graph_at_ref(repo_path: &Path, refspec: &str) -> CodexResult<CodeGraph> {
    let repo = git2::Repository::open(repo_path).map_err(|err| CodexError::Git(err.to_string()))?;
    let files = collect_files_at_ref(&repo, refspec)?;
    build_graph_from_files(&files)
}

fn build_graph_from_worktree(repo_path: &Path) -> CodexResult<CodeGraph> {
    let files = collect_files_in_worktree(repo_path)?;
    build_graph_from_files(&files)
}

fn load_lock_entries(repo_path: &Path) -> CodexResult<Vec<PersistedLockEntry>> {
    let path = repo_path.join(".nex").join("locks.json");
    if !path.exists() {
        return Ok(Vec::new());
    }
    let content = std::fs::read_to_string(path)?;
    Ok(serde_json::from_str(&content)?)
}

fn compute_diff_between_refs(
    repo_path: &Path,
    base_ref: &str,
    head_ref: &str,
) -> CodexResult<SemanticDiff> {
    let before = build_graph_at_ref(repo_path, base_ref)?;
    let after = build_graph_at_ref(repo_path, head_ref)?;
    Ok(before.diff(&after))
}

fn filter_diff_to_path(diff: &SemanticDiff, path: &Path) -> SemanticDiff {
    SemanticDiff {
        added: diff
            .added
            .iter()
            .filter(|unit| unit.file_path == path)
            .cloned()
            .collect(),
        removed: diff
            .removed
            .iter()
            .filter(|unit| unit.file_path == path)
            .cloned()
            .collect(),
        modified: diff
            .modified
            .iter()
            .filter(|unit| unit.after.file_path == path || unit.before.file_path == path)
            .cloned()
            .collect(),
        moved: diff
            .moved
            .iter()
            .filter(|moved| moved.new_path == path || moved.old_path == path)
            .cloned()
            .collect(),
    }
}

fn validation_findings_for_path(
    repo_path: &Path,
    relative_path: &Path,
    before: &CodeGraph,
    after: &CodeGraph,
    locks: &[PersistedLockEntry],
) -> Vec<ValidationFinding> {
    let diff = before.diff(after);
    let mut findings = Vec::new();

    for modified in &diff.modified {
        if modified.after.file_path != relative_path {
            continue;
        }
        let has_lock = locks.iter().any(|lock| {
            lock.target == modified.before.id && matches!(lock.kind, IntentKind::Write)
        });
        if !has_lock {
            findings.push(ValidationFinding {
                severity: DiagnosticSeverity::ERROR,
                message: format!(
                    "modified `{}` without a Write lock",
                    modified.after.qualified_name
                ),
                range: unit_range_in_worktree(repo_path, &modified.after),
            });
        }
    }

    for removed in &diff.removed {
        if removed.file_path != relative_path {
            continue;
        }
        let has_lock = locks
            .iter()
            .any(|lock| lock.target == removed.id && matches!(lock.kind, IntentKind::Delete));
        if !has_lock {
            findings.push(ValidationFinding {
                severity: DiagnosticSeverity::ERROR,
                message: format!("deleted `{}` without a Delete lock", removed.qualified_name),
                range: Range::default(),
            });
        }
    }

    let modified_names: HashSet<&str> = diff
        .modified
        .iter()
        .map(|modified| modified.after.qualified_name.as_str())
        .collect();

    for removed in &diff.removed {
        for caller in before.callers_of(&removed.id) {
            if modified_names.contains(caller.qualified_name.as_str()) {
                continue;
            }
            let Some(after_caller) = find_unit_by_name(after, &caller.qualified_name) else {
                continue;
            };
            if after_caller.file_path != relative_path {
                continue;
            }
            findings.push(ValidationFinding {
                severity: DiagnosticSeverity::ERROR,
                message: format!(
                    "`{}` still references deleted `{}`",
                    after_caller.qualified_name, removed.qualified_name
                ),
                range: unit_range_in_worktree(repo_path, after_caller),
            });
        }
    }

    for modified in &diff.modified {
        if !modified.changes.contains(&ChangeKind::SignatureChanged) {
            continue;
        }
        let Some(after_unit) = find_unit_by_name(after, &modified.after.qualified_name) else {
            continue;
        };
        for caller in after.callers_of(&after_unit.id) {
            if modified_names.contains(caller.qualified_name.as_str()) {
                continue;
            }
            if caller.file_path != relative_path {
                continue;
            }
            findings.push(ValidationFinding {
                severity: DiagnosticSeverity::WARNING,
                message: format!(
                    "`{}` may be using old signature of `{}`",
                    caller.qualified_name, after_unit.qualified_name
                ),
                range: unit_range_in_worktree(repo_path, caller),
            });
        }
    }

    findings
}

fn unit_range_in_worktree(repo_path: &Path, unit: &SemanticUnit) -> Range {
    let full_path = repo_path.join(&unit.file_path);
    let text = std::fs::read_to_string(full_path).unwrap_or_default();
    range_from_bytes(&text, &unit.byte_range)
}

fn find_unit_by_name<'a>(graph: &'a CodeGraph, qualified_name: &str) -> Option<&'a SemanticUnit> {
    graph
        .units()
        .into_iter()
        .find(|unit| unit.qualified_name == qualified_name)
}
