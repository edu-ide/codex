//! OSS provider utilities shared between TUI and exec.

use codex_core::LLAMA_SERVER_OSS_PROVIDER_ID;
use codex_core::LMSTUDIO_OSS_PROVIDER_ID;
use codex_core::OLLAMA_OSS_PROVIDER_ID;
use codex_core::config::Config;
use std::io;
use std::time::Duration;

pub const DEFAULT_LLAMA_SERVER_MODEL: &str = "local-model";

/// Returns the default model for a given OSS provider.
pub fn get_default_model_for_oss_provider(provider_id: &str) -> Option<&'static str> {
    match provider_id {
        LMSTUDIO_OSS_PROVIDER_ID => Some(codex_lmstudio::DEFAULT_OSS_MODEL),
        OLLAMA_OSS_PROVIDER_ID => Some(codex_ollama::DEFAULT_OSS_MODEL),
        LLAMA_SERVER_OSS_PROVIDER_ID => Some(DEFAULT_LLAMA_SERVER_MODEL),
        _ => None,
    }
}

/// Ensures the specified OSS provider is ready (models downloaded, service reachable).
pub async fn ensure_oss_provider_ready(
    provider_id: &str,
    config: &Config,
) -> Result<(), std::io::Error> {
    match provider_id {
        LMSTUDIO_OSS_PROVIDER_ID => {
            codex_lmstudio::ensure_oss_ready(config)
                .await
                .map_err(|e| std::io::Error::other(format!("OSS setup failed: {e}")))?;
        }
        OLLAMA_OSS_PROVIDER_ID => {
            codex_ollama::ensure_responses_supported(&config.model_provider).await?;
            codex_ollama::ensure_oss_ready(config)
                .await
                .map_err(|e| std::io::Error::other(format!("OSS setup failed: {e}")))?;
        }
        LLAMA_SERVER_OSS_PROVIDER_ID => {
            ensure_llama_server_ready(config).await?;
        }
        _ => {
            // Unknown provider, skip setup
        }
    }
    Ok(())
}

async fn ensure_llama_server_ready(config: &Config) -> io::Result<()> {
    let base_url = config
        .model_provider
        .base_url
        .as_ref()
        .ok_or_else(|| io::Error::other("llama-server provider must have a base_url"))?;
    let host_root = base_url
        .strip_suffix("/v1")
        .unwrap_or(base_url)
        .trim_end_matches('/');
    let models_url = format!("{host_root}/v1/models");
    let health_url = format!("{host_root}/health");

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .map_err(io::Error::other)?;

    for url in [&health_url, &models_url] {
        match client.get(url).send().await {
            Ok(response) if response.status().is_success() => return Ok(()),
            Ok(_) | Err(_) => {}
        }
    }

    Err(io::Error::other(format!(
        "No running llama-server detected. Start llama.cpp with an OpenAI-compatible server on {host_root} (default port 8080)."
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_default_model_for_provider_lmstudio() {
        let result = get_default_model_for_oss_provider(LMSTUDIO_OSS_PROVIDER_ID);
        assert_eq!(result, Some(codex_lmstudio::DEFAULT_OSS_MODEL));
    }

    #[test]
    fn test_get_default_model_for_provider_ollama() {
        let result = get_default_model_for_oss_provider(OLLAMA_OSS_PROVIDER_ID);
        assert_eq!(result, Some(codex_ollama::DEFAULT_OSS_MODEL));
    }

    #[test]
    fn test_get_default_model_for_provider_llama_server() {
        let result = get_default_model_for_oss_provider(LLAMA_SERVER_OSS_PROVIDER_ID);
        assert_eq!(result, Some(DEFAULT_LLAMA_SERVER_MODEL));
    }

    #[test]
    fn test_get_default_model_for_provider_unknown() {
        let result = get_default_model_for_oss_provider("unknown-provider");
        assert_eq!(result, None);
    }
}
