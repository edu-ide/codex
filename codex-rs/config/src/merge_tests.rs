use super::*;
use crate::config_toml::ConfigToml;
use crate::types::MemoriesToml;
use pretty_assertions::assert_eq;

fn parse_toml(value: &str) -> TomlValue {
    toml::from_str(value).expect("TOML should parse")
}

#[test]
fn merge_toml_values_normalizes_legacy_key_from_base_layer() {
    let mut base = parse_toml(
        r#"
[memories]
no_memories_if_mcp_or_web_search = false
"#,
    );
    let overlay = parse_toml(
        r#"
[memories]
disable_on_external_context = true
"#,
    );

    merge_toml_values(&mut base, &overlay);

    let expected = parse_toml(
        r#"
[memories]
disable_on_external_context = true
"#,
    );
    assert_eq!(base, expected);

    let config: ConfigToml = base.try_into().expect("merged config should deserialize");
    assert_eq!(
        config.memories,
        Some(MemoriesToml {
            disable_on_external_context: Some(true),
            ..Default::default()
        })
    );
}

#[test]
fn merge_toml_values_normalizes_legacy_key_from_overlay_layer() {
    let mut base = parse_toml(
        r#"
[memories]
disable_on_external_context = false
"#,
    );
    let overlay = parse_toml(
        r#"
[memories]
no_memories_if_mcp_or_web_search = true
"#,
    );

    merge_toml_values(&mut base, &overlay);

    let expected = parse_toml(
        r#"
[memories]
disable_on_external_context = true
"#,
    );
    assert_eq!(base, expected);

    let config: ConfigToml = base.try_into().expect("merged config should deserialize");
    assert_eq!(
        config.memories,
        Some(MemoriesToml {
            disable_on_external_context: Some(true),
            ..Default::default()
        })
    );
}

#[test]
fn merge_toml_values_prefers_canonical_key_when_one_layer_has_both_names() {
    let mut base = TomlValue::Table(toml::map::Map::new());
    let overlay = parse_toml(
        r#"
[memories]
disable_on_external_context = true
no_memories_if_mcp_or_web_search = false
"#,
    );

    merge_toml_values(&mut base, &overlay);

    let expected = parse_toml(
        r#"
[memories]
disable_on_external_context = true
"#,
    );
    assert_eq!(base, expected);
}

#[test]
fn merge_toml_values_normalizes_permission_network_domains_before_overlaying() {
    let mut base = parse_toml(
        r#"
[permissions.dev.network.domains]
"example.com" = "deny"
"#,
    );
    let overlay = parse_toml(
        r#"
[permissions.dev.network.domains]
"EXAMPLE.COM" = "allow"
"#,
    );

    merge_toml_values(&mut base, &overlay);

    let expected = parse_toml(
        r#"
[permissions.dev.network.domains]
"example.com" = "allow"
"#,
    );
    assert_eq!(base, expected);
}

#[test]
fn merge_toml_values_switches_mcp_transport_from_stdio_to_http_atomically() {
    let mut base = parse_toml(
        r#"
[mcp_servers.excel]
command = "legacy-excel"
args = ["serve"]
env = { MODE = "stdio" }
cwd = "/tmp/legacy"
enabled = false
"#,
    );
    let overlay = parse_toml(
        r#"
[mcp_servers.excel]
url = "http://127.0.0.1:8011/mcp"
"#,
    );

    merge_toml_values(&mut base, &overlay);

    let expected = parse_toml(
        r#"
[mcp_servers.excel]
url = "http://127.0.0.1:8011/mcp"
enabled = false
"#,
    );
    assert_eq!(base, expected);
}

#[test]
fn merge_toml_values_switches_mcp_transport_from_http_to_stdio_atomically() {
    let mut base = parse_toml(
        r#"
[mcp_servers.excel]
url = "http://127.0.0.1:8011/mcp"
bearer_token_env_var = "EXCEL_TOKEN"
http_headers = { X-Mode = "http" }
oauth_resource = "excel"
enabled = true
"#,
    );
    let overlay = parse_toml(
        r#"
[mcp_servers.excel]
command = "excel-mcp"
args = ["serve"]
"#,
    );

    merge_toml_values(&mut base, &overlay);

    let expected = parse_toml(
        r#"
[mcp_servers.excel]
command = "excel-mcp"
args = ["serve"]
enabled = true
"#,
    );
    assert_eq!(base, expected);
}

#[test]
fn merge_toml_values_keeps_mcp_transport_for_shared_field_override() {
    let mut base = parse_toml(
        r#"
[mcp_servers.excel]
url = "http://127.0.0.1:8011/mcp"
enabled = true
"#,
    );
    let overlay = parse_toml(
        r#"
[mcp_servers.excel]
enabled = false
"#,
    );

    merge_toml_values(&mut base, &overlay);

    let expected = parse_toml(
        r#"
[mcp_servers.excel]
url = "http://127.0.0.1:8011/mcp"
enabled = false
"#,
    );
    assert_eq!(base, expected);
}

#[test]
fn merge_toml_values_does_not_hide_invalid_mcp_transport_in_one_layer() {
    let mut base = parse_toml(
        r#"
[mcp_servers.excel]
command = "legacy-excel"
"#,
    );
    let overlay = parse_toml(
        r#"
[mcp_servers.excel]
command = "excel-mcp"
url = "http://127.0.0.1:8011/mcp"
"#,
    );

    merge_toml_values(&mut base, &overlay);

    let excel = base
        .get("mcp_servers")
        .and_then(TomlValue::as_table)
        .and_then(|servers| servers.get("excel"))
        .and_then(TomlValue::as_table)
        .expect("merged Excel MCP config");
    assert!(excel.contains_key("command"));
    assert!(excel.contains_key("url"));
}
