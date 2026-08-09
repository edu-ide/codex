use super::EnsureNativeRuntimeRequest;
use super::EnsureNativeRuntimeResponse;
use super::StopNativeRuntimeRequest;
use super::StopNativeRuntimeResponse;
use crate::config::IlhaeProfileNativeRuntimeConfig;
use std::time::Duration;

const RUNTIME_TOKEN_HEADER: &str = "X-Ilhae-Runtime-Token";

pub async fn ensure_remote_native_runtime(
    profile_id: &str,
    config: &IlhaeProfileNativeRuntimeConfig,
) -> anyhow::Result<EnsureNativeRuntimeResponse> {
    crate::native_runtime_endpoint::validate_proxy_transport(config)?;
    let control_url = crate::native_runtime_endpoint::effective_proxy_control_url(config)
        .ok_or_else(|| anyhow::anyhow!("native runtime proxy_url is required"))?;
    let mut request =
        reqwest::Client::builder()
            .build()?
            .post(&control_url)
            .json(&EnsureNativeRuntimeRequest {
                profile_id: profile_id.to_string(),
                thinking_mode: crate::config::current_thinking_mode(),
                config: config.clone(),
            });
    if let Some(token) = proxy_token(config) {
        request = request.header(RUNTIME_TOKEN_HEADER, token);
    }

    let response = request.send().await?;
    let status = response.status();
    let body = response.bytes().await?;
    if !status.is_success() {
        let message = serde_json::from_slice::<serde_json::Value>(&body)
            .ok()
            .and_then(|body| {
                body.get("error")
                    .and_then(|error| error.as_str())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| String::from_utf8_lossy(&body).into_owned());
        anyhow::bail!("native runtime controller returned {status}: {message}");
    }

    serde_json::from_slice(&body).map_err(Into::into)
}

pub async fn stop_remote_native_runtime(
    profile_id: &str,
    config: &IlhaeProfileNativeRuntimeConfig,
) -> anyhow::Result<StopNativeRuntimeResponse> {
    crate::native_runtime_endpoint::validate_proxy_transport(config)?;
    let ensure_url = crate::native_runtime_endpoint::effective_proxy_control_url(config)
        .ok_or_else(|| anyhow::anyhow!("native runtime proxy_url is required"))?;
    let stop_url = control_action_url(&ensure_url, "stop")?;
    let mut request = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?
        .post(stop_url)
        .json(&StopNativeRuntimeRequest {
            profile_id: profile_id.to_string(),
            config: config.clone(),
        });
    if let Some(token) = proxy_token(config) {
        request = request.header(RUNTIME_TOKEN_HEADER, token);
    }

    let response = request.send().await?;
    let status = response.status();
    let body = response.bytes().await?;
    if !status.is_success() {
        let message = serde_json::from_slice::<serde_json::Value>(&body)
            .ok()
            .and_then(|body| {
                body.get("error")
                    .and_then(|error| error.as_str())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| String::from_utf8_lossy(&body).into_owned());
        anyhow::bail!("native runtime controller returned {status}: {message}");
    }

    serde_json::from_slice(&body).map_err(Into::into)
}

fn proxy_token(config: &IlhaeProfileNativeRuntimeConfig) -> Option<&str> {
    config
        .proxy_token
        .as_deref()
        .map(str::trim)
        .filter(|token| !token.is_empty())
}

fn control_action_url(control_url: &str, action: &str) -> anyhow::Result<url::Url> {
    let mut url = url::Url::parse(control_url)?;
    let is_ensure_endpoint = url
        .path_segments()
        .and_then(Iterator::last)
        .is_some_and(|segment| segment == "ensure");
    if !is_ensure_endpoint {
        anyhow::bail!("native runtime proxy control URL must end with `/ensure`");
    }
    {
        let mut segments = url.path_segments_mut().map_err(|_| {
            anyhow::anyhow!("native runtime proxy control URL cannot be a base URL")
        })?;
        segments.pop_if_empty();
        segments.pop().push(action);
    }
    Ok(url)
}

#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;
