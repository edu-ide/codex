use super::*;
use pretty_assertions::assert_eq;

#[test]
fn runtime_mcp_ownership_candidates_require_an_actual_value_change() {
    let original: TomlValue = toml::from_str(
        r#"model = "fable-fusion"

[mcp_servers.office]
url = "http://127.0.0.1:9123/mcp"

[mcp_servers.human]
command = "human-server"
"#,
    )
    .expect("parse original runtime config");

    assert!(
        changed_runtime_mcp_servers(&original, &original).is_empty(),
        "rewriting an identical Office value must not mint ownership"
    );

    let changed: TomlValue = toml::from_str(
        r#"model = "fable-fusion"

[mcp_servers.office]
url = "http://127.0.0.1:9456/mcp"

[mcp_servers.human]
command = "different-human-server"
"#,
    )
    .expect("parse changed runtime config");
    assert_eq!(
        changed_runtime_mcp_servers(&original, &changed),
        BTreeSet::from(["office".to_string()]),
        "human-owned changes are never runtime ownership candidates"
    );
}

#[test]
fn runtime_mcp_ownership_requires_exact_active_server_and_lkg_base_hashes() {
    let lkg: TomlValue = toml::from_str(
        r#"model = "fable-fusion"

[mcp_servers.human]
command = "human-server"
"#,
    )
    .expect("parse LKG");
    let active: TomlValue = toml::from_str(
        r#"model = "fable-fusion"

[mcp_servers.human]
command = "human-server"

[mcp_servers.office]
url = "http://127.0.0.1:9123/mcp"
"#,
    )
    .expect("parse active config");
    let owned_names = BTreeSet::from(["office".to_string()]);
    let office = active
        .get("mcp_servers")
        .and_then(TomlValue::as_table)
        .and_then(|servers| servers.get("office"))
        .expect("Office server");
    let mut ownership = RuntimeMcpOwnership {
        schema_version: ILHAE_RUNTIME_MCP_OWNERSHIP_SCHEMA_VERSION,
        generation: 1,
        active_sha256: canonical_toml_sha256(&active),
        base_sha256: canonical_toml_sha256(&runtime_mcp_base_projection(&active, &owned_names)),
        servers: BTreeMap::from([("office".to_string(), canonical_toml_sha256(office))]),
    };

    assert!(runtime_mcp_ownership_matches(&ownership, &active, &lkg));

    ownership
        .servers
        .insert("office".to_string(), "0".repeat(64));
    assert!(
        !runtime_mcp_ownership_matches(&ownership, &active, &lkg),
        "a forged per-server hash must invalidate ownership"
    );

    ownership
        .servers
        .insert("office".to_string(), canonical_toml_sha256(office));
    ownership.base_sha256 = "0".repeat(64);
    assert!(
        !runtime_mcp_ownership_matches(&ownership, &active, &lkg),
        "a forged base hash must invalidate ownership"
    );
}

#[test]
fn runtime_mcp_canonical_hash_matches_the_desktop_contract() {
    let fixture: TomlValue = toml::from_str(
        r#"model = "fable-fusion"

[mcp_servers.office]
args = ["serve", "--port", "9123"]
enabled = true
url = "http://127.0.0.1:9123/mcp"
"#,
    )
    .expect("parse canonical hash fixture");

    assert_eq!(
        canonical_toml_sha256(&fixture),
        "c2bbe8008600a3ddf65f9988877b129feac296cf77841588736b140342b92884"
    );
}
