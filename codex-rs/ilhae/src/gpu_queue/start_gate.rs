use std::time::Duration;

use anyhow::Context;
use tokio::time::Instant;

use super::api::StatusResponse;
use super::api::default_listen_addr;

const START_GATE_DISABLED_ENV: &str = "ILHAE_GPU_QUEUE_START_GATE_DISABLED";
const GPU_QUEUE_ADDR_ENV: &str = "ILHAE_GPU_QUEUE_ADDR";
const START_GATE_TIMEOUT_SECS_ENV: &str = "ILHAE_GPU_QUEUE_START_GATE_TIMEOUT_SECS";
const START_GATE_POLL_MS_ENV: &str = "ILHAE_GPU_QUEUE_START_GATE_POLL_MS";
const DEFAULT_START_GATE_TIMEOUT_SECS: u64 = 900;
const DEFAULT_START_GATE_POLL_MS: u64 = 1_000;
const STATUS_REQUEST_TIMEOUT_MS: u64 = 1_000;

pub async fn wait_for_native_runtime_start_gate() -> anyhow::Result<()> {
    if env_flag(START_GATE_DISABLED_ENV) {
        return Ok(());
    }

    let timeout = env_u64(START_GATE_TIMEOUT_SECS_ENV, DEFAULT_START_GATE_TIMEOUT_SECS);
    if timeout == 0 {
        return Ok(());
    }

    let poll_interval =
        Duration::from_millis(env_u64(START_GATE_POLL_MS_ENV, DEFAULT_START_GATE_POLL_MS));
    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(STATUS_REQUEST_TIMEOUT_MS))
        .build()?;
    let status_url = format!(
        "{}/status",
        normalize_gpu_queue_base_url(
            &std::env::var(GPU_QUEUE_ADDR_ENV).unwrap_or_else(|_| default_listen_addr()),
        )
    );
    let deadline = Instant::now() + Duration::from_secs(timeout);

    loop {
        let Some(status) = fetch_status(&client, &status_url).await? else {
            return Ok(());
        };

        if !status_blocks_native_runtime_start(&status) {
            return Ok(());
        }

        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            let lease_id = status
                .active_lease
                .as_ref()
                .map(|lease| lease.lease_id.as_str())
                .unwrap_or("unknown");
            anyhow::bail!(
                "timed out waiting for GPU lease `{lease_id}` before starting local LLM runtime"
            );
        }
        tokio::time::sleep(remaining.min(poll_interval)).await;
    }
}

async fn fetch_status(
    client: &reqwest::Client,
    status_url: &str,
) -> anyhow::Result<Option<StatusResponse>> {
    let response = match client.get(status_url).send().await {
        Ok(response) => response,
        Err(error) => return handle_status_request_error(error),
    };

    if !response.status().is_success() {
        return Ok(None);
    }

    let text = response.text().await?;
    serde_json::from_str(&text)
        .map(Some)
        .with_context(|| format!("failed to parse GPU queue status response: {text}"))
}

fn handle_status_request_error(error: reqwest::Error) -> anyhow::Result<Option<StatusResponse>> {
    if error.is_connect() || error.is_timeout() {
        return Ok(None);
    }
    Err(error).context("failed to query GPU queue status before starting local LLM runtime")
}

fn status_blocks_native_runtime_start(status: &StatusResponse) -> bool {
    status
        .active_lease
        .as_ref()
        .is_some_and(|lease| lease.preempt_llm)
}

fn normalize_gpu_queue_base_url(addr: &str) -> String {
    let trimmed = addr.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return "http://127.0.0.1:43290".to_string();
    }
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        trimmed.to_string()
    } else {
        format!("http://{trimmed}")
    }
}

fn env_flag(name: &str) -> bool {
    std::env::var(name)
        .map(|value| {
            matches!(
                value.to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

fn env_u64(name: &str, default_value: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(default_value)
}

#[cfg(test)]
mod tests {
    use super::super::api::LeaseInfo;
    use super::super::api::LeaseMode;
    use super::super::api::LeaseState;
    use super::super::api::LlmRuntimeState;
    use super::*;

    #[test]
    fn active_preempting_lease_blocks_native_runtime_start() {
        let status = status_with_active_lease(active_lease(/*preempt_llm*/ true));

        assert!(status_blocks_native_runtime_start(&status));
    }

    #[test]
    fn active_non_preempting_lease_does_not_block_native_runtime_start() {
        let status = status_with_active_lease(active_lease(/*preempt_llm*/ false));

        assert!(!status_blocks_native_runtime_start(&status));
    }

    #[test]
    fn pending_leases_do_not_block_native_runtime_start() {
        let mut status = empty_status();
        status.pending_leases = vec![active_lease(/*preempt_llm*/ true)];

        assert!(!status_blocks_native_runtime_start(&status));
    }

    #[test]
    fn normalizes_plain_gpu_queue_addr() {
        assert_eq!(
            normalize_gpu_queue_base_url("127.0.0.1:43290/"),
            "http://127.0.0.1:43290"
        );
    }

    fn status_with_active_lease(lease: LeaseInfo) -> StatusResponse {
        StatusResponse {
            active_lease: Some(lease),
            ..empty_status()
        }
    }

    fn empty_status() -> StatusResponse {
        StatusResponse {
            uptime_seconds: 0,
            llm_state: LlmRuntimeState::Stopped,
            active_lease: None,
            pending_leases: Vec::new(),
        }
    }

    fn active_lease(preempt_llm: bool) -> LeaseInfo {
        LeaseInfo {
            lease_id: "gpu-lease-1".to_string(),
            owner: "videoeditor".to_string(),
            kind: "video".to_string(),
            mode: LeaseMode::Exclusive,
            state: LeaseState::Granted,
            preempt_llm,
            llm_was_preempted: preempt_llm,
            ttl_seconds: 3600,
            queued_at: 1,
            granted_at: Some(1),
            expires_at: Some(3601),
        }
    }
}
