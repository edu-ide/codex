use super::EnsureNativeRuntimeRequest;
use super::EnsureNativeRuntimeResponse;
use super::StopNativeRuntimeRequest;
use super::StopNativeRuntimeResponse;
use crate::config::IlhaeProfileNativeRuntimeConfig;
use std::process::Stdio;
use std::time::Duration;

const RUNTIME_TOKEN_HEADER: &str = "X-Ilhae-Runtime-Token";

pub async fn ensure_remote_native_runtime(
    profile_id: &str,
    config: &IlhaeProfileNativeRuntimeConfig,
) -> anyhow::Result<EnsureNativeRuntimeResponse> {
    ensure_ssh_tunnel(config).await?;
    let control_url = crate::native_runtime_endpoint::effective_proxy_control_url(config)
        .ok_or_else(|| anyhow::anyhow!("native runtime proxy_control_url is required"))?;
    let mut request =
        reqwest::Client::builder()
            .build()?
            .post(&control_url)
            .json(&EnsureNativeRuntimeRequest {
                profile_id: profile_id.to_string(),
                thinking_mode: crate::config::current_thinking_mode(),
                config: config.clone(),
            });
    if let Some(token) = proxy_token(config)? {
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
        anyhow::bail!("remote native runtime controller returned {status}: {message}");
    }

    serde_json::from_slice(&body).map_err(Into::into)
}

pub async fn stop_remote_native_runtime(
    profile_id: &str,
    config: &IlhaeProfileNativeRuntimeConfig,
) -> anyhow::Result<StopNativeRuntimeResponse> {
    ensure_ssh_tunnel(config).await?;
    let ensure_url = crate::native_runtime_endpoint::effective_proxy_control_url(config)
        .ok_or_else(|| anyhow::anyhow!("native runtime proxy_control_url is required"))?;
    let stop_url = control_action_url(&ensure_url, "stop")?;
    let mut request = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?
        .post(stop_url)
        .json(&StopNativeRuntimeRequest {
            profile_id: profile_id.to_string(),
            config: config.clone(),
        });
    if let Some(token) = proxy_token(config)? {
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
        anyhow::bail!("remote native runtime controller returned {status}: {message}");
    }

    serde_json::from_slice(&body).map_err(Into::into)
}

fn proxy_token(config: &IlhaeProfileNativeRuntimeConfig) -> anyhow::Result<Option<String>> {
    let Some(token_env) = config
        .proxy_control_token_env
        .as_deref()
        .map(str::trim)
        .filter(|token_env| !token_env.is_empty())
    else {
        return Ok(None);
    };
    let token = std::env::var(token_env).map_err(|_| {
        anyhow::anyhow!("native runtime proxy token environment variable `{token_env}` is not set")
    })?;
    if token.is_empty() {
        anyhow::bail!("native runtime proxy token environment variable `{token_env}` is empty");
    }
    Ok(Some(token))
}

async fn ensure_ssh_tunnel(config: &IlhaeProfileNativeRuntimeConfig) -> anyhow::Result<()> {
    let Some(ssh_host) = crate::native_runtime_endpoint::ssh_host(config) else {
        return Ok(());
    };
    if ssh_host.starts_with('-')
        || ssh_host
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
    {
        anyhow::bail!("native runtime ssh_host must be an OpenSSH host alias or destination");
    }

    let control_url = crate::native_runtime_endpoint::effective_proxy_control_url(config)
        .ok_or_else(|| anyhow::anyhow!("native runtime proxy control URL could not be derived"))?;
    let mut health_url = url::Url::parse(&control_url)?;
    health_url.set_path("/_ilhae/health");
    health_url.set_query(None);
    health_url.set_fragment(None);
    if proxy_is_ready(config, &health_url).await? {
        return Ok(());
    }

    let local_port = crate::native_runtime_endpoint::ssh_local_port(config);
    let remote_port = crate::native_runtime_endpoint::ssh_remote_port(config);
    if local_port == 0 || remote_port == 0 {
        anyhow::bail!("native runtime SSH forwarding ports must be greater than zero");
    }
    let forward = format!("127.0.0.1:{local_port}:127.0.0.1:{remote_port}");
    let output = tokio::process::Command::new("ssh")
        .args([
            "-f",
            "-N",
            "-o",
            "BatchMode=yes",
            "-o",
            "ExitOnForwardFailure=yes",
            "-o",
            "ConnectTimeout=10",
            "-L",
            &forward,
            ssh_host,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|error| {
            anyhow::anyhow!("failed to start OpenSSH tunnel for `{ssh_host}`: {error}")
        })?;

    if !output.status.success() && !proxy_is_ready(config, &health_url).await? {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr = stderr.trim().chars().take(4_000).collect::<String>();
        anyhow::bail!(
            "OpenSSH tunnel for `{ssh_host}` failed with {}: {stderr}",
            output.status
        );
    }

    for _ in 0..50 {
        if proxy_is_ready(config, &health_url).await? {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    anyhow::bail!(
        "OpenSSH tunnel for `{ssh_host}` started but the Ilhae proxy did not become ready"
    )
}

async fn proxy_is_ready(
    config: &IlhaeProfileNativeRuntimeConfig,
    health_url: &url::Url,
) -> anyhow::Result<bool> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(1))
        .build()?;
    let mut request = client.get(health_url.clone());
    if let Some(token) = proxy_token(config)? {
        request = request.header(RUNTIME_TOKEN_HEADER, token);
    }
    let response = match request.send().await {
        Ok(response) => response,
        Err(error) if error.is_connect() || error.is_timeout() => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    let status = response.status();
    let body = response.bytes().await?;
    if !status.is_success() {
        anyhow::bail!("local SSH forwarding port responded with HTTP {status}");
    }
    let is_ilhae_proxy = serde_json::from_slice::<serde_json::Value>(&body)
        .ok()
        .and_then(|body| {
            body.get("status")
                .and_then(|status| status.as_str())
                .map(str::to_string)
        })
        .is_some_and(|status| status == "ok");
    if !is_ilhae_proxy {
        anyhow::bail!("local SSH forwarding port is occupied by a service other than Ilhae proxy");
    }
    Ok(true)
}

fn control_action_url(control_url: &str, action: &str) -> anyhow::Result<url::Url> {
    let mut url = url::Url::parse(control_url)?;
    let is_ensure_endpoint = url
        .path_segments()
        .and_then(Iterator::last)
        .is_some_and(|segment| segment == "ensure");
    if !is_ensure_endpoint {
        anyhow::bail!("native runtime proxy_control_url must end with `/ensure`");
    }
    {
        let mut segments = url.path_segments_mut().map_err(|_| {
            anyhow::anyhow!("native runtime proxy_control_url cannot be a base URL")
        })?;
        segments.pop_if_empty();
        segments.pop().push(action);
    }
    Ok(url)
}

#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;
