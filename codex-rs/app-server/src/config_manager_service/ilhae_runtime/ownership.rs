//! Desktop MCP ownership sidecar contract, isolated from upstream config editing.

use super::ConfigManagerError;
use super::runtime_config_version_conflict;
use codex_app_server_protocol::ConfigWriteErrorCode;
use codex_core::path_utils::write_atomically;
use serde::Deserialize;
use serde::Serialize;
use sha2::Digest;
use sha2::Sha256;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::path::PathBuf;
use tokio::task;
use toml::Value as TomlValue;

const ILHAE_RUNTIME_CONFIG_LKG_FILE: &str = ".config.toml.ilhae-lkg";
const ILHAE_RUNTIME_MCP_OWNERSHIP_FILE: &str = ".config.toml.ilhae-runtime-ownership.json";
const ILHAE_RUNTIME_MCP_OWNERSHIP_SCHEMA_VERSION: u64 = 1;
const ILHAE_RUNTIME_MCP_OFFICE_SERVER: &str = "office";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct RuntimeMcpOwnership {
    schema_version: u64,
    generation: u64,
    active_sha256: String,
    base_sha256: String,
    servers: BTreeMap<String, String>,
}

pub(super) struct RuntimeMcpOwnershipWrite {
    expected_sidecar: Option<Vec<u8>>,
    next: RuntimeMcpOwnership,
}

fn is_ilhae_runtime_owned_mcp_server(name: &str) -> bool {
    name == ILHAE_RUNTIME_MCP_OFFICE_SERVER || name.starts_with("mcpb_")
}

// Only actual reserved-server value changes are ownership candidates. Merely
// rewriting a human-owned entry must not promote it to runtime ownership.
pub(super) fn changed_runtime_mcp_servers(
    original: &TomlValue,
    updated: &TomlValue,
) -> BTreeSet<String> {
    let original_servers = original.get("mcp_servers").and_then(TomlValue::as_table);
    let updated_servers = updated.get("mcp_servers").and_then(TomlValue::as_table);
    let mut candidates = BTreeSet::new();
    for servers in [original_servers, updated_servers].into_iter().flatten() {
        candidates.extend(
            servers
                .keys()
                .filter(|name| is_ilhae_runtime_owned_mcp_server(name))
                .cloned(),
        );
    }
    candidates.retain(|name| {
        original_servers.and_then(|servers| servers.get(name))
            != updated_servers.and_then(|servers| servers.get(name))
    });
    candidates
}

pub(super) fn prepare_runtime_mcp_ownership_write(
    codex_home: &Path,
    original: &TomlValue,
    updated: &TomlValue,
    changed_servers: &BTreeSet<String>,
) -> Result<Option<RuntimeMcpOwnershipWrite>, ConfigManagerError> {
    let sidecar_path = codex_home.join(ILHAE_RUNTIME_MCP_OWNERSHIP_FILE);
    let expected_sidecar = match std::fs::read(&sidecar_path) {
        Ok(bytes) => Some(bytes),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
        Err(_) => return Ok(None),
    };
    let parsed_sidecar = expected_sidecar
        .as_deref()
        .and_then(|bytes| serde_json::from_slice::<RuntimeMcpOwnership>(bytes).ok());

    let lkg_path = codex_home.join(ILHAE_RUNTIME_CONFIG_LKG_FILE);
    let lkg_bytes = match std::fs::read(&lkg_path) {
        Ok(bytes) => bytes,
        Err(_) => return Ok(None),
    };
    let lkg = match std::str::from_utf8(&lkg_bytes)
        .ok()
        .and_then(|text| toml::from_str::<TomlValue>(text).ok())
    {
        Some(value) => value,
        None => return Ok(None),
    };

    let previous_is_valid = parsed_sidecar.as_ref().is_some_and(|ownership| {
        runtime_mcp_ownership_file_is_private(&sidecar_path)
            && runtime_mcp_ownership_matches(ownership, original, &lkg)
    });
    let mut owned_names: BTreeSet<String> = if previous_is_valid {
        parsed_sidecar
            .as_ref()
            .map(|ownership| ownership.servers.keys().cloned().collect())
            .unwrap_or_default()
    } else {
        BTreeSet::new()
    };

    let updated_servers = updated.get("mcp_servers").and_then(TomlValue::as_table);
    if let Some(previous) = parsed_sidecar.as_ref().filter(|_| previous_is_valid) {
        owned_names.retain(|name| {
            updated_servers
                .and_then(|servers| servers.get(name))
                .is_some_and(|value| {
                    previous.servers.get(name) == Some(&canonical_toml_sha256(value))
                })
        });
    }
    for name in changed_servers {
        if updated_servers
            .and_then(|servers| servers.get(name))
            .is_some()
        {
            owned_names.insert(name.clone());
        } else {
            owned_names.remove(name);
        }
    }

    let updated_base = runtime_mcp_base_projection(updated, &owned_names);
    let lkg_base = runtime_mcp_base_projection(&lkg, &owned_names);
    let updated_base_sha256 = canonical_toml_sha256(&updated_base);
    if updated_base_sha256 != canonical_toml_sha256(&lkg_base) {
        return Ok(None);
    }

    let servers = owned_names
        .iter()
        .filter_map(|name| {
            updated_servers
                .and_then(|table| table.get(name))
                .map(|value| (name.clone(), canonical_toml_sha256(value)))
        })
        .collect();
    let observed_generation = parsed_sidecar
        .as_ref()
        .filter(|_| previous_is_valid)
        .map(|ownership| ownership.generation)
        .unwrap_or(0);
    let generation = observed_generation.checked_add(1).ok_or_else(|| {
        ConfigManagerError::write(
            ConfigWriteErrorCode::ConfigVersionConflict,
            "Runtime ownership generation overflowed; retry after reprojection.",
        )
    })?;

    Ok(Some(RuntimeMcpOwnershipWrite {
        expected_sidecar,
        next: RuntimeMcpOwnership {
            schema_version: ILHAE_RUNTIME_MCP_OWNERSHIP_SCHEMA_VERSION,
            generation,
            active_sha256: canonical_toml_sha256(updated),
            base_sha256: updated_base_sha256,
            servers,
        },
    }))
}

