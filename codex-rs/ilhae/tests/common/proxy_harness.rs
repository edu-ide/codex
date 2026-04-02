//! Shared ProxyProcess test harness for headless JSON-RPC over stdio tests.
//!
//! Used by: agent_chat, artifact, mock tests.

#![allow(dead_code)]

use serde_json::{Value, json};
use std::ffi::OsString;
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// A headless proxy test harness that communicates via JSON-RPC over stdio.
pub struct ProxyProcess {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    rx: mpsc::Receiver<String>,
    msg_id: u32,
}

impl ProxyProcess {
    fn resolve_proxy_bin() -> std::path::PathBuf {
        if let Ok(mut exe) = std::env::current_exe() {
            exe.pop(); // deps
            let debug_dir = exe.clone();
            let candidate = debug_dir.join(if cfg!(windows) {
                "ilhae-proxy.exe"
            } else {
                "ilhae-proxy"
            });
            if candidate.exists() {
                return candidate;
            }
        }

        let cwd = std::env::current_dir().expect("cwd");
        let mut cur: Option<&std::path::Path> = Some(cwd.as_path());
        while let Some(dir) = cur {
            for subpath in [
                "target/debug/ilhae-proxy",
                "target/release/ilhae-proxy",
                "services/ilhae-agent/target/debug/ilhae-proxy",
                "services/ilhae-agent/target/release/ilhae-proxy",
            ] {
                let candidate = dir.join(subpath);
                if candidate.exists() {
                    return candidate;
                }
            }
            cur = dir.parent();
        }
        cwd.join("target/debug/ilhae-proxy")
    }

    /// Spawn a real proxy (connects to actual LLM).
    pub fn spawn() -> Self {
        Self::spawn_inner(false, None, std::iter::empty::<(OsString, OsString)>())
    }

    /// Spawn a mock proxy (ILHAE_MOCK=true, no LLM needed).
    pub fn spawn_mock() -> Self {
        Self::spawn_inner(true, None, std::iter::empty::<(OsString, OsString)>())
    }

    /// Spawn with optional stderr log file.
    pub fn spawn_with_log(mock: bool, stderr_path: &str) -> Self {
        Self::spawn_inner(
            mock,
            Some(stderr_path),
            std::iter::empty::<(OsString, OsString)>(),
        )
    }

    /// Spawn with extra environment variables.
    pub fn spawn_with_log_and_env<I, K, V>(mock: bool, stderr_path: &str, envs: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<OsString>,
        V: Into<OsString>,
    {
        let vars = envs
            .into_iter()
            .map(|(k, v)| (k.into(), v.into()))
            .collect::<Vec<_>>();
        Self::spawn_inner(mock, Some(stderr_path), vars)
    }

    fn spawn_inner<I>(mock: bool, stderr_path: Option<&str>, envs: I) -> Self
    where
        I: IntoIterator<Item = (OsString, OsString)>,
    {
        let proxy_bin = Self::resolve_proxy_bin();
        assert!(proxy_bin.exists(), "Build the proxy first: cargo build");

        let mut cmd = Command::new(&proxy_bin);
        if mock {
            cmd.env("ILHAE_MOCK", "true");
        }
        cmd.env("RUST_LOG", "info")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped());
        cmd.envs(envs);

        if let Some(path) = stderr_path {
            let stderr_file = std::fs::File::create(path).expect("create stderr log");
            cmd.stderr(Stdio::from(stderr_file));
        } else {
            cmd.stderr(Stdio::inherit());
        }

        let mut child = cmd.spawn().expect("Failed to spawn proxy");

        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();

        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                match line {
                    Ok(l) => {
                        if tx.send(l).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        // Wait for proxy startup
        std::thread::sleep(Duration::from_secs(5));

        ProxyProcess {
            child,
            stdin,
            rx,
            msg_id: 0,
        }
    }

    /// Send a JSON-RPC request. Returns the message ID.
    pub fn send(&mut self, method: &str, params: Value) -> u32 {
        self.msg_id += 1;
        let req = json!({
            "jsonrpc": "2.0", "id": self.msg_id,
            "method": method, "params": params,
        });
        writeln!(self.stdin, "{}", req).expect("write to proxy stdin");
        self.msg_id
    }

    /// Read until we get the response for `target_id`, collecting notifications.
    pub fn read_response(
        &mut self,
        target_id: u32,
        timeout: Duration,
    ) -> (Option<Value>, Vec<Value>) {
        let deadline = Instant::now() + timeout;
        let mut notifications = Vec::new();

        while Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match self
                .rx
                .recv_timeout(remaining.min(Duration::from_millis(500)))
            {
                Ok(line) => {
                    eprintln!("[ProxyHarness Rx] {}", line);
                    if let Ok(msg) = serde_json::from_str::<Value>(line.trim()) {
                        if msg.get("id").and_then(|v| v.as_u64()) == Some(target_id as u64) {
                            return (Some(msg), notifications);
                        } else if msg.get("method").is_some() {
                            notifications.push(msg);
                        }
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        (None, notifications)
    }

    /// Send request and wait for response (convenience).
    pub fn call(&mut self, method: &str, params: Value, timeout_secs: u64) -> Value {
        let id = self.send(method, params);
        let (resp, _) = self.read_response(id, Duration::from_secs(timeout_secs));
        resp.expect(&format!("{} should respond", method))
    }

    /// Send a prompt and wait for response + notifications.
    pub fn prompt(&mut self, session_id: &str, message: &str) -> (Value, Vec<Value>) {
        self.prompt_with_timeout(session_id, message, 120)
    }

    /// Send a prompt with custom timeout (seconds).
    pub fn prompt_with_timeout(
        &mut self,
        session_id: &str,
        message: &str,
        timeout_secs: u64,
    ) -> (Value, Vec<Value>) {
        let id = self.send(
            "session/prompt",
            json!({
                "sessionId": session_id,
                "prompt": [{ "type": "text", "text": message }]
            }),
        );
        let (resp, notifs) = self.read_response(id, Duration::from_secs(timeout_secs));
        let resp = resp.expect("Prompt should respond");
        (resp, notifs)
    }

    /// Initialize + create session, returns session_id.
    pub fn init_and_create_session(&mut self) -> String {
        let resp = self.call(
            "initialize",
            json!({
                "protocolVersion": "2025-11-25",
                "clientCapabilities": {},
                "clientInfo": { "name": "e2e-test", "version": "1.0" }
            }),
            30,
        );
        assert!(
            resp.get("result").is_some(),
            "Initialize failed: {:?}",
            resp
        );

        let resp = self.call(
            "session/new",
            json!({ "cwd": "/tmp", "mcpServers": [], "mode": "yolo" }),
            15,
        );
        resp["result"]["sessionId"]
            .as_str()
            .expect("sessionId missing")
            .to_string()
    }
}

impl Drop for ProxyProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
