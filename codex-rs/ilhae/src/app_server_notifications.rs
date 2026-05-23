use std::time::Duration;

use codex_app_server_protocol::GpuQueueLeaseMode as AppServerGpuQueueLeaseMode;
use codex_app_server_protocol::GpuQueueLeaseSnapshot as AppServerGpuQueueLeaseSnapshot;
use codex_app_server_protocol::GpuQueueLeaseState as AppServerGpuQueueLeaseState;
use codex_app_server_protocol::GpuQueueLlmRuntimeState as AppServerGpuQueueLlmRuntimeState;
use codex_app_server_protocol::GpuQueueRuntimeEventNotification;
use codex_app_server_protocol::GpuQueueRuntimeEventType as AppServerGpuQueueRuntimeEventType;
use codex_app_server_protocol::ServerNotification;
use futures_util::StreamExt;
use tokio::sync::mpsc;

use crate::gpu_queue::api::GpuQueueRuntimeEvent;
use crate::gpu_queue::api::GpuQueueRuntimeEventType;
use crate::gpu_queue::api::LeaseInfo;
use crate::gpu_queue::api::LeaseMode;
use crate::gpu_queue::api::LeaseState;
use crate::gpu_queue::api::LlmRuntimeState;

const RECONNECT_DELAY: Duration = Duration::from_secs(2);

pub fn spawn_app_server_external_notification_bridge() -> mpsc::Receiver<ServerNotification> {
    let (tx, rx) = mpsc::channel(256);
    spawn_gpu_queue_runtime_event_bridge(tx);
    rx
}

fn spawn_gpu_queue_runtime_event_bridge(tx: mpsc::Sender<ServerNotification>) {
    let addr = crate::gpu_queue::api::default_listen_addr();
    tokio::spawn(async move {
        let client = reqwest::Client::new();
        loop {
            if tx.is_closed() {
                break;
            }
            if let Err(err) = forward_gpu_queue_event_stream(&client, &addr, &tx).await {
                tracing::debug!(
                    error = ?err,
                    addr,
                    "GPU queue event stream bridge disconnected"
                );
            }
            tokio::time::sleep(RECONNECT_DELAY).await;
        }
    });
}

async fn forward_gpu_queue_event_stream(
    client: &reqwest::Client,
    addr: &str,
    tx: &mpsc::Sender<ServerNotification>,
) -> anyhow::Result<()> {
    let response = client
        .get(gpu_queue_events_url(addr))
        .header(reqwest::header::ACCEPT, "text/event-stream")
        .send()
        .await?;
    if !response.status().is_success() {
        anyhow::bail!("GPU queue event stream returned {}", response.status());
    }

    let mut stream = response.bytes_stream();
    let mut buffer = String::new();
    let mut data_lines = Vec::<String>::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        buffer.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(newline) = buffer.find('\n') {
            let mut line = buffer.drain(..=newline).collect::<String>();
            if line.ends_with('\n') {
                line.pop();
            }
            if line.ends_with('\r') {
                line.pop();
            }
            if line.is_empty() {
                if forward_sse_data(&mut data_lines, tx).await? {
                    return Ok(());
                }
                continue;
            }
            if let Some(data) = line.strip_prefix("data:") {
                data_lines.push(data.trim_start().to_string());
            }
        }
    }
    let _ = forward_sse_data(&mut data_lines, tx).await?;
    Ok(())
}

async fn forward_sse_data(
    data_lines: &mut Vec<String>,
    tx: &mpsc::Sender<ServerNotification>,
) -> anyhow::Result<bool> {
    if data_lines.is_empty() {
        return Ok(false);
    }
    let data = data_lines.join("\n");
    data_lines.clear();
    let event: GpuQueueRuntimeEvent = serde_json::from_str(&data)?;
    Ok(tx
        .send(gpu_queue_runtime_event_to_server_notification(event))
        .await
        .is_err())
}

fn gpu_queue_runtime_event_to_server_notification(
    event: GpuQueueRuntimeEvent,
) -> ServerNotification {
    ServerNotification::GpuQueueRuntimeEvent(GpuQueueRuntimeEventNotification {
        event_id: event.event_id,
        created_at: unix_secs_to_i64(event.created_at),
        event_type: gpu_queue_runtime_event_type(event.event_type),
        message: event.message,
        llm_state: llm_runtime_state(event.llm_state),
        lease: event.lease.map(lease_snapshot),
    })
}

