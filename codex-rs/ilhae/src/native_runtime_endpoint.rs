use crate::config::IlhaeProfileNativeRuntimeConfig;
use std::net::IpAddr;

pub(crate) const DEFAULT_LOCAL_PROXY_URL: &str = "http://127.0.0.1:8083";

pub(crate) fn effective_proxy_url(config: &IlhaeProfileNativeRuntimeConfig) -> Option<String> {
    if config.proxy_bypass {
        return None;
    }
    config
        .proxy_url
        .as_deref()
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .map(|url| url.trim_end_matches('/').to_string())
        .or_else(|| uses_runtime_proxy(config).then(|| DEFAULT_LOCAL_PROXY_URL.to_string()))
}

pub(crate) fn uses_runtime_proxy(config: &IlhaeProfileNativeRuntimeConfig) -> bool {
    if config.proxy_bypass {
        return false;
    }
    config
        .proxy_url
        .as_deref()
        .is_some_and(|url| !url.trim().is_empty())
        || config
            .provider
            .as_deref()
            .is_some_and(|provider| provider.trim().eq_ignore_ascii_case("llama-server"))
}

pub(crate) fn effective_proxy_base_url(config: &IlhaeProfileNativeRuntimeConfig) -> Option<String> {
    effective_proxy_url(config).map(|url| format!("{url}/v1"))
}

pub(crate) fn effective_proxy_health_url(
    config: &IlhaeProfileNativeRuntimeConfig,
) -> Option<String> {
    effective_proxy_url(config).map(|url| format!("{url}/health"))
}

pub(crate) fn effective_proxy_control_url(
    config: &IlhaeProfileNativeRuntimeConfig,
) -> Option<String> {
    effective_proxy_url(config).map(|url| format!("{url}/_ilhae/native-runtime/ensure"))
}

pub(crate) fn validate_proxy_transport(
    config: &IlhaeProfileNativeRuntimeConfig,
) -> anyhow::Result<()> {
    let Some(proxy_url) = effective_proxy_url(config) else {
        return Ok(());
    };
    let parsed = url::Url::parse(&proxy_url)?;
    if !parsed.username().is_empty() || parsed.password().is_some() {
        anyhow::bail!("native runtime proxy_url must not contain credentials");
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        anyhow::bail!("native runtime proxy_url must not contain a query or fragment");
    }
    if !matches!(parsed.path(), "" | "/") {
        anyhow::bail!("native runtime proxy_url must be an origin without a path");
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("native runtime proxy_url must include a host"))?;
    let is_loopback = host.eq_ignore_ascii_case("localhost")
        || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback());
    match parsed.scheme() {
        "https" => {}
        "http" if is_loopback => {}
        "http" if config.proxy_allow_insecure_http => {}
        "http" => anyhow::bail!(
            "remote native runtime proxy_url must use HTTPS unless proxy_allow_insecure_http = true"
        ),
        scheme => anyhow::bail!("native runtime proxy_url uses unsupported scheme `{scheme}`"),
    }
    if !is_loopback
        && !config
            .proxy_token
            .as_deref()
            .is_some_and(|token| !token.trim().is_empty())
    {
        anyhow::bail!("proxy_token is required for a remote native runtime proxy");
    }
    Ok(())
}

pub(crate) fn runtime_upstream_base_url(
    config: &IlhaeProfileNativeRuntimeConfig,
) -> Option<String> {
    let base_url = config.base_url.trim();
    if !base_url.is_empty() {
        return Some(base_url.to_string());
    }
    let base_url = config.url.as_ref().map(|url| url.trim());
    if let Some(base_url) = base_url.filter(|base_url| !base_url.is_empty()) {
        return Some(base_url.to_string());
    }
    runtime_base_url_from_args(config)
}

pub(crate) fn runtime_upstream_health_url(config: &IlhaeProfileNativeRuntimeConfig) -> String {
    let health_url = config.health_url.trim();
    if !health_url.is_empty() {
        return health_url.to_string();
    }
    let base_url = runtime_upstream_base_url(config).unwrap_or_default();
    crate::config::native_runtime_health_url_from_base_url(&base_url)
}

pub(crate) fn runtime_base_url_from_args(
    config: &IlhaeProfileNativeRuntimeConfig,
) -> Option<String> {
    let port = runtime_port_from_args(config)?;
    let mut host = None;
    let mut args = config.args.iter();
    while let Some(arg) = args.next() {
        if arg == "--host" {
            host = args.next().map(String::as_str);
        } else if let Some(value) = arg.strip_prefix("--host=") {
            host = Some(value);
        }
    }
    let host = host
        .unwrap_or("127.0.0.1")
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']');
    let host = if host == "*" || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_unspecified()) {
        "127.0.0.1".to_string()
    } else {
        match host.parse::<IpAddr>() {
            Ok(IpAddr::V6(address)) => format!("[{address}]"),
            _ if host.is_empty() => "127.0.0.1".to_string(),
            _ => host.to_string(),
        }
    };
    Some(format!("http://{host}:{port}/v1"))
}

pub(crate) fn runtime_port_from_args(config: &IlhaeProfileNativeRuntimeConfig) -> Option<u16> {
    let mut port = None;
    let mut args = config.args.iter();
    while let Some(arg) = args.next() {
        if matches!(arg.as_str(), "--port" | "-p") {
            port = args
                .next()
                .and_then(|value| value.parse::<u16>().ok())
                .filter(|port| *port > 0);
        } else if let Some(value) = arg
            .strip_prefix("--port=")
            .or_else(|| arg.strip_prefix("-p="))
        {
            port = value.parse::<u16>().ok().filter(|port| *port > 0);
        }
    }
    port
}

#[cfg(test)]
#[path = "native_runtime_endpoint_tests.rs"]
mod tests;
