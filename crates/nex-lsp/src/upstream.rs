//! Upstream stdio LSP transport for proxying standard editor requests.

use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{
    Arc,
    atomic::{AtomicI64, Ordering},
};
use tokio::io::{
    AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader, BufWriter,
};
use tokio::process::Command;
use tokio::sync::{Mutex, mpsc, oneshot};
use tower::Service;
use tower_lsp::Client;
use tower_lsp::jsonrpc::{Error, ErrorCode, Id, Request, Response};
use tower_lsp::lsp_types::PublishDiagnosticsParams;

type PendingMap = Arc<Mutex<HashMap<Id, oneshot::Sender<std::result::Result<Value, Error>>>>>;

#[derive(Debug)]
enum OutboundMessage {
    Request(Request),
    Response(Response),
}

/// Running upstream stdio LSP session.
#[derive(Clone)]
pub struct UpstreamSession {
    outbound: mpsc::UnboundedSender<OutboundMessage>,
    pending: PendingMap,
    next_request_id: Arc<AtomicI64>,
}

impl UpstreamSession {
    /// Spawn a stdio upstream language server and start proxy tasks.
    pub fn spawn(
        client: Client,
        repo_path: Option<&Path>,
        command: &str,
        args: &[String],
        diagnostics_tx: mpsc::UnboundedSender<PublishDiagnosticsParams>,
    ) -> std::io::Result<Arc<Self>> {
        let mut child = Command::new(command);
        child
            .args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        if let Some(path) = repo_path {
            child.current_dir(path);
        }

        let mut child = child.spawn()?;
        let stdin = child.stdin.take().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::BrokenPipe, "upstream stdin unavailable")
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "upstream stdout unavailable",
            )
        })?;

        let (outbound_tx, outbound_rx) = mpsc::unbounded_channel();
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let session = Arc::new(Self {
            outbound: outbound_tx.clone(),
            pending: pending.clone(),
            next_request_id: Arc::new(AtomicI64::new(1)),
        });

        tokio::spawn(writer_task(BufWriter::new(stdin), outbound_rx));
        tokio::spawn(reader_task(
            client,
            BufReader::new(stdout),
            outbound_tx,
            pending,
            diagnostics_tx,
        ));
        tokio::spawn(async move {
            let _ = child.wait().await;
        });

        Ok(session)
    }

    /// Send a notification to the upstream server.
    pub fn notify<P>(&self, method: &str, params: &P) -> std::result::Result<(), Error>
    where
        P: Serialize,
    {
        let value = serde_json::to_value(params).map_err(internal_error)?;
        let request = Request::build(method.to_string()).params(value).finish();
        self.outbound
            .send(OutboundMessage::Request(request))
            .map_err(|_| Error::internal_error())
    }

    /// Send a request to the upstream server and await the raw JSON result.
    pub async fn request<P>(&self, method: &str, params: &P) -> std::result::Result<Value, Error>
    where
        P: Serialize,
    {
        let id = Id::Number(self.next_request_id.fetch_add(1, Ordering::SeqCst));
        let value = serde_json::to_value(params).map_err(internal_error)?;
        let request = Request::build(method.to_string())
            .id(id.clone())
            .params(value)
            .finish();

        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id.clone(), tx);
        if self
            .outbound
            .send(OutboundMessage::Request(request))
            .is_err()
        {
            self.pending.lock().await.remove(&id);
            return Err(Error::internal_error());
        }

        match rx.await {
            Ok(result) => result,
            Err(_) => Err(Error::internal_error()),
        }
    }
}

async fn writer_task<W>(mut writer: BufWriter<W>, mut rx: mpsc::UnboundedReceiver<OutboundMessage>)
where
    W: AsyncWrite + Unpin,
{
    while let Some(message) = rx.recv().await {
        let payload = match message {
            OutboundMessage::Request(request) => match serde_json::to_vec(&request) {
                Ok(payload) => payload,
                Err(_) => continue,
            },
            OutboundMessage::Response(response) => match serde_json::to_vec(&response) {
                Ok(payload) => payload,
                Err(_) => continue,
            },
        };

        if write_message(&mut writer, &payload).await.is_err() {
            break;
        }
    }
}

