use super::api::LlmRuntimeState;

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

        if crate::startup_main::native_runtime_healthcheck(&config.health_url).await {
            LlmRuntimeState::Running
        } else {
            LlmRuntimeState::Stopped
        }
    }

    pub async fn is_running(&self) -> anyhow::Result<bool> {
        Ok(self.state().await == LlmRuntimeState::Running)
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
