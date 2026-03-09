use futures_util::{SinkExt, StreamExt};
use nex_core::SemanticDiff;
use nex_eventlog::SemanticEvent;
use nex_lsp::protocol::EventStreamParams;
use nex_lsp::{CodexLspConfig, build_service};
use serde_json::json;
use std::path::Path;
use tower::Service;
use tower::util::ServiceExt;
use tower_lsp::LanguageServer;
use tower_lsp::jsonrpc::{Request, Response};
use tower_lsp::lsp_types::{
    CodeLensParams, CompletionContext, CompletionParams, DidOpenTextDocumentParams,
    DidSaveTextDocumentParams, GotoDefinitionParams, HoverContents, HoverParams,
    PartialResultParams, Position, TextDocumentIdentifier, TextDocumentItem,
    TextDocumentPositionParams, Url, WorkDoneProgressParams,
};

fn init_temp_repo() -> (tempfile::TempDir, git2::Repository) {
    let dir = tempfile::tempdir().expect("create tempdir");
    let repo = git2::Repository::init(dir.path()).expect("init repo");

    let mut config = repo.config().expect("get config");
    config.set_str("user.name", "Test").expect("set name");
    config
        .set_str("user.email", "test@example.com")
        .expect("set email");

    (dir, repo)
}

fn write_and_stage(repo: &git2::Repository, relative_path: &str, content: &str) {
    let full_path = repo.workdir().unwrap().join(relative_path);
    if let Some(parent) = full_path.parent() {
        std::fs::create_dir_all(parent).expect("create dirs");
    }
    std::fs::write(&full_path, content).expect("write file");

    let mut index = repo.index().expect("get index");
    index
        .add_path(Path::new(relative_path))
        .expect("add to index");
    index.write().expect("write index");
}

fn commit(repo: &git2::Repository, msg: &str) {
    let mut index = repo.index().expect("get index");
    let tree_oid = index.write_tree().expect("write tree");
    let tree = repo.find_tree(tree_oid).expect("find tree");
    let sig = repo.signature().expect("sig");
    let parents: Vec<git2::Commit> = match repo.head() {
        Ok(head) => vec![head.peel_to_commit().expect("peel")],
        Err(_) => vec![],
    };
    let refs: Vec<&git2::Commit> = parents.iter().collect();
    repo.commit(Some("HEAD"), &sig, &sig, msg, &tree, &refs)
        .expect("commit");
}

async fn initialize_service(
    service: &mut tower_lsp::LspService<nex_lsp::CodexLspBackend>,
    root_uri: &Url,
) {
    let initialize = Request::build("initialize")
        .id(1)
        .params(json!({
            "capabilities": {},
            "rootUri": root_uri,
        }))
        .finish();
    let response = service
        .ready()
        .await
        .unwrap()
        .call(initialize)
        .await
        .unwrap();
    assert!(response.unwrap().is_ok());

    let initialized = Request::build("initialized").params(json!({})).finish();
    let _ = service
        .ready()
        .await
        .unwrap()
        .call(initialized)
        .await
        .unwrap();
}

fn test_config(repo: &git2::Repository) -> CodexLspConfig {
    CodexLspConfig {
        repo_path: Some(repo.workdir().unwrap().to_path_buf()),
        base_ref: "HEAD".to_string(),
        event_poll_ms: 20,
        upstream_command: None,
        upstream_args: Vec::new(),
    }
}

fn upstream_test_config(repo: &git2::Repository) -> CodexLspConfig {
    CodexLspConfig {
        upstream_command: Some(env!("CARGO_BIN_EXE_nex-lsp-fake-upstream").to_string()),
        ..test_config(repo)
    }
}

fn position_params(uri: &Url) -> TextDocumentPositionParams {
    TextDocumentPositionParams {
        text_document: TextDocumentIdentifier { uri: uri.clone() },
        position: Position {
            line: 0,
            character: 0,
        },
    }
}

fn spawn_client_socket_task(
    socket: tower_lsp::ClientSocket,
) -> tokio::sync::mpsc::UnboundedReceiver<Request> {
    let (mut stream, mut sink) = socket.split();
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        while let Some(request) = stream.next().await {
            let forwarded = request.clone();
            let _ = tx.send(forwarded);
            if let Some(id) = request.id().cloned() {
                sink.send(Response::from_ok(id, json!(null)))
                    .await
                    .expect("respond to client request");
            }
        }
    });
    rx
}

async fn wait_for_notification(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<Request>,
    method: &str,
) -> Request {
    let deadline = std::time::Duration::from_secs(3);
    tokio::time::timeout(deadline, async {
        loop {
            let request = rx.recv().await.expect("server notification");
            if request.method() == method {
                return request;
            }
        }
    })
    .await
    .expect("notification before timeout")
}

