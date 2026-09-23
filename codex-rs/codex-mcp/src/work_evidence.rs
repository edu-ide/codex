//! Persist source-verified observations before delivering them to Brain. Only
//! evidence records are retried; the originating MCP operation is never replayed.

use std::fs;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use chrono::DateTime;
use chrono::Utc;
use codex_protocol::mcp::CallToolResult;
use codex_rmcp_client::RmcpClient;
use serde_json::Value;
use sha1::Digest;
use sha1::Sha1;

pub(crate) const RECORD_TOOL: &str = "brain_work_record";
const ENVELOPE_KEY: &str = "ugot/work-evidence";
const MAX_BROWSER_ARCHIVE_BYTES: usize = 256 * 1024;
// JSON escaping can expand the bounded HTML and excerpt by up to six times.
const MAX_OUTBOX_RECORD_BYTES: usize = 2 * 1024 * 1024;
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub(crate) struct WorkEvidenceOutbox {
    directory: PathBuf,
    running: AtomicBool,
    dirty: AtomicBool,
}

impl WorkEvidenceOutbox {
    pub(crate) fn new(codex_home: &Path) -> Self {
        Self {
            directory: codex_home.join("work-evidence-outbox"),
            running: AtomicBool::new(false),
            dirty: AtomicBool::new(false),
        }
    }

    pub(crate) async fn observe(
        self: &Arc<Self>,
        server: &str,
        tool: &str,
        request_meta: Option<&Value>,
        result: &mut CallToolResult,
    ) {
        let events = events_from_result(server, tool, request_meta, result);
        if events.is_empty() {
            return;
        }
        let receipt = receipt_for_events(&events);
        let outbox = Arc::clone(self);
        let saved = tokio::task::spawn_blocking(move || outbox.persist(&events)).await;
        if !matches!(saved, Ok(Ok(()))) {
            // Do not include tool content, account identifiers, or local paths.
            tracing::warn!("work evidence could not be persisted; original MCP result preserved");
        } else {
            self.dirty.store(true, Ordering::Release);
            result
                .content
                .push(serde_json::json!({"type": "text", "text": receipt}));
        }
    }

    pub(crate) fn claim_delivery(&self) -> bool {
        self.directory.is_dir()
            && self
                .running
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
    }

    pub(crate) fn release_delivery(&self) {
        self.running.store(false, Ordering::Release);
    }

