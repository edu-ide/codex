use crate::config::ConfigToml;
use crate::config::types::RawMcpServerConfig;
use crate::features::FEATURES;
use schemars::SchemaGenerator;
use schemars::generate::SchemaSettings;
use schemars::Schema;
use serde_json::Map;
use serde_json::Value;
use std::path::Path;

/// Schema for the `[features]` map with known + legacy keys only.
pub(crate) fn features_schema(_: &mut SchemaGenerator) -> Schema {
    let mut properties = serde_json::Map::new();
    let bool_schema = serde_json::json!({ "type": "boolean" });

    for feature in FEATURES {
        properties.insert(feature.key.to_string(), bool_schema.clone());
    }
    for legacy_key in crate::features::legacy_feature_keys() {
        properties.insert(legacy_key.to_string(), bool_schema.clone());
    }

    schemars::json_schema!({
        "type": "object",
        "properties": properties,
        "additionalProperties": false
    })
}

/// Schema for the `[mcp_servers]` map using the raw input shape.
pub(crate) fn mcp_servers_schema(schema_gen: &mut SchemaGenerator) -> Schema {
    let additional = schema_gen.subschema_for::<RawMcpServerConfig>();
    schemars::json_schema!({
        "type": "object",
        "additionalProperties": additional
    })
}

/// Build the config schema for `config.toml`.
pub fn config_schema() -> Schema {
    let generator = SchemaSettings::draft2020_12()
        .into_generator();
    generator.into_root_schema_for::<ConfigToml>()
}

/// Canonicalize a JSON value by sorting its keys.
fn canonicalize(value: &Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.iter().map(canonicalize).collect()),
        Value::Object(map) => {
            let mut entries: Vec<_> = map.iter().collect();
            entries.sort_by(|(left, _), (right, _)| left.cmp(right));
            let mut sorted = Map::with_capacity(map.len());
            for (key, child) in entries {
                sorted.insert(key.clone(), canonicalize(child));
            }
            Value::Object(sorted)
        }
        _ => value.clone(),
    }
}

/// Render the config schema as pretty-printed JSON.
pub fn config_schema_json() -> anyhow::Result<Vec<u8>> {
    let schema = config_schema();
    let value = serde_json::to_value(schema)?;
    let value = canonicalize(&value);
    let json = serde_json::to_vec_pretty(&value)?;
    Ok(json)
}

/// Write the config schema fixture to disk.
pub fn write_config_schema(out_path: &Path) -> anyhow::Result<()> {
    let json = config_schema_json()?;
    std::fs::write(out_path, json)?;
    Ok(())
}

#[cfg(test)]
#[path = "schema_tests.rs"]
mod tests;
