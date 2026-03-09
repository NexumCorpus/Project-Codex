use serde_json::{Value, json};
use std::io::{self, BufRead, BufReader, BufWriter, Write};

fn main() -> io::Result<()> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut reader = BufReader::new(stdin.lock());
    let mut writer = BufWriter::new(stdout.lock());

    while let Some(payload) = read_message(&mut reader)? {
        let message: Value = match serde_json::from_slice(&payload) {
            Ok(message) => message,
            Err(_) => continue,
        };

        let Some(method) = message.get("method").and_then(Value::as_str) else {
            continue;
        };
        let id = message.get("id").cloned();

        match method {
            "initialize" => {
                if let Some(id) = id {
                    write_message(
                        &mut writer,
                        &json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "result": {
                                "capabilities": {
                                    "completionProvider": {
                                        "resolveProvider": false,
                                        "triggerCharacters": ["."]
                                    },
                                    "hoverProvider": true,
                                    "definitionProvider": true
                                },
                                "serverInfo": {
                                    "name": "fake-upstream",
                                    "version": "test"
                                }
                            }
                        }),
                    )?;
                }
            }
            "shutdown" => {
                if let Some(id) = id {
                    write_message(
                        &mut writer,
                        &json!({"jsonrpc": "2.0", "id": id, "result": Value::Null}),
                    )?;
                }
            }
            "textDocument/completion" => {
                if let Some(id) = id {
                    write_message(
                        &mut writer,
                        &json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "result": {
                                "isIncomplete": false,
                                "items": [
                                    {
                                        "label": "upstream-item",
                                        "kind": 3,
                                        "detail": "proxied completion"
                                    }
                                ]
                            }
                        }),
                    )?;
                }
            }
            "textDocument/hover" => {
                if let Some(id) = id {
                    write_message(
                        &mut writer,
                        &json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "result": {
                                "contents": {
                                    "kind": "plaintext",
                                    "value": "upstream hover"
                                }
                            }
                        }),
                    )?;
                }
            }
            "textDocument/definition" => {
                if let Some(id) = id {
                    let uri = message["params"]["textDocument"]["uri"]
                        .as_str()
                        .unwrap_or("file:///missing.ts");
                    write_message(
                        &mut writer,
                        &json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "result": {
                                "uri": uri,
                                "range": {
                                    "start": {"line": 0, "character": 0},
                                    "end": {"line": 0, "character": 8}
                                }
                            }
                        }),
                    )?;
                }
            }
            "textDocument/didSave" => {
                let uri = message["params"]["textDocument"]["uri"]
                    .as_str()
                    .unwrap_or("file:///missing.ts");
                write_message(
                    &mut writer,
                    &json!({
                        "jsonrpc": "2.0",
                        "method": "textDocument/publishDiagnostics",
                        "params": {
                            "uri": uri,
                            "diagnostics": [
                                {
                                    "range": {
                                        "start": {"line": 0, "character": 0},
                                        "end": {"line": 0, "character": 8}
                                    },
                                    "severity": 2,
                                    "source": "fake-upstream",
                                    "message": "upstream diagnostic"
                                }
                            ]
                        }
                    }),
                )?;
            }
            "initialized"
            | "textDocument/didOpen"
            | "textDocument/didChange"
            | "textDocument/didClose" => {}
            _ => {
                if let Some(id) = id {
                    write_message(
                        &mut writer,
                        &json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "error": {
                                "code": -32601,
                                "message": "Method not found"
                            }
                        }),
                    )?;
                }
            }
        }
    }

    Ok(())
}

fn read_message(reader: &mut impl BufRead) -> io::Result<Option<Vec<u8>>> {
    let mut content_length = None;

    loop {
        let mut line = String::new();
        let read = reader.read_line(&mut line)?;
        if read == 0 {
            return Ok(None);
        }

        if line == "\r\n" {
            break;
        }

        if let Some(value) = line.strip_prefix("Content-Length:") {
            content_length = Some(value.trim().parse::<usize>().map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "invalid content length")
            })?);
        }
    }

    let length = content_length
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing content length"))?;
    let mut payload = vec![0; length];
    reader.read_exact(&mut payload)?;
    Ok(Some(payload))
}

fn write_message(writer: &mut impl Write, payload: &Value) -> io::Result<()> {
    let bytes = serde_json::to_vec(payload)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    write!(writer, "Content-Length: {}\r\n\r\n", bytes.len())?;
    writer.write_all(&bytes)?;
    writer.flush()
}