fn gpu_queue_runtime_event_type(
    event_type: GpuQueueRuntimeEventType,
) -> AppServerGpuQueueRuntimeEventType {
    match event_type {
        GpuQueueRuntimeEventType::LeaseQueued => AppServerGpuQueueRuntimeEventType::LeaseQueued,
        GpuQueueRuntimeEventType::LeaseGranted => AppServerGpuQueueRuntimeEventType::LeaseGranted,
        GpuQueueRuntimeEventType::LeaseReleased => AppServerGpuQueueRuntimeEventType::LeaseReleased,
        GpuQueueRuntimeEventType::LeaseExpired => AppServerGpuQueueRuntimeEventType::LeaseExpired,
        GpuQueueRuntimeEventType::LlmStopping => AppServerGpuQueueRuntimeEventType::LlmStopping,
        GpuQueueRuntimeEventType::LlmWaitingForIdle => {
            AppServerGpuQueueRuntimeEventType::LlmWaitingForIdle
        }
        GpuQueueRuntimeEventType::LlmIdleWaitTimedOut => {
            AppServerGpuQueueRuntimeEventType::LlmIdleWaitTimedOut
        }
        GpuQueueRuntimeEventType::LlmStopped => AppServerGpuQueueRuntimeEventType::LlmStopped,
        GpuQueueRuntimeEventType::LlmStarting => AppServerGpuQueueRuntimeEventType::LlmStarting,
        GpuQueueRuntimeEventType::LlmRunning => AppServerGpuQueueRuntimeEventType::LlmRunning,
        GpuQueueRuntimeEventType::LlmStopFailed => AppServerGpuQueueRuntimeEventType::LlmStopFailed,
        GpuQueueRuntimeEventType::LlmStartFailed => {
            AppServerGpuQueueRuntimeEventType::LlmStartFailed
        }
    }
}

fn lease_snapshot(lease: LeaseInfo) -> AppServerGpuQueueLeaseSnapshot {
    AppServerGpuQueueLeaseSnapshot {
        lease_id: lease.lease_id,
        owner: lease.owner,
        kind: lease.kind,
        mode: lease_mode(lease.mode),
        state: lease_state(lease.state),
        preempt_llm: lease.preempt_llm,
        llm_was_preempted: lease.llm_was_preempted,
        queued_at: unix_secs_to_i64(lease.queued_at),
        granted_at: lease.granted_at.map(unix_secs_to_i64),
        expires_at: lease.expires_at.map(unix_secs_to_i64),
    }
}

fn lease_mode(mode: LeaseMode) -> AppServerGpuQueueLeaseMode {
    match mode {
        LeaseMode::Exclusive => AppServerGpuQueueLeaseMode::Exclusive,
        LeaseMode::Shared => AppServerGpuQueueLeaseMode::Shared,
    }
}

fn lease_state(state: LeaseState) -> AppServerGpuQueueLeaseState {
    match state {
        LeaseState::Granted => AppServerGpuQueueLeaseState::Granted,
        LeaseState::Pending => AppServerGpuQueueLeaseState::Pending,
    }
}

fn llm_runtime_state(state: LlmRuntimeState) -> AppServerGpuQueueLlmRuntimeState {
    match state {
        LlmRuntimeState::Running => AppServerGpuQueueLlmRuntimeState::Running,
        LlmRuntimeState::Stopped => AppServerGpuQueueLlmRuntimeState::Stopped,
        LlmRuntimeState::Starting => AppServerGpuQueueLlmRuntimeState::Starting,
        LlmRuntimeState::Stopping => AppServerGpuQueueLlmRuntimeState::Stopping,
        LlmRuntimeState::Unknown => AppServerGpuQueueLlmRuntimeState::Unknown,
    }
}

fn gpu_queue_events_url(addr: &str) -> String {
    let base_url = if addr.starts_with("http://") || addr.starts_with("https://") {
        addr.trim_end_matches('/').to_string()
    } else {
        format!("http://{}", addr.trim_end_matches('/'))
    };
    format!("{base_url}/events")
}

fn unix_secs_to_i64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}