    pub(crate) async fn wait_for_delivery(&self) -> bool {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        loop {
            let pending = fs::read_dir(&self.directory).is_ok_and(|mut entries| {
                entries.any(|entry| {
                    entry.is_ok_and(|entry| {
                        entry
                            .path()
                            .extension()
                            .is_some_and(|extension| extension == "json")
                    })
                })
            });
            if !pending || tokio::time::Instant::now() >= deadline {
                return pending;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    pub(crate) async fn deliver(self: &Arc<Self>, client: Arc<RmcpClient>) {
        let mut delay = 2;
        loop {
            self.dirty.store(false, Ordering::Release);
            let outbox = Arc::clone(self);
            let records = match tokio::task::spawn_blocking(move || outbox.pending()).await {
                Ok(Ok(records)) => records,
                _ => {
                    tracing::warn!("work evidence outbox could not be read");
                    break;
                }
            };
            if records.is_empty() {
                self.release_delivery();
                if self.dirty.swap(false, Ordering::AcqRel) && self.claim_delivery() {
                    continue;
                }
                return;
            }
            if client.is_closed().await {
                break;
            }
            let mut failed = false;
            for (path, event) in records {
                let event_id = event["event_id"].as_str().unwrap_or_default();
                let response = client
                    .call_tool(
                        RECORD_TOOL.to_string(),
                        Some(event.clone()),
                        /*meta*/ None,
                        Some(Duration::from_secs(10)),
                    )
                    .await;
                if response
                    .map(crate::binding::call_tool_result_from_rmcp)
                    .is_ok_and(|result| acknowledged(&result, event_id))
                {
                    let removed = tokio::task::spawn_blocking(move || fs::remove_file(path)).await;
                    if !matches!(removed, Ok(Ok(()))) {
                        failed = true;
                    }
                } else {
                    failed = true;
                }
            }
            if failed {
                // A durable record remains unchanged, including its observation time.
                tracing::debug!("work evidence delivery pending; retrying only Brain records");
                tokio::time::sleep(Duration::from_secs(delay)).await;
                delay = (delay * 2).min(60);
            } else {
                delay = 2;
            }
        }
        self.release_delivery();
    }

    fn persist(&self, events: &[Value]) -> Result<()> {
        fs::create_dir_all(&self.directory)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&self.directory, fs::Permissions::from_mode(0o700))?;
        }
        for event in events {
            let id = event["event_id"].as_str().context("missing evidence ID")?;
            let path = self.directory.join(format!("{id}.json"));
            let bytes = serde_json::to_vec(event)?;
            if bytes.len() > MAX_OUTBOX_RECORD_BYTES {
                bail!("work evidence exceeds outbox record limit");
            }
            if path.exists() {
                if fs::read(&path)? != bytes {
                    bail!("conflicting work evidence ID");
                }
                continue;
            }
            let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let temporary = self
                .directory
                .join(format!(".{id}.{}.{sequence}.tmp", std::process::id()));
            let mut options = fs::OpenOptions::new();
            options.create_new(true).write(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&temporary)?;
            let saved = (|| -> Result<()> {
                file.write_all(&bytes)?;
                file.sync_all()?;
                drop(file);
                fs::rename(&temporary, &path)?;
                Ok(())
            })();
            if saved.is_err() {
                let _ = fs::remove_file(&temporary);
            }
            saved?;
        }
        #[cfg(unix)]
        fs::File::open(&self.directory)?.sync_all()?;
        Ok(())
    }

    fn pending(&self) -> Result<Vec<(PathBuf, Value)>> {
        let mut paths = fs::read_dir(&self.directory)?
            .filter_map(std::result::Result::ok)
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "json")
            })
            .collect::<Vec<_>>();
        paths.sort();
        let mut records = Vec::new();
        for path in paths {
            if fs::metadata(&path)?.len() > MAX_OUTBOX_RECORD_BYTES as u64 {
                continue;
            }
            let Ok(event) = serde_json::from_slice::<Value>(&fs::read(&path)?) else {
                continue;
            };
            let Some(id) = event.get("event_id").and_then(Value::as_str) else {
                continue;
            };
            if path.file_stem().and_then(|stem| stem.to_str()) != Some(id) {
                continue;
            }
            records.push((path, event));
            if records.len() == 16 {
                break;
            }
        }
        Ok(records)
    }
}

pub(crate) fn append_pending_notice(result: &mut CallToolResult) {
    result.content.push(serde_json::json!({
        "type": "text",
        "text": "{\"ugot_work_evidence_sync\":{\"outbox_pending\":true,\"coverage\":\"Some observed evidence is still pending indexing; search results may be incomplete.\"}}"
    }));
}

fn receipt_for_events(events: &[Value]) -> String {
    let mut receipts = Vec::new();
    for event in events {
        let mut receipt = serde_json::Map::new();
        for key in [
            "work_key",
            "source_kind",
            "account_id",
            "profile_id",
            "event_id",
        ] {
            receipt.insert(key.to_string(), event[key].clone());
        }
        receipt.insert("brain_index_status".to_string(), "pending".into());
        receipts.push(Value::Object(receipt));
        if serde_json::to_vec(&receipts).map_or(true, |bytes| bytes.len() > 1900) {
            receipts.pop();
            break;
        }
    }
    serde_json::json!({"ugot_work_evidence": {
        "remaining_count": events.len() - receipts.len(),
        "items": receipts,
    }})
    .to_string()
}

fn bounded(value: Option<&Value>, max: usize) -> Option<&str> {
    value
        .and_then(Value::as_str)
        .filter(|text| !text.trim().is_empty() && text.len() <= max && !text.contains('\0'))
}