async fn reader_task<R>(
    client: Client,
    mut reader: BufReader<R>,
    outbound: mpsc::UnboundedSender<OutboundMessage>,
    pending: PendingMap,
    diagnostics_tx: mpsc::UnboundedSender<PublishDiagnosticsParams>,
) where
    R: AsyncRead + Unpin,
{
    loop {
        let Some(message) = read_message(&mut reader).await.ok().flatten() else {
            fail_pending(&pending).await;
            break;
        };

        if let Ok(response) = serde_json::from_slice::<Response>(&message) {
            let (id, body) = response.into_parts();
            if let Some(tx) = pending.lock().await.remove(&id) {
                let _ = tx.send(body);
            }
            continue;
        }

        let request = match serde_json::from_slice::<Request>(&message) {
            Ok(request) => request,
            Err(_) => {
                fail_pending(&pending).await;
                break;
            }
        };

        if request.method() == "textDocument/publishDiagnostics" {
            if let Some(params) = request.params()
                && let Ok(params) =
                    serde_json::from_value::<PublishDiagnosticsParams>(params.clone())
            {
                let _ = diagnostics_tx.send(params);
            }
            continue;
        }

        let (method, id, params) = request.into_parts();
        if let Some(id) = id {
            let forwarded_id = format!("nex-upstream-{}", request_key_fragment(&id));
            let mut forwarded = Request::build(method.clone()).id(forwarded_id);
            if let Some(params) = params.clone() {
                forwarded = forwarded.params(params);
            }

            let response = match client.clone().call(forwarded.finish()).await {
                Ok(Some(response)) => {
                    let (_, body) = response.into_parts();
                    Response::from_parts(id, body)
                }
                Ok(None) => Response::from_error(id, Error::internal_error()),
                Err(_) => Response::from_error(
                    id,
                    Error {
                        code: ErrorCode::InternalError,
                        message: "editor request forwarding failed".into(),
                        data: None,
                    },
                ),
            };
            let _ = outbound.send(OutboundMessage::Response(response));
            continue;
        }

        let mut forwarded = Request::build(method);
        if let Some(params) = params {
            forwarded = forwarded.params(params);
        }
        let _ = client.clone().call(forwarded.finish()).await;
    }
}

async fn fail_pending(pending: &PendingMap) {
    let mut pending = pending.lock().await;
    for (_, tx) in pending.drain() {
        let _ = tx.send(Err(Error::internal_error()));
    }
}

async fn write_message<W>(writer: &mut BufWriter<W>, payload: &[u8]) -> std::io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    writer
        .write_all(format!("Content-Length: {}\r\n\r\n", payload.len()).as_bytes())
        .await?;
    writer.write_all(payload).await?;
    writer.flush().await
}

async fn read_message<R>(reader: &mut BufReader<R>) -> std::io::Result<Option<Vec<u8>>>
where
    R: AsyncRead + Unpin,
{
    let mut content_length = None;

    loop {
        let mut line = String::new();
        let read = reader.read_line(&mut line).await?;
        if read == 0 {
            return Ok(None);
        }

        if line == "\r\n" {
            break;
        }

        if let Some(value) = line.strip_prefix("Content-Length:") {
            let length = value.trim().parse::<usize>().map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid content length")
            })?;
            content_length = Some(length);
        }
    }

    let length = content_length.ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "missing content length")
    })?;
    let mut payload = vec![0; length];
    reader.read_exact(&mut payload).await?;
    Ok(Some(payload))
}

fn internal_error(error: impl std::error::Error) -> Error {
    Error {
        code: ErrorCode::InternalError,
        message: error.to_string().into(),
        data: None,
    }
}

fn request_key_fragment(id: &Id) -> String {
    match id {
        Id::Number(value) => value.to_string(),
        Id::String(value) => value.clone(),
        Id::Null => "null".to_string(),
    }
}
