use crate::tools::handlers::lsp::LspOperation;
use lsp_types::{
    ClientCapabilities, InitializeParams, Position, SymbolKind, TextDocumentIdentifier, Url,
};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{Mutex, oneshot};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LspStatus {
    Uninitialized,
    Initializing,
    Ready,
    ShuttingDown,
    Shutdown,
    Error,
}

pub struct LspServerInstance {
    process: Mutex<Child>,
    stdin: Mutex<Option<ChildStdin>>,
    pending_requests: Mutex<HashMap<u64, oneshot::Sender<Value>>>,
    next_id: Mutex<u64>,
    server_name: String,
    diagnostics: Mutex<HashMap<String, Vec<Value>>>,
    status: Mutex<LspStatus>,
}

impl LspServerInstance {
    pub async fn new(cmd: &str, args: &[&str]) -> anyhow::Result<Arc<Self>> {
        let mut child = Command::new(cmd)
            .args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()?;

        let stdin = child.stdin.take().expect("Failed to open stdin");
        let stdout = child.stdout.take().expect("Failed to open stdout");

        let instance = Arc::new(Self {
            process: Mutex::new(child),
            stdin: Mutex::new(Some(stdin)),
            pending_requests: Mutex::new(HashMap::new()),
            next_id: Mutex::new(1),
            server_name: cmd.to_string(),
            diagnostics: Mutex::new(HashMap::new()),
            status: Mutex::new(LspStatus::Uninitialized),
        });

        // Spawn reader loop
        let instance_clone = Arc::clone(&instance);
        tokio::spawn(async move {
            instance_clone.reader_loop(stdout).await;
        });

        Ok(instance)
    }

    async fn cache_diagnostics(&self, params: &Value) {
        if let Some(uri) = params.get("uri").and_then(|u| u.as_str()) {
            if let Some(diags) = params.get("diagnostics").and_then(|d| d.as_array()) {
                self.diagnostics
                    .lock()
                    .await
                    .insert(uri.to_string(), diags.clone());
            }
        }
    }

    pub async fn get_diagnostics(&self, uri: &str) -> Vec<Value> {
        self.diagnostics
            .lock()
            .await
            .get(uri)
            .cloned()
            .unwrap_or_default()
    }

    async fn reader_loop(&self, stdout: ChildStdout) {
        let mut reader = BufReader::new(stdout);
        loop {
            let mut line = String::new();
            // Read headers
            let mut content_length: Option<usize> = None;
            loop {
                line.clear();
                if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
                    return; // EOF
                }
                let line = line.trim();
                if line.is_empty() {
                    break;
                }
                if line.to_lowercase().starts_with("content-length:") {
                    if let Some(len_str) = line.split(':').nth(1) {
                        content_length = len_str.trim().parse().ok();
                    }
                }
            }

            if let Some(mut len) = content_length {
                let mut body = vec![0; len];
                if reader.read_exact(&mut body).await.is_err() {
                    break; // EOF or error
                }

                if let Ok(msg) = serde_json::from_slice::<Value>(&body) {
                    if let Some(id_val) = msg.get("id") {
                        if let Some(id) = id_val.as_u64() {
                            let mut pending = self.pending_requests.lock().await;
                            if let Some(tx) = pending.remove(&id) {
                                let _ = tx.send(msg);
                            }
                        }
                    } else if let Some(method) = msg.get("method").and_then(|m| m.as_str()) {
                        if method == "textDocument/publishDiagnostics" {
                            if let Some(params) = msg.get("params") {
                                self.cache_diagnostics(params).await;
                            }
                        }
                    }
                }
            }
        }
    }

    pub async fn send_request(&self, method: &str, params: Value) -> anyhow::Result<Value> {
        let mut retries = 0;
        let max_retries = 3;
        let mut delay_ms = 500;

        loop {
            let id = {
                let mut nid = self.next_id.lock().await;
                let id = *nid;
                *nid += 1;
                id
            };

            let msg = json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": method,
                "params": params.clone()
            });

            let s = serde_json::to_string(&msg)?;
            let payload = format!("Content-Length: {}\r\n\r\n{}", s.len(), s);

            let (tx, rx) = oneshot::channel();
            self.pending_requests.lock().await.insert(id, tx);

            if let Some(stdin) = self.stdin.lock().await.as_mut() {
                stdin.write_all(payload.as_bytes()).await?;
                stdin.flush().await?;
            } else {
                return Err(anyhow::anyhow!("stdin disconnected"));
            }

            let resp = rx.await?;
            if let Some(err) = resp.get("error") {
                if let Some(code) = err.get("code").and_then(|c| c.as_i64()) {
                    if (code == -32801 || code == -32800) && retries < max_retries {
                        // -32801: Content Modified, -32800: Request Cancelled
                        tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                        retries += 1;
                        delay_ms *= 2;
                        continue;
                    }
                }
                return Err(anyhow::anyhow!("LSP Error: {:?}", err));
            }

            return Ok(resp.get("result").cloned().unwrap_or(Value::Null));
        }
    }

    pub async fn send_notification(&self, method: &str, params: Value) -> anyhow::Result<()> {
        let msg = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params
        });

        let s = serde_json::to_string(&msg)?;
        let payload = format!("Content-Length: {}\r\n\r\n{}", s.len(), s);

        if let Some(stdin) = self.stdin.lock().await.as_mut() {
            stdin.write_all(payload.as_bytes()).await?;
            stdin.flush().await?;
        }
        Ok(())
    }

    pub async fn initialize(&self, root_uri: &str) -> anyhow::Result<()> {
        #[allow(deprecated)]
        let params = InitializeParams {
            process_id: Some(std::process::id() as u32),
            root_uri: Some(Url::parse(root_uri)?),
            capabilities: ClientCapabilities::default(),
            ..Default::default()
        };

        {
            let mut status = self.status.lock().await;
            *status = LspStatus::Initializing;
        }

        self.send_request("initialize", serde_json::to_value(params)?)
            .await?;
        self.send_notification("initialized", json!({})).await?;

        {
            let mut status = self.status.lock().await;
            *status = LspStatus::Ready;
        }
        Ok(())
    }

    pub async fn shutdown(&self) -> anyhow::Result<()> {
        {
            let mut status = self.status.lock().await;
            if *status == LspStatus::Shutdown || *status == LspStatus::ShuttingDown {
                return Ok(());
            }
            *status = LspStatus::ShuttingDown;
        }

        let _ = self.send_request("shutdown", json!({})).await;
        let _ = self.send_notification("exit", json!({})).await;

        let mut status = self.status.lock().await;
        *status = LspStatus::Shutdown;
        Ok(())
    }
}