fn runtime_mcp_ownership_matches(
    ownership: &RuntimeMcpOwnership,
    active: &TomlValue,
    lkg: &TomlValue,
) -> bool {
    if ownership.schema_version != ILHAE_RUNTIME_MCP_OWNERSHIP_SCHEMA_VERSION
        || ownership.generation == 0
        || !is_sha256_hex(&ownership.active_sha256)
        || !is_sha256_hex(&ownership.base_sha256)
        || ownership.active_sha256 != canonical_toml_sha256(active)
        || ownership
            .servers
            .iter()
            .any(|(name, hash)| !is_ilhae_runtime_owned_mcp_server(name) || !is_sha256_hex(hash))
    {
        return false;
    }
    let owned_names: BTreeSet<String> = ownership.servers.keys().cloned().collect();
    let active_servers = active.get("mcp_servers").and_then(TomlValue::as_table);
    if ownership.servers.iter().any(|(name, expected_hash)| {
        active_servers
            .and_then(|servers| servers.get(name))
            .map(canonical_toml_sha256)
            .as_ref()
            != Some(expected_hash)
    }) {
        return false;
    }
    ownership.base_sha256
        == canonical_toml_sha256(&runtime_mcp_base_projection(active, &owned_names))
        && ownership.base_sha256
            == canonical_toml_sha256(&runtime_mcp_base_projection(lkg, &owned_names))
}

fn runtime_mcp_base_projection(config: &TomlValue, owned_names: &BTreeSet<String>) -> TomlValue {
    let mut projection = config.clone();
    let Some(root) = projection.as_table_mut() else {
        return projection;
    };
    if let Some(servers) = root
        .get_mut("mcp_servers")
        .and_then(TomlValue::as_table_mut)
    {
        servers.retain(|name, _| !owned_names.contains(name));
        if servers.is_empty() {
            root.remove("mcp_servers");
        }
    }
    projection
}

fn canonical_toml_sha256(value: &TomlValue) -> String {
    let mut canonical = Vec::new();
    append_canonical_toml_value(value, &mut canonical);
    format!("{:x}", Sha256::digest(canonical))
}

fn append_canonical_toml_value(value: &TomlValue, output: &mut Vec<u8>) {
    match value {
        TomlValue::String(value) => {
            output.push(b's');
            append_canonical_bytes(value.as_bytes(), output);
        }
        TomlValue::Integer(value) => {
            output.push(b'i');
            output.extend_from_slice(&value.to_be_bytes());
        }
        TomlValue::Float(value) => {
            output.push(b'f');
            output.extend_from_slice(&value.to_bits().to_be_bytes());
        }
        TomlValue::Boolean(value) => {
            output.push(b'b');
            output.push(u8::from(*value));
        }
        TomlValue::Datetime(value) => {
            output.push(b'd');
            append_canonical_bytes(value.to_string().as_bytes(), output);
        }
        TomlValue::Array(values) => {
            output.push(b'a');
            output.extend_from_slice(&(values.len() as u64).to_be_bytes());
            for value in values {
                append_canonical_toml_value(value, output);
            }
        }
        TomlValue::Table(values) => {
            output.push(b't');
            output.extend_from_slice(&(values.len() as u64).to_be_bytes());
            let mut entries = values.iter().collect::<Vec<_>>();
            entries.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));
            for (key, value) in entries {
                append_canonical_bytes(key.as_bytes(), output);
                append_canonical_toml_value(value, output);
            }
        }
    }
}

fn append_canonical_bytes(bytes: &[u8], output: &mut Vec<u8>) {
    output.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
    output.extend_from_slice(bytes);
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn runtime_mcp_ownership_file_is_private(path: &Path) -> bool {
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return false;
    };
    if !metadata.file_type().is_file() {
        return false;
    }
    #[cfg(unix)]
    if metadata.permissions().mode() & 0o777 != 0o600 {
        return false;
    }
    true
}

pub(super) async fn persist_runtime_mcp_ownership_cas(
    codex_home: PathBuf,
    write: RuntimeMcpOwnershipWrite,
) -> Result<(), ConfigManagerError> {
    task::spawn_blocking(move || {
        let path = codex_home.join(ILHAE_RUNTIME_MCP_OWNERSHIP_FILE);
        let current = match std::fs::read(&path) {
            Ok(bytes) => Some(bytes),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
            Err(err) => return Err(err),
        };
        if current != write.expected_sidecar {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                "runtime ownership changed during update",
            ));
        }
        let mut contents = serde_json::to_string(&write.next)
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
        contents.push('\n');
        write_atomically(&path, &contents)?;
        #[cfg(unix)]
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        Ok(())
    })
    .await
    .map_err(|err| ConfigManagerError::anyhow("runtime ownership task panicked", err.into()))?
    .map_err(|err| {
        if err.kind() == std::io::ErrorKind::WouldBlock {
            runtime_config_version_conflict()
        } else {
            ConfigManagerError::io("failed to persist runtime ownership", err)
        }
    })
}

#[cfg(test)]
#[path = "ownership_tests.rs"]
mod tests;
