//! OSS provider utilities shared between TUI and exec.

use codex_core::LLAMA_SERVER_OSS_PROVIDER_ID;
use codex_core::LMSTUDIO_OSS_PROVIDER_ID;
use codex_core::OLLAMA_OSS_PROVIDER_ID;
use codex_core::config::Config;
use serde::Deserialize;
use std::io;
use std::time::Duration;

pub const DEFAULT_LLAMA_SERVER_MODEL: &str = "local-model";

#[derive(Debug, Deserialize)]
struct LlamaServerModelsResponse {
    #[serde(default)]
    data: Vec<LlamaServerModelEntry>,
    #[serde(default)]
    models: Vec<LlamaServerModelEntry>,
}

#[derive(Debug, Deserialize)]
struct LlamaServerModelEntry {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    name: Option<String>,
}

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

pub async fn hydrate_oss_model_name(config: &mut Config) -> io::Result<()> {
    if config.model_provider_id != LLAMA_SERVER_OSS_PROVIDER_ID {
        return Ok(());
    }

    if !matches!(
        config.model.as_deref(),
        None | Some(DEFAULT_LLAMA_SERVER_MODEL)
    ) {
        return Ok(());
    }

    if let Some(model) = discover_llama_server_model(config).await? {
        config.model = Some(model);
    }

    Ok(())
}

async fn ensure_llama_server_ready(config: &Config) -> io::Result<()> {
    let (host_root, health_url, models_url) = llama_server_urls(config)?;

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

async fn discover_llama_server_model(config: &Config) -> io::Result<Option<String>> {
    let (_, _, models_url) = llama_server_urls(config)?;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .map_err(io::Error::other)?;

    let response = client
        .get(models_url)
        .send()
        .await
        .map_err(io::Error::other)?;

    if !response.status().is_success() {
        return Ok(None);
    }

    let models = response
        .json::<LlamaServerModelsResponse>()
        .await
        .map_err(io::Error::other)?;

    Ok(extract_first_llama_server_model(&models))
}

fn llama_server_urls(config: &Config) -> io::Result<(String, String, String)> {
    let base_url = config
        .model_provider
        .base_url
        .as_ref()
        .ok_or_else(|| io::Error::other("llama-server provider must have a base_url"))?;
    let host_root = base_url
        .strip_suffix("/v1")
        .unwrap_or(base_url)
        .trim_end_matches('/')
        .to_string();
    let health_url = format!("{host_root}/health");
    let models_url = format!("{host_root}/v1/models");
    Ok((host_root, health_url, models_url))
}

fn extract_first_llama_server_model(models: &LlamaServerModelsResponse) -> Option<String> {
    models
        .data
        .iter()
        .chain(models.models.iter())
        .find_map(|entry| {
            [entry.id.as_deref(), entry.model.as_deref(), entry.name.as_deref()]
                .into_iter()
                .flatten()
                .map(str::trim)
                .find(|value| !value.is_empty())
                .map(ToOwned::to_owned)
        })
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

    #[test]
    fn extract_first_llama_server_model_prefers_openai_data_id() {
        let models = LlamaServerModelsResponse {
            data: vec![LlamaServerModelEntry {
                id: Some("Qwen3.5-27B.Q6_K.gguf".to_string()),
                model: Some("ignored".to_string()),
                name: Some("ignored".to_string()),
            }],
            models: vec![],
        };

        assert_eq!(
            extract_first_llama_server_model(&models).as_deref(),
            Some("Qwen3.5-27B.Q6_K.gguf")
        );
    }

    #[test]
    fn extract_first_llama_server_model_falls_back_to_legacy_models_list() {
        let models = LlamaServerModelsResponse {
            data: vec![],
            models: vec![LlamaServerModelEntry {
                id: None,
                model: Some("legacy-model.gguf".to_string()),
                name: None,
            }],
        };

        assert_eq!(
            extract_first_llama_server_model(&models).as_deref(),
            Some("legacy-model.gguf")
        );
    }
}