pub struct LspServerManager {
    servers: Mutex<HashMap<String, Arc<LspServerInstance>>>,
}

impl LspServerManager {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            servers: Mutex::new(HashMap::new()),
        })
    }

    pub async fn get_or_start_server(
        &self,
        ext: &str,
        root_uri: &str,
    ) -> anyhow::Result<Arc<LspServerInstance>> {
        let mut servers = self.servers.lock().await;

        let cmd = match ext {
            "ts" | "tsx" | "js" | "jsx" => "typescript-language-server",
            "rs" => "rust-analyzer",
            "py" => "pylsp",
            _ => return Err(anyhow::anyhow!("Unsupported file extension: {}", ext)),
        };

        if let Some(instance) = servers.get(cmd) {
            return Ok(Arc::clone(instance));
        }

        let args = match cmd {
            "typescript-language-server" => vec!["--stdio"],
            _ => vec![], // rust-analyzer and pylsp default to stdio
        };

        let instance = LspServerInstance::new(cmd, &args).await?;
        instance.initialize(root_uri).await?;

        servers.insert(cmd.to_string(), Arc::clone(&instance));
        Ok(instance)
    }

    pub async fn ensure_file_open(
        &self,
        instance: &Arc<LspServerInstance>,
        file_path: &str,
        content: &str,
    ) -> anyhow::Result<()> {
        // Normally we'd track opened files. For simplicity in phase 1, we just re-open or send didChange.
        let uri = format!("file://{}", file_path);
        instance
            .send_notification(
                "textDocument/didOpen",
                json!({
                    "textDocument": {
                        "uri": uri,
                        "languageId": "rust", // TODO infer from ext
                        "version": 1,
                        "text": content,
                    }
                }),
            )
            .await?;
        Ok(())
    }

    pub async fn get_current_diagnostics(
        &self,
        ext: &str,
        root_uri: &str,
        file_path: &str,
    ) -> anyhow::Result<Vec<Value>> {
        let instance = self.get_or_start_server(ext, root_uri).await?;

        // Wait a small amount of time for any pending diagnostics to arrive
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        // The URI format used for caching
        let uri = format!("file://{}", file_path);

        let mut diagnostics = instance.diagnostics.lock().await;
        let diags = diagnostics.get(&uri).cloned().unwrap_or_default();

        Ok(diags)
    }
}