async fn wait_for_publish_diagnostics(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<Request>,
    expected_count: usize,
) -> serde_json::Value {
    let deadline = std::time::Duration::from_secs(3);
    tokio::time::timeout(deadline, async {
        loop {
            let request = rx.recv().await.expect("server notification");
            if request.method() != "textDocument/publishDiagnostics" {
                continue;
            }
            let params = request.params().cloned().expect("diagnostics params");
            let count = params["diagnostics"]
                .as_array()
                .map(|diagnostics| diagnostics.len())
                .unwrap_or_default();
            if count >= expected_count {
                return params;
            }
        }
    })
    .await
    .expect("diagnostics before timeout")
}

#[tokio::test(flavor = "current_thread")]
async fn semantic_diff_custom_method_returns_file_scoped_changes() {
    let (_dir, repo) = init_temp_repo();
    write_and_stage(
        &repo,
        "src/app.ts",
        "function alpha(): number { return 1; }",
    );
    commit(&repo, "v1");
    repo.tag_lightweight(
        "v1",
        &repo.head().unwrap().peel_to_commit().unwrap().into_object(),
        false,
    )
    .unwrap();

    write_and_stage(
        &repo,
        "src/app.ts",
        "function alpha(): number { return 2; }\nfunction beta(): number { return 3; }",
    );
    commit(&repo, "v2");
    repo.tag_lightweight(
        "v2",
        &repo.head().unwrap().peel_to_commit().unwrap().into_object(),
        false,
    )
    .unwrap();

    let app_uri = Url::from_file_path(repo.workdir().unwrap().join("src/app.ts")).unwrap();
    let config = test_config(&repo);
    let (mut service, _socket) = build_service(config);
    initialize_service(&mut service, &app_uri).await;

    let request = Request::build("nex/semanticDiff")
        .id(2)
        .params(json!({
            "baseRef": "v1",
            "headRef": "v2",
            "uri": app_uri,
        }))
        .finish();
    let response = service.ready().await.unwrap().call(request).await.unwrap();
    let (_, body) = response.unwrap().into_parts();
    let diff: SemanticDiff = serde_json::from_value(body.unwrap()).unwrap();

    assert_eq!(diff.added.len(), 1);
    assert_eq!(diff.modified.len(), 1);
    assert_eq!(diff.added[0].name, "beta");
    assert_eq!(diff.modified[0].after.name, "alpha");
}

#[tokio::test(flavor = "current_thread")]
async fn code_lens_surfaces_active_lock_annotations() {
    let (_dir, repo) = init_temp_repo();
    write_and_stage(
        &repo,
        "src/app.ts",
        "function validate(input: string): boolean { return input.length > 0; }",
    );
    commit(&repo, "initial");

    let nex_dir = repo.workdir().unwrap().join(".nex");
    std::fs::create_dir_all(&nex_dir).unwrap();
    std::fs::write(
        nex_dir.join("locks.json"),
        serde_json::to_string_pretty(&json!([{
            "agent_name": "alice",
            "agent_id": [0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],
            "target_name": "validate",
            "target": [0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],
            "kind": "Write"
        }]))
        .unwrap(),
    )
    .unwrap();

    let uri = Url::from_file_path(repo.workdir().unwrap().join("src/app.ts")).unwrap();
    let config = test_config(&repo);
    let (mut service, _socket) = build_service(config);
    initialize_service(&mut service, &uri).await;

    let lenses = service
        .inner()
        .code_lens(CodeLensParams {
            text_document: TextDocumentIdentifier { uri },
            work_done_progress_params: WorkDoneProgressParams {
                work_done_token: None,
            },
            partial_result_params: Default::default(),
        })
        .await
        .unwrap()
        .unwrap();

    assert_eq!(lenses.len(), 1);
    assert!(
        lenses[0]
            .command
            .as_ref()
            .unwrap()
            .title
            .contains("Agent alice is editing this function")
    );
}

#[tokio::test(flavor = "current_thread")]
async fn code_lens_surfaces_python_lock_annotations() {
    let (_dir, repo) = init_temp_repo();
    write_and_stage(
        &repo,
        "src/app.py",
        "def validate(input: str) -> bool:\n    return len(input) > 0\n",
    );
    commit(&repo, "initial");

    let nex_dir = repo.workdir().unwrap().join(".nex");
    std::fs::create_dir_all(&nex_dir).unwrap();
    std::fs::write(
        nex_dir.join("locks.json"),
        serde_json::to_string_pretty(&json!([{
            "agent_name": "alice",
            "agent_id": [0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],
            "target_name": "validate",
            "target": [0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],
            "kind": "Write"
        }]))
        .unwrap(),
    )
    .unwrap();

    let uri = Url::from_file_path(repo.workdir().unwrap().join("src/app.py")).unwrap();
    let config = test_config(&repo);
    let (mut service, _socket) = build_service(config);
    initialize_service(&mut service, &uri).await;

    let lenses = service
        .inner()
        .code_lens(CodeLensParams {
            text_document: TextDocumentIdentifier { uri },
            work_done_progress_params: WorkDoneProgressParams {
                work_done_token: None,
            },
            partial_result_params: Default::default(),
        })
        .await
        .unwrap()
        .unwrap();

    assert_eq!(lenses.len(), 1);
    assert!(
        lenses[0]
            .command
            .as_ref()
            .unwrap()
            .title
            .contains("Agent alice is editing this function")
    );
}

