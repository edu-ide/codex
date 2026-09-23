//! Codex schema validation and versioned System2 metadata.

use super::super::IlhaeTomlConfig;
use super::super::ResolvedSystem2TargetConfig;
use super::super::active_system2_target_config_from;
use super::ILHAE_RUNTIME_SYSTEM2_PROJECTION_KEY;
use super::ILHAE_RUNTIME_SYSTEM2_PROJECTION_SCHEMA_VERSION;
use super::mcp::mcp_server_transport_is_semantically_valid;
use std::path::Path;

pub(super) fn system2_projection_table_from(
    config: &IlhaeTomlConfig,
) -> Option<toml::value::Table> {
    let system2 = active_system2_target_config_from(config)?;
    let mut projection = toml::value::Table::new();
    projection.insert(
        "schema_version".to_string(),
        toml::Value::Integer(ILHAE_RUNTIME_SYSTEM2_PROJECTION_SCHEMA_VERSION),
    );
    projection.insert(
        "source_profile_id".to_string(),
        toml::Value::String(system2.source_profile_id),
    );
    projection.insert(
        "target_profile_id".to_string(),
        toml::Value::String(system2.target_profile_id),
    );
    projection.insert(
        "base_url".to_string(),
        toml::Value::String(system2.base_url),
    );
    projection.insert(
        "model_name".to_string(),
        toml::Value::String(system2.model_name),
    );
    Some(projection)
}

pub(super) fn system2_projection_from_runtime_document(
    document: &toml::Value,
) -> Result<Option<ResolvedSystem2TargetConfig>, String> {
    let Some(desktop) = document.get("desktop") else {
        return Ok(None);
    };
    let desktop = desktop
        .as_table()
        .ok_or_else(|| "Generated Codex runtime desktop metadata is invalid".to_string())?;
    let Some(projection) = desktop.get(ILHAE_RUNTIME_SYSTEM2_PROJECTION_KEY) else {
        return Ok(None);
    };
    let projection = projection
        .as_table()
        .ok_or_else(|| "Generated Codex runtime System2 projection is invalid".to_string())?;
    if projection
        .get("schema_version")
        .and_then(toml::Value::as_integer)
        != Some(ILHAE_RUNTIME_SYSTEM2_PROJECTION_SCHEMA_VERSION)
    {
        return Err("Generated Codex runtime System2 projection schema is invalid".to_string());
    }
    let required = |key| {
        projection
            .get(key)
            .and_then(toml::Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map(str::to_string)
            .ok_or_else(|| format!("Generated Codex runtime System2 projection is missing {key}"))
    };
    Ok(Some(ResolvedSystem2TargetConfig {
        source_profile_id: required("source_profile_id")?,
        target_profile_id: required("target_profile_id")?,
        base_url: required("base_url")?,
        model_name: required("model_name")?,
    }))
}

pub(super) fn validate_ilhae_codex_runtime_config(content: &str) -> Result<(), String> {
    parse_validated_ilhae_codex_runtime_config(content).map(drop)
}

pub(super) fn parse_validated_ilhae_codex_runtime_config(
    content: &str,
) -> Result<Option<ResolvedSystem2TargetConfig>, String> {
    let parsed = toml::from_str::<codex_config::config_toml::ConfigToml>(content)
        .map_err(|error| format!("Generated Codex runtime config is invalid: {error}"))?;
    for (name, server) in &parsed.mcp_servers {
        if !mcp_server_transport_is_semantically_valid(server) {
            return Err(format!(
                "Generated Codex runtime config contains an unusable MCP transport: {name}"
            ));
        }
    }
    let document = content
        .parse::<toml::Value>()
        .map_err(|_| "Generated Codex runtime config is not valid TOML".to_string())?;
    system2_projection_from_runtime_document(&document)
}

pub(super) fn read_valid_ilhae_codex_runtime_config(path: &Path) -> Option<Vec<u8>> {
    read_valid_ilhae_codex_runtime_config_with_system2(path).map(|(bytes, _)| bytes)
}

pub(super) fn read_valid_ilhae_codex_runtime_config_with_system2(
    path: &Path,
) -> Option<(Vec<u8>, Option<ResolvedSystem2TargetConfig>)> {
    let bytes = std::fs::read(path).ok()?;
    let content = std::str::from_utf8(&bytes).ok()?;
    let system2 = parse_validated_ilhae_codex_runtime_config(content).ok()?;
    Some((bytes, system2))
}