fn events_from_result(
    server: &str,
    tool: &str,
    request_meta: Option<&Value>,
    result: &CallToolResult,
) -> Vec<Value> {
    // Provider metadata is trusted only from the configured product integrations.
    let expected_source = match server {
        "browser" | "agent-browser" | "ugot-browser" => "browser",
        "email" | "mail" | "ugot-mail" => "mail",
        "office" | "doc-mcp" | "excel-mcp" => "office",
        _ => return Vec::new(),
    };
    if tool == RECORD_TOOL {
        return Vec::new();
    }
    let Some(meta) = request_meta else {
        return Vec::new();
    };
    let Some(thread_id) = bounded(meta.get("threadId"), 256) else {
        return Vec::new();
    };
    let work_key = match meta.get("ugot/work-context") {
        Some(context) => match bounded(context.get("work_key"), 256) {
            Some(key) => key,
            None => return Vec::new(),
        },
        None => thread_id,
    };
    let operation_id = bounded(meta.get("callId"), 256);
    let Some(envelopes) = result
        .meta
        .as_ref()
        .and_then(|meta| meta.get(ENVELOPE_KEY))
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };
    let tool_id = format!("{server}/{tool}");
    if tool_id.len() > 256 {
        return Vec::new();
    }
    envelopes
        .iter()
        .take(32)
        .enumerate()
        .filter_map(|(index, envelope)| {
            let mut event = serde_json::Map::new();
            for (key, limit) in [
                ("source_kind", 16),
                ("resource_uri", 4096),
                ("title", 1024),
                ("locator", 2048),
                ("account_id", 256),
                ("profile_id", 256),
                ("observed_at", 64),
                ("status", 16),
            ] {
                event.insert(key.to_string(), bounded(envelope.get(key), limit)?.into());
            }
            if event["source_kind"].as_str() != Some(expected_source)
                || !matches!(
                    event["status"].as_str(),
                    Some("observed" | "saved" | "submitted" | "unknown" | "failed")
                )
                || (result.is_error == Some(true) && event["status"] != "failed")
            {
                return None;
            }
            let observed_at = DateTime::parse_from_rfc3339(event["observed_at"].as_str()?).ok()?;
            if observed_at > Utc::now() + chrono::Duration::minutes(5) {
                return None;
            }
            for key in ["account_id", "profile_id"] {
                let scope = event[key].as_str()?;
                if scope.trim() != scope || scope == "*" {
                    return None;
                }
            }
            let content = envelope.get("content")?.as_str()?;
            if content.len() > 65536 || content.contains('\0') {
                return None;
            }
            event.insert("content".to_string(), content.into());
            if let Some(revision) = envelope.get("revision").filter(|value| !value.is_null()) {
                event.insert("revision".to_string(), bounded(Some(revision), 256)?.into());
            }
            if let Some(archive) = envelope
                .get("browser_archive")
                .filter(|value| !value.is_null())
            {
                if expected_source != "browser" {
                    return None;
                }
                let html = bounded(archive.get("html"), MAX_BROWSER_ARCHIVE_BYTES)?;
                let sha256 = bounded(archive.get("sha256"), 64)?;
                if sha256.len() != 64
                    || !sha256
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                    || archive.get("capture_kind").and_then(Value::as_str)
                        != Some("sanitized_dom_html")
                {
                    return None;
                }
                // Brain verifies the digest against these exact UTF-8 bytes.
                event.insert(
                    "browser_archive".to_string(),
                    serde_json::json!({
                        "html": html, "sha256": sha256, "capture_kind": "sanitized_dom_html",
                    }),
                );
            }
            event.insert("work_key".to_string(), work_key.into());
            event.insert("tool_id".to_string(), tool_id.clone().into());
            if let Some(id) = operation_id {
                event.insert("operation_id".to_string(), id.into());
            }
            let identity = serde_json::to_vec(&(thread_id, index, &event)).ok()?;
            let id = format!("mcp-{:x}", Sha1::digest(identity));
            event.insert("event_id".to_string(), id.into());
            Some(Value::Object(event))
        })
        .collect()
}

fn acknowledged(result: &CallToolResult, event_id: &str) -> bool {
    if result.is_error == Some(true) {
        return false;
    }
    let payload = match &result.structured_content {
        Some(value) => value.clone(),
        None => {
            let Some(text) = result
                .content
                .first()
                .and_then(|item| item.get("text"))
                .and_then(Value::as_str)
            else {
                return false;
            };
            let Ok(value) = serde_json::from_str::<Value>(text) else {
                return false;
            };
            value
        }
    };
    payload.get("event_id").and_then(Value::as_str) == Some(event_id)
        && payload.get("inserted").is_some_and(Value::is_boolean)
        && payload.get("is_latest").is_some_and(Value::is_boolean)
}

#[cfg(test)]
#[path = "work_evidence_tests.rs"]
mod tests;
