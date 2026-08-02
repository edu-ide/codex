use crate::key_aliases::normalize_key_aliases;
use crate::key_aliases::normalized_with_key_aliases;
use codex_network_proxy::normalize_host;
use toml::Value as TomlValue;

/// Merge config `overlay` into `base`, giving `overlay` precedence.
pub fn merge_toml_values(base: &mut TomlValue, overlay: &TomlValue) {
    merge_toml_values_at_path(base, overlay, &mut Vec::new());
}

fn merge_toml_values_at_path(base: &mut TomlValue, overlay: &TomlValue, path: &mut Vec<String>) {
    if let TomlValue::Table(overlay_table) = overlay
        && let TomlValue::Table(base_table) = base
    {
        normalize_key_aliases(path, base_table);
        let mut overlay_table = overlay_table.clone();
        normalize_key_aliases(path, &mut overlay_table);
        if is_permission_network_domains_path(path) {
            normalize_network_domain_keys(base_table);
            normalize_network_domain_keys(&mut overlay_table);
        }
        normalize_mcp_transport_boundary(path, base_table, &overlay_table);

        for (key, value) in overlay_table {
            path.push(key.clone());
            if let Some(existing) = base_table.get_mut(&key) {
                merge_toml_values_at_path(existing, &value, path);
            } else {
                base_table.insert(key, normalized_with_key_aliases(&value, path));
            }
            path.pop();
        }
    } else {
        *base = normalized_with_key_aliases(overlay, path);
    }
}

/// MCP transport is a discriminated choice, not a set of independently
/// mergeable fields. When a higher-precedence layer selects a transport,
/// discard fields that belong exclusively to the other transport before the
/// ordinary recursive merge. Shared settings such as `enabled` can still be
/// overridden without restating the transport.
///
/// A single layer that declares both selectors is intentionally left intact so
/// deserialization reports the invalid source configuration instead of hiding
/// it.
fn normalize_mcp_transport_boundary(
    path: &[String],
    base_table: &mut toml::map::Map<String, TomlValue>,
    overlay_table: &toml::map::Map<String, TomlValue>,
) {
    if !matches!(path, [mcp_servers, _] if mcp_servers == "mcp_servers") {
        return;
    }

    let selects_stdio = overlay_table.contains_key("command");
    let selects_http = overlay_table.contains_key("url");
    if selects_stdio == selects_http {
        return;
    }

    let incompatible_fields: &[&str] = if selects_stdio {
        &[
            "url",
            "bearer_token",
            "bearer_token_env_var",
            "http_headers",
            "env_http_headers",
            "oauth",
            "oauth_resource",
            "auth",
        ]
    } else {
        &["command", "args", "env", "env_vars", "cwd"]
    };
    for field in incompatible_fields {
        base_table.remove(*field);
    }
}

fn is_permission_network_domains_path(path: &[String]) -> bool {
    matches!(
        path,
        [permissions, _, network, domains]
            if permissions == "permissions" && network == "network" && domains == "domains"
    )
}

fn normalize_network_domain_keys(table: &mut toml::map::Map<String, TomlValue>) {
    let entries = std::mem::take(table);
    for (pattern, value) in entries {
        table.insert(normalize_host(&pattern), value);
    }
}

#[cfg(test)]
#[path = "merge_tests.rs"]
mod tests;
