//! Project and validate MCP transports at the Codex boundary.

use std::path::Path;
use tracing::warn;
#[path = "../../../../../../../../tools/desktop/native_mcp_launchers.rs"]
pub(super) mod native_mcp_launchers;
use super::RETIRED_EXCEL_MCP_SERVER_NAME;
use super::projection::user_table_for_managed_config;

pub(super) fn user_mcp_servers_for_managed_config(user_config: &toml::Value) -> toml::value::Table {
    let mut servers = toml::value::Table::new();
    let mut excluded_invalid_count = 0usize;
    for (name, server) in user_table_for_managed_config(user_config, "mcp_servers") {
        // Explicit Office desktop/remote transports are user intent too. Only
        // the retired Excel alias is excluded; no human source is rewritten.
        if name == RETIRED_EXCEL_MCP_SERVER_NAME {
            continue;
        }
        match server.clone().try_into::<codex_config::McpServerConfig>() {
            Ok(config) if mcp_server_transport_is_semantically_valid(&config) => {
                servers.insert(name, server);
            }
            Ok(_) | Err(_) => excluded_invalid_count += 1,
        }
    }
    if excluded_invalid_count > 0 {
        warn!(
            "Excluded {excluded_invalid_count} invalid MCP configuration entries from the Codex runtime snapshot"
        );
    }

    disable_duplicate_legacy_fortune_mcp_server(&mut servers);
    servers
}

pub(super) fn native_mcp_defaults(servers: &mut toml::value::Table, user_config: &toml::Value) {
    let configured = user_config
        .get("mcp_servers")
        .and_then(toml::Value::as_table);
    for (name, command, aliases) in [
        ("brain", "brain", &["brain-mcp", "mcpb_brain"][..]),
        ("office", "ugot-office-mcp", &["mcpb_office"][..]),
        (
            "browser",
            "ugot-browser",
            &["agent-browser", "ugot-browser", "browser-bridge-mcp"][..],
        ),
        (
            "email",
            "email",
            &[
                "mail",
                "mail-mcp",
                "ugot-mail",
                "mcpb_mail_mcp",
                "mcpb_mail-mcp",
            ][..],
        ),
    ] {
        if servers.contains_key(name) {
            continue;
        }
        if configured.is_some_and(|original| {
            original.contains_key(name)
                || aliases
                    .iter()
                    .any(|alias| original.contains_key(*alias) && !servers.contains_key(*alias))
        }) {
            // Invalid explicit configuration must not turn into a different
            // default account after validation excludes that entry.
            continue;
        }
        let configured_aliases: Vec<_> = aliases
            .iter()
            .filter(|alias| servers.contains_key(**alias))
            .collect();
        if configured_aliases.len() == 1 {
            let server = servers
                .remove(*configured_aliases[0])
                .expect("configured alias");
            servers.insert(name.to_owned(), server);
            continue;
        }
        if !configured_aliases.is_empty() {
            // Multiple configured services may represent different accounts.
            // Leave them untouched rather than pick or launch a default profile.
            continue;
        }
        let mut server = toml::value::Table::new();
        let Some(executable) =
            native_mcp_launchers::resolve_command(command, Path::new(env!("CARGO_MANIFEST_DIR")))
        else {
            warn!(
                "Native {command} launcher was not found; configure mcp_servers.{name}.command or install its native executable"
            );
            continue;
        };
        server.insert("command".to_owned(), executable.into());
        server.insert("args".to_owned(), toml::Value::Array(vec!["mcp".into()]));
        server.insert(
            "env_vars".to_owned(),
            toml::Value::Array(
                native_mcp_launchers::env_vars(command)
                    .into_iter()
                    .map(toml::Value::from)
                    .collect(),
            ),
        );
        servers.insert(name.to_owned(), server.into());
    }
}

pub(super) fn mcp_server_transport_is_semantically_valid(
    config: &codex_config::McpServerConfig,
) -> bool {
    match &config.transport {
        codex_config::McpServerTransportConfig::Stdio { command, .. } => !command.trim().is_empty(),
        codex_config::McpServerTransportConfig::StreamableHttp { url, .. } => {
            if url.is_empty() || url.trim() != url {
                return false;
            }
            let Ok(parsed) = url::Url::parse(url) else {
                return false;
            };
            matches!(parsed.scheme(), "http" | "https") && parsed.host_str().is_some()
        }
    }
}

pub(super) fn is_fortune_mcp_server(value: &toml::Value) -> bool {
    let Some(table) = value.as_table() else {
        return false;
    };
    table
        .get("url")
        .and_then(toml::Value::as_str)
        .is_some_and(|url| url.trim_end_matches('/') == "https://fortune.ugot.uk/mcp")
        || table
            .get("oauth_resource")
            .and_then(toml::Value::as_str)
            .is_some_and(|url| url.trim_end_matches('/') == "https://fortune.ugot.uk/mcp")
}

pub(super) fn disable_duplicate_legacy_fortune_mcp_server(servers: &mut toml::value::Table) {
    let has_canonical_fortune = servers
        .get("ugot_fortune")
        .is_some_and(is_fortune_mcp_server);
    let has_legacy_fortune = servers.get("fortune").is_some_and(is_fortune_mcp_server);

    if has_canonical_fortune
        && has_legacy_fortune
        && let Some(table) = servers
            .get_mut("fortune")
            .and_then(toml::Value::as_table_mut)
    {
        table.insert("enabled".to_string(), toml::Value::Boolean(false));
    }
}
