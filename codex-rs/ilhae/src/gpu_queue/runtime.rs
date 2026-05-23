use super::api::LlmRuntimeState;
use serde::Deserialize;
use std::time::Duration;
use std::time::Instant;

const LLM_IDLE_POLL_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Debug, Clone)]
pub struct NativeLlmRuntime {
    profile_id: Option<String>,
}

impl NativeLlmRuntime {
    pub fn new(profile_id: Option<String>) -> Self {
        Self { profile_id }
    }

    pub async fn state(&self) -> LlmRuntimeState {
        let Some((_, config)) =
            crate::config::get_native_runtime_config(self.profile_id.as_deref())
        else {
            return LlmRuntimeState::Unknown;
        };

        if !config.enabled {
            return LlmRuntimeState::Stopped;
        }

        if crate::startup_main::native_runtime_healthcheck(&config.health_url).await
            || !crate::startup_main::find_native_runtime_pids(&config).is_empty()
        {
            LlmRuntimeState::Running
        } else {
            LlmRuntimeState::Stopped
        }
    }

    pub async fn is_running(&self) -> anyhow::Result<bool> {
        Ok(self.state().await == LlmRuntimeState::Running)
    }

    pub async fn wait_until_idle(&self, timeout: Duration) -> anyhow::Result<bool> {
        let Some((_, config)) =
            crate::config::get_native_runtime_config(self.profile_id.as_deref())
        else {
            return Ok(true);
        };

        if !config.enabled {
            return Ok(true);
        }

        let Some(slots_url) = slots_url_from_base_url(&config.base_url) else {
            return Ok(true);
        };

        let client = reqwest::Client::new();
        let deadline = Instant::now() + timeout;
        loop {
            if native_runtime_slots_idle(&client, &slots_url).await? {
                return Ok(true);
            }

            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(false);
            }
            tokio::time::sleep(remaining.min(LLM_IDLE_POLL_INTERVAL)).await;
        }
    }

    pub async fn start(&self) -> anyhow::Result<()> {
        crate::ensure_native_runtime_for_cli(self.profile_id.as_deref()).await
    }

    pub async fn stop(&self) -> anyhow::Result<()> {
        let Some((profile_id, config)) =
            crate::config::get_native_runtime_config(self.profile_id.as_deref())
        else {
            return Ok(());
        };
        crate::stop_native_runtime_server_for_config(&profile_id, &config).await
    }

    pub async fn restart(&self) -> anyhow::Result<()> {
        self.stop().await?;
        self.start().await
    }
}

#[derive(Debug, Deserialize)]
struct LlamaSlotStatus {
    #[serde(default)]
    is_processing: bool,
}

async fn native_runtime_slots_idle(
    client: &reqwest::Client,
    slots_url: &str,
) -> anyhow::Result<bool> {
    let response = match client
        .get(slots_url)
        .timeout(Duration::from_secs(2))
        .send()
        .await
    {
        Ok(response) => response,
        Err(_) => return Ok(true),
    };
    if !response.status().is_success() {
        return Ok(true);
    }
    let slots = response.json::<Vec<LlamaSlotStatus>>().await?;
    Ok(slots.iter().all(|slot| !slot.is_processing))
}

fn slots_url_from_base_url(base_url: &str) -> Option<String> {
    let base_url = base_url.trim();
    if base_url.is_empty() {
        return None;
    }

    let mut url = url::Url::parse(base_url).ok()?;
    let mut path = url.path().trim_end_matches('/').to_string();
    if path == "/v1" {
        path.clear();
    } else if let Some(stripped) = path.strip_suffix("/v1") {
        path = stripped.to_string();
    }
    path.push_str("/slots");
    url.set_path(&path);
    url.set_query(None);
    url.set_fragment(None);
    Some(url.to_string())
}

#[cfg(test)]
mod tests {
    use super::slots_url_from_base_url;

    #[test]
    fn slots_url_removes_openai_v1_suffix() {
        assert_eq!(
            slots_url_from_base_url("http://127.0.0.1:8082/v1").as_deref(),
            Some("http://127.0.0.1:8082/slots")
        );
        assert_eq!(
            slots_url_from_base_url("http://127.0.0.1:8082/proxy/v1/").as_deref(),
            Some("http://127.0.0.1:8082/proxy/slots")
        );
    }
}
