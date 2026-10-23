use std::future::Future;
use std::io::Write;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use codex_config::ToolResultProcessorConfig;
#[cfg(windows)]
use codex_utils_pty::JobObject;
use serde::Deserialize;
use serde::Serialize;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

/// Runs one opt-in local processor. Failure leaves the original tool output intact;
/// tool execution itself is never retried. The command receives one JSON document on stdin.
pub async fn process_tool_result(
    config: &ToolResultProcessorConfig,
    request: &impl Serialize,
    cwd: &Path,
    cancelled: impl Future<Output = ()>,
) -> Option<String> {
    config.validate().ok()?;
    let mut input = BoundedInput {
        bytes: Vec::new(),
        limit: config.max_input_bytes,
    };
    serde_json::to_writer(&mut input, request).ok()?;
    let mut command = Command::new(&config.command[0]);
    command
        .args(&config.command[1..])
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(/*pgroup*/ 0);
    #[cfg(windows)]
    let job = JobObject::create().ok()?;
    #[cfg(windows)]
    let mut child = job.spawn_contained(&mut command).ok()?;
    #[cfg(not(windows))]
    let mut child = command.spawn().ok()?;
    let _process_tree = ProcessorProcessTree {
        process_id: child.id(),
        #[cfg(windows)]
        job,
    };
    let mut stdin = child.stdin.take()?;
    let stdout = child.stdout.take()?;
    let result = {
        let exchange = async {
            let write = async move {
                stdin.write_all(&input.bytes).await?;
                stdin.shutdown().await?;
                drop(stdin);
                Ok::<(), std::io::Error>(())
            };
            let read = async {
                let mut bytes = Vec::new();
                stdout
                    .take(config.max_output_bytes as u64 + 1)
                    .read_to_end(&mut bytes)
                    .await?;
                if bytes.len() > config.max_output_bytes {
                    return Err(std::io::Error::other("processor output limit exceeded"));
                }
                Ok(bytes)
            };
            let (_, bytes, status) = tokio::try_join!(write, read, child.wait())?;
            if !status.success() {
                return Err(std::io::Error::other("processor exited unsuccessfully"));
            }
            serde_json::from_slice::<ProcessorResponse>(&bytes).map_err(std::io::Error::other)
        };
        tokio::select! {
            result = tokio::time::timeout(Duration::from_millis(config.timeout_ms), exchange) => result.ok().and_then(Result::ok),
            () = cancelled => None,
        }
    };
    if result.is_none() {
        // Reap the direct child before returning; the guard also kills descendants on drop.
        #[cfg(unix)]
        if let Some(process_id) = child.id() {
            let _ = codex_utils_pty::process_group::kill_process_group(process_id);
        }
        #[cfg(windows)]
        let _ = _process_tree.job.terminate();
        let _ = child.start_kill();
        let _ = tokio::time::timeout(Duration::from_secs(1), child.wait()).await;
    }
    match result? {
        ProcessorResponse::ReplaceText { version: 1, text } => Some(text),
        ProcessorResponse::Unchanged { version } => {
            let _ = version;
            None
        }
        ProcessorResponse::ReplaceText { .. } => None,
    }
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum ProcessorResponse {
    Unchanged { version: u8 },
    ReplaceText { version: u8, text: String },
}

struct BoundedInput {
    bytes: Vec<u8>,
    limit: usize,
}

impl Write for BoundedInput {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            return Err(std::io::Error::other("processor input limit exceeded"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

struct ProcessorProcessTree {
    process_id: Option<u32>,
    #[cfg(windows)]
    job: JobObject,
}

impl Drop for ProcessorProcessTree {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(process_id) = self.process_id {
            let _ = codex_utils_pty::process_group::kill_process_group(process_id);
        }
        #[cfg(windows)]
        let _ = self.job.terminate();
    }
}

#[cfg(all(test, unix))]
#[path = "tool_result_processor_tests.rs"]
mod tests;