#[tokio::test(flavor = "current_thread")]
async fn code_lens_surfaces_rust_lock_annotations() {
    let (_dir, repo) = init_temp_repo();
    write_and_stage(
        &repo,
        "src/lib.rs",
        "fn validate(input: &str) -> bool {\n    !input.is_empty()\n}\n",
    );
    commit(&repo, "initial");

    let nex_dir = repo.workdir().unwrap().join(".nex");
    std::fs::create_dir_all(&nex_dir).unwrap();
    std::fs::write(
        nex_dir.join("locks.json"),
        serde_json::to_string_pretty(&json!([{
            "agent_name": "alice",
            "agent_id": [0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],
            "target_name": "validate",
            "target": [0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],
            "kind": "Write"
        }]))
        .unwrap(),
    )
    .unwrap();

    let uri = Url::from_file_path(repo.workdir().unwrap().join("src/lib.rs")).unwrap();
    let config = test_config(&repo);
    let (mut service, _socket) = build_service(config);
    initialize_service(&mut service, &uri).await;

    let lenses = service
        .inner()
        .code_lens(CodeLensParams {
            text_document: TextDocumentIdentifier { uri },
            work_done_progress_params: WorkDoneProgressParams {
                work_done_token: None,
            },
            partial_result_params: Default::default(),
        })
        .await
        .unwrap()
        .unwrap();

    assert_eq!(lenses.len(), 1);
    assert!(
        lenses[0]
            .command
            .as_ref()
            .unwrap()
            .title
            .contains("Agent alice is editing this function")
    );
}

#[tokio::test(flavor = "current_thread")]
async fn did_save_publishes_validation_status_for_unlocked_change() {
    let (_dir, repo) = init_temp_repo();
    write_and_stage(
        &repo,
        "src/app.ts",
        "function validate(input: string): boolean { return input.length > 0; }",
    );
    commit(&repo, "initial");

    std::fs::write(
        repo.workdir().unwrap().join("src/app.ts"),
        "function validate(input: string): boolean { return input.length > 1; }",
    )
    .unwrap();

    let uri = Url::from_file_path(repo.workdir().unwrap().join("src/app.ts")).unwrap();
    let config = test_config(&repo);
    let (mut service, _socket) = build_service(config);
    initialize_service(&mut service, &uri).await;

    let validation = service.inner().validation_status_for(&uri).await.unwrap();
    assert_eq!(validation.len(), 1);
    assert!(validation[0].message.contains("without a Write lock"));
}

#[tokio::test(flavor = "current_thread")]
async fn initialized_publishes_semantic_event_notifications() {
    let (_dir, repo) = init_temp_repo();
    write_and_stage(
        &repo,
        "src/app.ts",
        "function alpha(): number { return 1; }",
    );
    commit(&repo, "initial");

    let nex_dir = repo.workdir().unwrap().join(".nex");
    std::fs::create_dir_all(&nex_dir).unwrap();
    let event = SemanticEvent::new(uuid::Uuid::new_v4(), "alice", "commit alpha", Vec::new());
    std::fs::write(
        nex_dir.join("events.json"),
        serde_json::to_string_pretty(&vec![event.clone()]).unwrap(),
    )
    .unwrap();

    let config = test_config(&repo);
    let (service, _socket) = build_service(config);

    let payloads = service
        .inner()
        .collect_new_event_stream_params()
        .await
        .unwrap();
    assert_eq!(payloads.len(), 1);
    let params: EventStreamParams = payloads[0].clone();
    assert_eq!(params.event_id, event.id.to_string());
    assert_eq!(params.agent_id, "alice");
    assert_eq!(params.description, "commit alpha");
}

