use crate::config::IlhaeProfileNativeRuntimeConfig;
use std::net::IpAddr;

pub(crate) const DEFAULT_SSH_LOCAL_PORT: u16 = 18083;
pub(crate) const DEFAULT_SSH_REMOTE_PORT: u16 = 8083;

pub(crate) fn ssh_host(config: &IlhaeProfileNativeRuntimeConfig) -> Option<&str> {
    config
        .ssh_host
        .as_deref()
        .map(str::trim)
        .filter(|host| !host.is_empty())
}

pub(crate) fn ssh_local_port(config: &IlhaeProfileNativeRuntimeConfig) -> u16 {
    config.ssh_local_port.unwrap_or(DEFAULT_SSH_LOCAL_PORT)
}

pub(crate) fn ssh_remote_port(config: &IlhaeProfileNativeRuntimeConfig) -> u16 {
    config.ssh_remote_port.unwrap_or(DEFAULT_SSH_REMOTE_PORT)
}

pub(crate) fn uses_remote_proxy(config: &IlhaeProfileNativeRuntimeConfig) -> bool {
    ssh_host(config).is_some()
        || config
            .proxy_control_url
            .as_deref()
            .is_some_and(|url| !url.trim().is_empty())
}

pub(crate) fn effective_proxy_base_url(config: &IlhaeProfileNativeRuntimeConfig) -> Option<String> {
    config
        .proxy_base_url
        .as_deref()
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .map(str::to_string)
        .or_else(|| {
            ssh_host(config).map(|_| format!("http://127.0.0.1:{}/v1", ssh_local_port(config)))
        })
}

pub(crate) fn effective_proxy_control_url(
    config: &IlhaeProfileNativeRuntimeConfig,
) -> Option<String> {
    config
        .proxy_control_url
        .as_deref()
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .map(str::to_string)
        .or_else(|| {
            ssh_host(config).map(|_| {
                format!(
                    "http://127.0.0.1:{}/_ilhae/native-runtime/ensure",
                    ssh_local_port(config)
                )
            })
        })
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