#[tokio::test(flavor = "current_thread")]
async fn completion_hover_and_definition_are_proxied_to_upstream() {
    let (_dir, repo) = init_temp_repo();
    write_and_stage(
        &repo,
        "src/app.ts",
        "function alpha(): number { return 1; }\nalpha();",
    );
    commit(&repo, "initial");

    let uri = Url::from_file_path(repo.workdir().unwrap().join("src/app.ts")).unwrap();
    let config = upstream_test_config(&repo);
    let (mut service, _socket) = build_service(config);
    initialize_service(&mut service, &uri).await;

    let completion = service
        .inner()
        .completion(CompletionParams {
            text_document_position: position_params(&uri),
            work_done_progress_params: WorkDoneProgressParams {
                work_done_token: None,
            },
            partial_result_params: PartialResultParams {
                partial_result_token: None,
            },
            context: Some(CompletionContext {
                trigger_kind: tower_lsp::lsp_types::CompletionTriggerKind::INVOKED,
                trigger_character: None,
            }),
        })
        .await
        .unwrap()
        .unwrap();
    let items = match completion {
        tower_lsp::lsp_types::CompletionResponse::Array(items) => items,
        tower_lsp::lsp_types::CompletionResponse::List(list) => list.items,
    };
    assert_eq!(items[0].label, "upstream-item");

    let hover = service
        .inner()
        .hover(HoverParams {
            text_document_position_params: position_params(&uri),
            work_done_progress_params: WorkDoneProgressParams {
                work_done_token: None,
            },
        })
        .await
        .unwrap()
        .unwrap();
    match hover.contents {
        HoverContents::Markup(markup) => assert_eq!(markup.value, "upstream hover"),
        HoverContents::Scalar(tower_lsp::lsp_types::MarkedString::String(value)) => {
            assert_eq!(value, "upstream hover")
        }
        _ => panic!("unexpected hover payload"),
    }

    let definition = service
        .inner()
        .goto_definition(GotoDefinitionParams {
            text_document_position_params: position_params(&uri),
            work_done_progress_params: WorkDoneProgressParams {
                work_done_token: None,
            },
            partial_result_params: PartialResultParams {
                partial_result_token: None,
            },
        })
        .await
        .unwrap()
        .unwrap();
    match definition {
        tower_lsp::lsp_types::GotoDefinitionResponse::Scalar(location) => {
            assert_eq!(location.uri, uri);
            assert_eq!(location.range.start.line, 0);
        }
        _ => panic!("unexpected definition payload"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn did_save_merges_local_and_upstream_diagnostics() {
    let (_dir, repo) = init_temp_repo();
    write_and_stage(
        &repo,
        "src/app.ts",
        "function validate(input: string): boolean { return input.length > 0; }",
    );
    commit(&repo, "initial");

    let updated = "function validate(input: string): boolean { return input.length > 1; }";
    std::fs::write(repo.workdir().unwrap().join("src/app.ts"), updated).unwrap();

    let uri = Url::from_file_path(repo.workdir().unwrap().join("src/app.ts")).unwrap();
    let config = upstream_test_config(&repo);
    let (mut service, socket) = build_service(config);
    let mut notifications = spawn_client_socket_task(socket);
    initialize_service(&mut service, &uri).await;
    let _ = wait_for_notification(&mut notifications, "window/logMessage").await;

    service
        .inner()
        .did_open(DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri: uri.clone(),
                language_id: "typescript".to_string(),
                version: 1,
                text: updated.to_string(),
            },
        })
        .await;

    service
        .inner()
        .did_save(DidSaveTextDocumentParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            text: Some(updated.to_string()),
        })
        .await;

    let diagnostics = wait_for_publish_diagnostics(&mut notifications, 2).await;
    let messages: Vec<_> = diagnostics["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|diagnostic| diagnostic["message"].as_str().unwrap().to_string())
        .collect();
    assert!(
        messages
            .iter()
            .any(|message| message.contains("without a Write lock"))
    );
    assert!(
        messages
            .iter()
            .any(|message| message == "upstream diagnostic")
    );
}

#[tokio::test(flavor = "current_thread")]
async fn graceful_degradation_without_nex_state_returns_empty_annotations() {
    let (_dir, repo) = init_temp_repo();
    write_and_stage(
        &repo,
        "src/app.ts",
        "function alpha(): number { return 1; }",
    );
    commit(&repo, "initial");

    let uri = Url::from_file_path(repo.workdir().unwrap().join("src/app.ts")).unwrap();
    let config = test_config(&repo);
    let (mut service, _socket) = build_service(config);
    initialize_service(&mut service, &uri).await;

    let lenses = service
        .inner()
        .code_lens(CodeLensParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            work_done_progress_params: WorkDoneProgressParams {
                work_done_token: None,
            },
            partial_result_params: Default::default(),
        })
        .await
        .unwrap()
        .unwrap();
    assert!(lenses.is_empty());

    let active_locks = service
        .inner()
        .active_lock_annotations_for(&uri)
        .await
        .unwrap();
    assert!(active_locks.is_empty());

    let validation = service.inner().validation_status_for(&uri).await.unwrap();
    assert!(validation.is_empty());
}
