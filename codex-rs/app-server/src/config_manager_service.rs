use crate::config_layer::config_layer_metadata_to_api;
use crate::config_layer::config_layer_to_api;
use crate::config_manager::ConfigManager;
use codex_app_server_protocol::Config as ApiConfig;
use codex_app_server_protocol::ConfigBatchWriteParams;
use codex_app_server_protocol::ConfigReadParams;
use codex_app_server_protocol::ConfigReadResponse;
use codex_app_server_protocol::ConfigValueWriteParams;
use codex_app_server_protocol::ConfigWriteErrorCode;
use codex_app_server_protocol::ConfigWriteResponse;
use codex_app_server_protocol::MergeStrategy;
use codex_app_server_protocol::OverriddenMetadata;
use codex_app_server_protocol::WriteStatus;
use codex_config::CONFIG_TOML_FILE;
use codex_config::ConfigLayerEntry;
use codex_config::ConfigLayerMetadata;
use codex_config::ConfigLayerSource;
use codex_config::ConfigLayerStack;
use codex_config::ConfigRequirementsToml;
use codex_config::ShellEnvironmentPolicyFilterRepresentation;
use codex_config::config_toml::ConfigToml;
use codex_config::merge_toml_values;
use codex_config::shell_environment_filter_entry;
use codex_config::validate_shell_environment_policy_filter_config;
use codex_core::config::deserialize_config_toml_with_base;
use codex_core::config::edit::ConfigEdit;
use codex_core::config::edit::ConfigEditsBuilder;
use codex_core::config::validate_feature_requirements_for_config_toml;
use codex_core::path_utils;
use codex_core::path_utils::SymlinkWritePaths;
use codex_core::path_utils::resolve_symlink_write_paths;
use codex_core::path_utils::write_atomically;
use codex_utils_absolute_path::AbsolutePathBuf;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value as JsonValue;
use sha2::Digest;
use sha2::Sha256;
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fs::File;
use std::fs::OpenOptions;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;
use std::time::Instant;
use thiserror::Error;
use tokio::task;
use toml::Value as TomlValue;
use toml_edit::Item as TomlItem;

const ILHAE_RUNTIME_CONFIG_LOCK_FILE: &str = ".config.toml.ilhae-runtime.lock";
const ILHAE_RUNTIME_CONFIG_LKG_FILE: &str = ".config.toml.ilhae-lkg";
const ILHAE_RUNTIME_MCP_OWNERSHIP_FILE: &str = ".config.toml.ilhae-runtime-ownership.json";
const ILHAE_RUNTIME_CONFIG_LOCK_TIMEOUT: Duration = Duration::from_secs(3);
const ILHAE_RUNTIME_CONFIG_LOCK_POLL_INTERVAL: Duration = Duration::from_millis(10);
const ILHAE_RUNTIME_MCP_OWNERSHIP_SCHEMA_VERSION: u64 = 1;
const ILHAE_RUNTIME_MCP_OFFICE_SERVER: &str = "office";
const ILHAE_RUNTIME_SYSTEM2_PROJECTION_KEY: &str = "ilhae_runtime_system2_projection";
const ILHAE_RUNTIME_CONFIG_LOCK_CONTENTION_MESSAGE: &str =
    "Runtime configuration is being updated; retry this edit.";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct RuntimeMcpOwnership {
    schema_version: u64,
    generation: u64,
    active_sha256: String,
    base_sha256: String,
    servers: BTreeMap<String, String>,
}

struct RuntimeMcpOwnershipWrite {
    expected_sidecar: Option<Vec<u8>>,
    next: RuntimeMcpOwnership,
}

#[derive(Debug, Error)]
pub(crate) enum ConfigManagerError {
    #[error("{message}")]
    Write {
        code: ConfigWriteErrorCode,
        message: String,
    },

    #[error("{context}: {source}")]
    Io {
        context: &'static str,
        #[source]
        source: std::io::Error,
    },

    #[error("{context}: {source}")]
    Json {
        context: &'static str,
        #[source]
        source: serde_json::Error,
    },

    #[error("{context}: {source}")]
    Toml {
        context: &'static str,
        #[source]
        source: toml::de::Error,
    },

    #[error("{context}: {source}")]
    Anyhow {
        context: &'static str,
        #[source]
        source: anyhow::Error,
    },
}

impl ConfigManagerError {
    fn write(code: ConfigWriteErrorCode, message: impl Into<String>) -> Self {
        Self::Write {
            code,
            message: message.into(),
        }
    }

    fn io(context: &'static str, source: std::io::Error) -> Self {
        Self::Io { context, source }
    }

    fn json(context: &'static str, source: serde_json::Error) -> Self {
        Self::Json { context, source }
    }

    fn toml(context: &'static str, source: toml::de::Error) -> Self {
        Self::Toml { context, source }
    }

    fn anyhow(context: &'static str, source: anyhow::Error) -> Self {
        Self::Anyhow { context, source }
    }

    pub(crate) fn write_error_code(&self) -> Option<ConfigWriteErrorCode> {
        match self {
            Self::Write { code, .. } => Some(code.clone()),
            _ => None,
        }
    }
}

impl ConfigManager {
    pub(crate) async fn read(
        &self,
        params: ConfigReadParams,
    ) -> Result<ConfigReadResponse, ConfigManagerError> {
        let layers = match params.cwd.as_deref() {
            Some(cwd) => {
                let cwd = AbsolutePathBuf::try_from(PathBuf::from(cwd)).map_err(|err| {
                    ConfigManagerError::io("failed to resolve config cwd to an absolute path", err)
                })?;
                self.load_config_layers(Some(cwd)).await.map_err(|err| {
                    ConfigManagerError::io("failed to read configuration layers", err)
                })?
            }
            None => self.load_thread_agnostic_config().await.map_err(|err| {
                ConfigManagerError::io("failed to read configuration layers", err)
            })?,
        };

        let effective = layers.effective_config();
        let mut effective_config_toml: ConfigToml = effective
            .try_into()
            .map_err(|err| ConfigManagerError::toml("invalid configuration", err))?;
        layers
            .requirements_toml()
            .apply_exact_to_config(&mut effective_config_toml);
        effective_config_toml.allow_login_shell.get_or_insert(true);

        let json_value = serde_json::to_value(&effective_config_toml)
            .map_err(|err| ConfigManagerError::json("failed to serialize configuration", err))?;
        let config: ApiConfig = serde_json::from_value(json_value)
            .map_err(|err| ConfigManagerError::json("failed to deserialize configuration", err))?;

        let mut origins = layers.origins();
        origins.retain(|path, _| {
            let segments = path.split('.').map(str::to_string).collect::<Vec<_>>();
            layers
                .requirements_toml()
                .exact_requirement_for_config_path(&segments)
                .is_none()
        });

        Ok(ConfigReadResponse {
            config,
            origins: origins
                .into_iter()
                .map(|(path, metadata)| (path, config_layer_metadata_to_api(metadata)))
                .collect(),
            layers: params.include_layers.then(|| {
                layers
                    .all_layers_high_to_low()
                    .map(|layer| config_layer_to_api(layer.as_layer()))
                    .collect()
            }),
        })
    }

    pub(crate) async fn read_requirements(
        &self,
    ) -> Result<Option<ConfigRequirementsToml>, ConfigManagerError> {
        let layers = self
            .load_thread_agnostic_config()
            .await
            .map_err(|err| ConfigManagerError::io("failed to read configuration layers", err))?;

        let requirements = layers.requirements_toml().clone();
        if requirements.is_empty() {
            Ok(None)
        } else {
            Ok(Some(requirements))
        }
    }

    pub(crate) async fn write_value(
        &self,
        params: ConfigValueWriteParams,
    ) -> Result<ConfigWriteResponse, ConfigManagerError> {
        let edits = vec![(params.key_path, params.value, params.merge_strategy)];
        self.apply_edits(params.file_path, params.expected_version, edits)
            .await
    }

    /// Clears a value from the active user config only when its current raw value matches.
    pub(crate) async fn clear_user_value_if_matches(
        &self,
        key_path: &str,
        expected_value: JsonValue,
    ) -> Result<(), ConfigManagerError> {
        let layers = self
            .load_thread_agnostic_config()
            .await
            .map_err(|err| ConfigManagerError::io("failed to load configuration", err))?;
        let Some(user_layer) = layers.get_active_user_layer() else {
            return Ok(());
        };
        let segments = parse_key_path(key_path).map_err(|message| {
            ConfigManagerError::write(ConfigWriteErrorCode::ConfigValidationError, message)
        })?;
        let expected_value = parse_value(expected_value).map_err(|message| {
            ConfigManagerError::write(ConfigWriteErrorCode::ConfigValidationError, message)
        })?;
        if value_at_path(&user_layer.config, &segments) != expected_value.as_ref() {
            return Ok(());
        }
        let expected_version = Some(user_layer.version.clone());

        self.apply_edits(
            /*file_path*/ None,
            expected_version,
            vec![(
                key_path.to_string(),
                JsonValue::Null,
                MergeStrategy::Replace,
            )],
        )
        .await?;
        Ok(())
    }

    pub(crate) async fn batch_write(
        &self,
        params: ConfigBatchWriteParams,
    ) -> Result<ConfigWriteResponse, ConfigManagerError> {
        let edits = params
            .edits
            .into_iter()
            .map(|edit| (edit.key_path, edit.value, edit.merge_strategy))
            .collect();

        self.apply_edits(params.file_path, params.expected_version, edits)
            .await
    }

    async fn apply_edits(
        &self,
        file_path: Option<String>,
        expected_version: Option<String>,
        edits: Vec<(String, JsonValue, MergeStrategy)>,
    ) -> Result<ConfigWriteResponse, ConfigManagerError> {
        let allowed_path = self
            .user_config_path()
            .map_err(|err| ConfigManagerError::io("failed to resolve user config path", err))?;
        let provided_path = match file_path {
            Some(path) => AbsolutePathBuf::from_absolute_path(PathBuf::from(path))
                .map_err(|err| ConfigManagerError::io("failed to resolve user config path", err))?,
            None => allowed_path.clone(),
        };

        if !paths_match(&allowed_path, &provided_path) {
            return Err(ConfigManagerError::write(
                ConfigWriteErrorCode::ConfigLayerReadonly,
                "Only writes to the user config are allowed",
            ));
        }

        // ILHAE Desktop의 preflight와 config/batchWrite는 같은 generated
        // config.toml을 읽고 쓴다. 첫 load보다 먼저 공용 OS lock을 잡고 최종 version
        // 응답을 만든 뒤까지 유지해 read-validate-persist를 하나의 트랜잭션으로 본다.
        // 일반 Codex app-server에는 이 제품 전용 sideband/locking을 적용하지 않는다.
        let ilhae_runtime_guard =
            acquire_ilhae_runtime_config_lock_if_enabled(self.codex_home().to_path_buf()).await?;

        let layers = self
            .load_thread_agnostic_config()
            .await
            .map_err(|err| ConfigManagerError::io("failed to load configuration", err))?;
        let user_layer = match layers.get_active_user_layer() {
            Some(layer) => Cow::Borrowed(layer),
            None => Cow::Owned(create_empty_user_layer(&allowed_path).await?),
        };

        if let Some(expected) = expected_version.as_deref()
            && expected != user_layer.version
        {
            return Err(ConfigManagerError::write(
                ConfigWriteErrorCode::ConfigVersionConflict,
                "Configuration was modified since last read. Fetch latest version and retry.",
            ));
        }

        let original_user_config = user_layer.config.clone();
        let runtime_config_snapshot = if ilhae_runtime_guard.is_some() {
            Some(capture_runtime_config_cas_snapshot(&provided_path, &original_user_config).await?)
        } else {
            None
        };
        let mut user_config = original_user_config.clone();
        let mut parsed_segments = Vec::new();
        let mut config_edits = Vec::new();

        for (key_path, value, strategy) in edits.into_iter() {
            let mut segments = parse_key_path(&key_path).map_err(|message| {
                ConfigManagerError::write(ConfigWriteErrorCode::ConfigValidationError, message)
            })?;
            if let Some(field) = layers
                .requirements_toml()
                .exact_requirement_for_config_path(&segments)
            {
                return Err(ConfigManagerError::write(
                    ConfigWriteErrorCode::ConfigRequirementReadonly,
                    format!("`{field}` is managed by requirements and cannot be changed"),
                ));
            }
            if (value.is_null() || matches!(strategy, MergeStrategy::Upsert))
                && let Some(pattern) = shell_environment_filter_entry(&user_config, &segments)
                    .map(|(pattern, _)| pattern.clone())
            {
                segments[2] = pattern;
            }
            reject_direct_runtime_projection_path(&segments)?;
            if !value.is_null() {
                match segments.as_slice() {
                    [segment] if segment == "profile" => {
                        return Err(ConfigManagerError::write(
                            ConfigWriteErrorCode::ConfigValidationError,
                            "`profile` is a legacy config selector and can no longer be written; use `--profile <name>` with `<name>.config.toml` instead",
                        ));
                    }
                    [segment, ..] if segment == "profiles" => {
                        return Err(ConfigManagerError::write(
                            ConfigWriteErrorCode::ConfigValidationError,
                            "`profiles` contains legacy config profile tables and can no longer be written; use `--profile <name>` with `<name>.config.toml` instead",
                        ));
                    }
                    _ => {}
                }
            }
            let parsed_value = parse_value(value).map_err(|message| {
                ConfigManagerError::write(ConfigWriteErrorCode::ConfigValidationError, message)
            })?;
            if matches!(strategy, MergeStrategy::Upsert)
                && let Some(value) = parsed_value.as_ref()
                && matches!(segments.as_slice(), [policy, ..] if policy == "shell_environment_policy")
            {
                validate_shell_environment_policy_filter_config(&sparse_overlay(&segments, value))
                    .map_err(|err| {
                        ConfigManagerError::write(
                            ConfigWriteErrorCode::ConfigValidationError,
                            format!("Invalid configuration: {err}"),
                        )
                    })?;
            }

            let persist_segments = if matches!(strategy, MergeStrategy::Upsert)
                && parsed_value.as_ref().is_some_and(|value| {
                    shell_environment_policy_representation_switch(&user_config, &segments, value)
                }) {
                vec!["shell_environment_policy".to_string()]
            } else {
                segments.clone()
            };
            let original_value = value_at_path(&user_config, &persist_segments).cloned();

            apply_merge(&mut user_config, &segments, parsed_value.as_ref(), strategy).map_err(
                |err| match err {
                    MergeError::Validation(message) => ConfigManagerError::write(
                        ConfigWriteErrorCode::ConfigValidationError,
                        message,
                    ),
                },
            )?;

            let updated_value = value_at_path(&user_config, &persist_segments).cloned();
            if original_value != updated_value {
                config_edits.push(match updated_value {
                    Some(value) => ConfigEdit::SetPath {
                        segments: persist_segments,
                        value: toml_value_to_item(&value).map_err(|err| {
                            ConfigManagerError::anyhow("failed to build config edits", err)
                        })?,
                    },
                    None => ConfigEdit::ClearPath {
                        segments: persist_segments,
                    },
                });
            }

            parsed_segments.push(segments);
        }

        // 이름이나 keyPath를 만졌다는 사실은 ownership 증명이 아니다. 실제 값이
        // 달라진 reserved server만 후보로 삼아 동일 값 재쓰기로 human entry가
        // runtime-owned로 승격되는 no-op laundering을 막는다.
        let changed_runtime_servers =
            changed_runtime_mcp_servers(&original_user_config, &user_config);
        ensure_runtime_projection_unchanged(&original_user_config, &user_config)?;

        validate_config(&user_config).map_err(|err| {
            ConfigManagerError::write(
                ConfigWriteErrorCode::ConfigValidationError,
                format!("Invalid configuration: {err}"),
            )
        })?;
        let user_config_toml =
            deserialize_config_toml_with_base(user_config.clone(), self.codex_home()).map_err(
                |err| {
                    ConfigManagerError::write(
                        ConfigWriteErrorCode::ConfigValidationError,
                        format!("Invalid configuration: {err}"),
                    )
                },
            )?;
        validate_feature_requirements_for_config_toml(
            &user_config_toml,
            layers.requirements().feature_requirements.as_ref(),
        )
        .map_err(|err| {
            ConfigManagerError::write(
                ConfigWriteErrorCode::ConfigValidationError,
                format!("Invalid configuration: {err}"),
            )
        })?;
        let updated_layers = layers
            .with_user_config(&provided_path, user_config.clone())
            .map_err(|err| {
                ConfigManagerError::write(
                    ConfigWriteErrorCode::ConfigValidationError,
                    format!("Invalid configuration: {err}"),
                )
            })?;
        let effective = updated_layers.effective_config();
        validate_config(&effective).map_err(|err| {
            ConfigManagerError::write(
                ConfigWriteErrorCode::ConfigValidationError,
                format!("Invalid configuration: {err}"),
            )
        })?;

        if !config_edits.is_empty() {
            let runtime_ownership_write =
                if ilhae_runtime_guard.is_some() && !changed_runtime_servers.is_empty() {
                    prepare_runtime_mcp_ownership_write(
                        self.codex_home(),
                        &original_user_config,
                        &user_config,
                        &changed_runtime_servers,
                    )?
                } else {
                    None
                };

            if let Some(expected_snapshot) = runtime_config_snapshot.as_ref() {
                ensure_runtime_config_cas_snapshot_unchanged(&provided_path, expected_snapshot)
                    .await?;
            }
            if let Some(write) = runtime_ownership_write {
                persist_runtime_mcp_ownership_cas(self.codex_home().to_path_buf(), write).await?;
            }
            ConfigEditsBuilder::for_config_path(provided_path.as_path())
                .with_edits(config_edits)
                .apply()
                .await
                .map_err(|err| ConfigManagerError::anyhow("failed to persist config.toml", err))?;
        }

        let overridden = first_overridden_edit(&updated_layers, &effective, &parsed_segments);
        let status = overridden
            .as_ref()
            .map(|_| WriteStatus::OkOverridden)
            .unwrap_or(WriteStatus::Ok);

        Ok(ConfigWriteResponse {
            status,
            version: updated_layers
                .get_active_user_layer()
                .ok_or_else(|| {
                    ConfigManagerError::write(
                        ConfigWriteErrorCode::UserLayerNotFound,
                        "user layer not found in updated layers",
                    )
                })?
                .version
                .clone(),
            file_path: provided_path,
            overridden_metadata: overridden,
        })
    }

    /// Loads a "thread-agnostic" config, which means the config layers do not
    /// include any in-repo .codex/ folders because there is no cwd/project root
    /// associated with this query.
    async fn load_thread_agnostic_config(&self) -> std::io::Result<ConfigLayerStack> {
        self.load_config_layers(/*cwd*/ None).await
    }
}

fn ilhae_runtime_config_writes_enabled() -> bool {
    std::env::var("ILHAE_APP_SERVER")
        .ok()
        .is_some_and(|value| value.trim() == "1")
}

async fn acquire_ilhae_runtime_config_lock_if_enabled(
    codex_home: PathBuf,
) -> Result<Option<File>, ConfigManagerError> {
    if !ilhae_runtime_config_writes_enabled() {
        return Ok(None);
    }

    let lock = task::spawn_blocking(move || {
        acquire_ilhae_runtime_config_lock_blocking(
            &codex_home,
            ILHAE_RUNTIME_CONFIG_LOCK_TIMEOUT,
            ILHAE_RUNTIME_CONFIG_LOCK_POLL_INTERVAL,
        )
    })
    .await
    .map_err(|err| ConfigManagerError::anyhow("runtime config lock task panicked", err.into()))?
    .map_err(|err| ConfigManagerError::io("failed to lock runtime configuration", err))?;

    lock.map(Some).ok_or_else(|| {
        ConfigManagerError::write(
            ConfigWriteErrorCode::ConfigVersionConflict,
            ILHAE_RUNTIME_CONFIG_LOCK_CONTENTION_MESSAGE,
        )
    })
}

fn acquire_ilhae_runtime_config_lock_blocking(
    codex_home: &Path,
    timeout: Duration,
    poll_interval: Duration,
) -> std::io::Result<Option<File>> {
    std::fs::create_dir_all(codex_home)?;
    let lock_path = codex_home.join(ILHAE_RUNTIME_CONFIG_LOCK_FILE);
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    options.mode(0o600);
    let file = options.open(lock_path)?;
    #[cfg(unix)]
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;

    let started = Instant::now();
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(Some(file)),
            Err(std::fs::TryLockError::WouldBlock) => {
                let elapsed = started.elapsed();
                if elapsed >= timeout {
                    return Ok(None);
                }
                let wait = poll_interval.min(timeout.saturating_sub(elapsed));
                if wait.is_zero() {
                    thread::yield_now();
                } else {
                    thread::sleep(wait);
                }
            }
            Err(std::fs::TryLockError::Error(err)) => return Err(err),
        }
    }
}

fn is_ilhae_runtime_owned_mcp_server(name: &str) -> bool {
    name == ILHAE_RUNTIME_MCP_OFFICE_SERVER || name.starts_with("mcpb_")
}

fn reject_direct_runtime_projection_path(segments: &[String]) -> Result<(), ConfigManagerError> {
    if segments.first().map(String::as_str) == Some("desktop")
        && segments.get(1).map(String::as_str) == Some(ILHAE_RUNTIME_SYSTEM2_PROJECTION_KEY)
    {
        return Err(ConfigManagerError::write(
            ConfigWriteErrorCode::ConfigLayerReadonly,
            "This desktop runtime projection is managed internally.",
        ));
    }
    Ok(())
}

fn ensure_runtime_projection_unchanged(
    original: &TomlValue,
    updated: &TomlValue,
) -> Result<(), ConfigManagerError> {
    let path = [
        "desktop".to_string(),
        ILHAE_RUNTIME_SYSTEM2_PROJECTION_KEY.to_string(),
    ];
    if value_at_path(original, &path) != value_at_path(updated, &path) {
        return Err(ConfigManagerError::write(
            ConfigWriteErrorCode::ConfigLayerReadonly,
            "This desktop runtime projection is managed internally.",
        ));
    }
    Ok(())
}

fn changed_runtime_mcp_servers(original: &TomlValue, updated: &TomlValue) -> BTreeSet<String> {
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

async fn capture_runtime_config_cas_snapshot(
    config_path: &AbsolutePathBuf,
    expected_config: &TomlValue,
) -> Result<Option<Vec<u8>>, ConfigManagerError> {
    let snapshot = read_optional_runtime_file(config_path.as_path()).await?;
    let parsed = parse_runtime_config_snapshot(snapshot.as_deref())?;
    if &parsed != expected_config {
        return Err(runtime_config_version_conflict());
    }
    Ok(snapshot)
}

async fn ensure_runtime_config_cas_snapshot_unchanged(
    config_path: &AbsolutePathBuf,
    expected_snapshot: &Option<Vec<u8>>,
) -> Result<(), ConfigManagerError> {
    let current = read_optional_runtime_file(config_path.as_path()).await?;
    if &current != expected_snapshot {
        return Err(runtime_config_version_conflict());
    }
    Ok(())
}

async fn read_optional_runtime_file(path: &Path) -> Result<Option<Vec<u8>>, ConfigManagerError> {
    match tokio::fs::read(path).await {
        Ok(bytes) => Ok(Some(bytes)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(ConfigManagerError::io(
            "failed to read runtime configuration",
            err,
        )),
    }
}

fn parse_runtime_config_snapshot(bytes: Option<&[u8]>) -> Result<TomlValue, ConfigManagerError> {
    match bytes {
        Some(bytes) => {
            let text = std::str::from_utf8(bytes).map_err(|err| {
                ConfigManagerError::write(
                    ConfigWriteErrorCode::ConfigVersionConflict,
                    format!("Configuration changed to non-UTF-8 data: {err}"),
                )
            })?;
            toml::from_str(text).map_err(|_| runtime_config_version_conflict())
        }
        None => Ok(TomlValue::Table(toml::map::Map::new())),
    }
}

fn runtime_config_version_conflict() -> ConfigManagerError {
    ConfigManagerError::write(
        ConfigWriteErrorCode::ConfigVersionConflict,
        "Configuration was modified outside the runtime transaction. Fetch latest version and retry.",
    )
}

fn prepare_runtime_mcp_ownership_write(
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

async fn persist_runtime_mcp_ownership_cas(
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

async fn create_empty_user_layer(
    config_toml: &AbsolutePathBuf,
) -> Result<ConfigLayerEntry, ConfigManagerError> {
    let SymlinkWritePaths {
        read_path,
        write_path,
    } = resolve_symlink_write_paths(config_toml.as_path())
        .map_err(|err| ConfigManagerError::io("failed to resolve user config path", err))?;
    let toml_value = match read_path {
        Some(path) => match tokio::fs::read_to_string(&path).await {
            Ok(contents) => toml::from_str(&contents).map_err(|e| {
                ConfigManagerError::toml("failed to parse existing user config.toml", e)
            })?,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                write_empty_user_config(write_path.clone()).await?;
                TomlValue::Table(toml::map::Map::new())
            }
            Err(err) => {
                return Err(ConfigManagerError::io(
                    "failed to read user config.toml",
                    err,
                ));
            }
        },
        None => {
            write_empty_user_config(write_path).await?;
            TomlValue::Table(toml::map::Map::new())
        }
    };
    Ok(ConfigLayerEntry::new(
        ConfigLayerSource::User {
            file: config_toml.clone(),
            profile: None,
        },
        toml_value,
    ))
}

async fn write_empty_user_config(write_path: PathBuf) -> Result<(), ConfigManagerError> {
    task::spawn_blocking(move || write_atomically(&write_path, ""))
        .await
        .map_err(|err| ConfigManagerError::anyhow("config persistence task panicked", err.into()))?
        .map_err(|err| ConfigManagerError::io("failed to create empty user config.toml", err))
}

fn parse_value(value: JsonValue) -> Result<Option<TomlValue>, String> {
    if value.is_null() {
        return Ok(None);
    }

    serde_json::from_value::<TomlValue>(value)
        .map(Some)
        .map_err(|err| format!("invalid value: {err}"))
}

fn parse_key_path(path: &str) -> Result<Vec<String>, String> {
    if path.trim().is_empty() {
        return Err("keyPath must not be empty".to_string());
    }

    let mut segments = Vec::new();
    let mut segment = String::new();
    let mut chars = path.chars();
    let mut quoted = false;

    // Split on dots unless they appear inside a quoted segment. Bare segments
    // intentionally stay permissive so existing paths like `sample@catalog`
    // remain valid.
    while let Some(ch) = chars.next() {
        match ch {
            '"' if segment.is_empty() && !quoted => quoted = true,
            '"' if quoted => quoted = false,
            '\\' if quoted => {
                // Quoted segments may escape punctuation that would otherwise
                // participate in parsing, such as `.` or `"`.
                let Some(escaped) = chars.next() else {
                    return Err("unterminated escape in keyPath".to_string());
                };
                segment.push(escaped);
            }
            '.' if !quoted => {
                if segment.is_empty() {
                    return Err("keyPath segments must not be empty".to_string());
                }
                segments.push(std::mem::take(&mut segment));
            }
            '"' => return Err("invalid quoted keyPath segment".to_string()),
            _ => segment.push(ch),
        }
    }

    if quoted {
        return Err("unterminated quoted keyPath segment".to_string());
    }
    if segment.is_empty() {
        return Err("keyPath segments must not be empty".to_string());
    }

    segments.push(segment);
    Ok(segments)
}

#[derive(Debug)]
enum MergeError {
    Validation(String),
}

fn apply_merge(
    root: &mut TomlValue,
    segments: &[String],
    value: Option<&TomlValue>,
    strategy: MergeStrategy,
) -> Result<bool, MergeError> {
    let Some(value) = value else {
        return clear_path(root, segments);
    };

    let Some((last, parents)) = segments.split_last() else {
        return Err(MergeError::Validation(
            "keyPath must not be empty".to_string(),
        ));
    };

    let multi_agent_v2_feature_depth = match segments {
        [features, feature, ..] if features == "features" && feature == "multi_agent_v2" => Some(2),
        [profiles, _, features, feature, ..]
            if profiles == "profiles" && features == "features" && feature == "multi_agent_v2" =>
        {
            Some(4)
        }
        _ => None,
    };
    let preserves_multi_agent_v2_feature_config =
        multi_agent_v2_feature_depth.is_some_and(|feature_depth| {
            match value_at_path(root, &segments[..feature_depth]) {
                Some(TomlValue::Boolean(_)) => {
                    segments.len() > feature_depth || matches!(value, TomlValue::Table(_))
                }
                Some(TomlValue::Table(_)) => {
                    segments.len() == feature_depth && matches!(value, TomlValue::Boolean(_))
                }
                _ => false,
            }
        });

    if preserves_multi_agent_v2_feature_config
        || matches!(strategy, MergeStrategy::Upsert)
            && (shell_environment_policy_representation_switch(root, segments, value)
                || (matches!(value_at_path(root, segments), Some(TomlValue::Table(_)))
                    && matches!(value, TomlValue::Table(_))))
    {
        let overlay = sparse_overlay(segments, value);
        merge_toml_values(root, &overlay);
        return Ok(true);
    }

    let mut current = root;

    for segment in parents {
        match current {
            TomlValue::Table(table) => {
                current = table
                    .entry(segment.clone())
                    .or_insert_with(|| TomlValue::Table(toml::map::Map::new()));
            }
            _ => {
                *current = TomlValue::Table(toml::map::Map::new());
                if let TomlValue::Table(table) = current {
                    current = table
                        .entry(segment.clone())
                        .or_insert_with(|| TomlValue::Table(toml::map::Map::new()));
                }
            }
        }
    }

    let table = current.as_table_mut().ok_or_else(|| {
        MergeError::Validation("cannot set value on non-table parent".to_string())
    })?;

    let changed = table
        .get(last)
        .map(|existing| Some(existing) != Some(value))
        .unwrap_or(true);
    table.insert(last.clone(), value.clone());
    Ok(changed)
}

fn sparse_overlay(path: &[String], value: &TomlValue) -> TomlValue {
    path.iter().rev().fold(value.clone(), |value, segment| {
        TomlValue::Table(toml::map::Map::from_iter([(segment.clone(), value)]))
    })
}

fn shell_environment_policy_representation_switch(
    root: &TomlValue,
    segments: &[String],
    value: &TomlValue,
) -> bool {
    let current = root
        .get("shell_environment_policy")
        .and_then(ShellEnvironmentPolicyFilterRepresentation::from_policy);
    let edited = ShellEnvironmentPolicyFilterRepresentation::from_edit(segments, value);
    current
        .zip(edited)
        .is_some_and(|(current, edited)| current != edited)
}

fn clear_path(root: &mut TomlValue, segments: &[String]) -> Result<bool, MergeError> {
    let Some((last, parents)) = segments.split_last() else {
        return Err(MergeError::Validation(
            "keyPath must not be empty".to_string(),
        ));
    };

    let mut current = root;
    for segment in parents {
        match current {
            TomlValue::Table(table) => {
                let Some(next) = table.get_mut(segment) else {
                    return Ok(false);
                };
                current = next;
            }
            _ => return Ok(false),
        }
    }

    let Some(parent) = current.as_table_mut() else {
        return Ok(false);
    };

    Ok(parent.remove(last).is_some())
}

fn toml_value_to_item(value: &TomlValue) -> anyhow::Result<TomlItem> {
    match value {
        TomlValue::Table(table) => {
            let mut table_item = toml_edit::Table::new();
            table_item.set_implicit(false);
            for (key, val) in table {
                table_item.insert(key, toml_value_to_item(val)?);
            }
            Ok(TomlItem::Table(table_item))
        }
        other => Ok(TomlItem::Value(toml_value_to_value(other)?)),
    }
}

fn toml_value_to_value(value: &TomlValue) -> anyhow::Result<toml_edit::Value> {
    match value {
        TomlValue::String(val) => Ok(toml_edit::Value::from(val.clone())),
        TomlValue::Integer(val) => Ok(toml_edit::Value::from(*val)),
        TomlValue::Float(val) => Ok(toml_edit::Value::from(*val)),
        TomlValue::Boolean(val) => Ok(toml_edit::Value::from(*val)),
        TomlValue::Datetime(val) => Ok(toml_edit::Value::from(*val)),
        TomlValue::Array(items) => {
            let mut array = toml_edit::Array::new();
            for item in items {
                array.push(toml_value_to_value(item)?);
            }
            Ok(toml_edit::Value::Array(array))
        }
        TomlValue::Table(table) => {
            let mut inline = toml_edit::InlineTable::new();
            for (key, val) in table {
                inline.insert(key, toml_value_to_value(val)?);
            }
            Ok(toml_edit::Value::InlineTable(inline))
        }
    }
}

fn validate_config(value: &TomlValue) -> Result<(), toml::de::Error> {
    let _: ConfigToml = value.clone().try_into()?;
    Ok(())
}

fn paths_match(expected: impl AsRef<Path>, provided: impl AsRef<Path>) -> bool {
    path_utils::paths_match_after_normalization(expected, provided)
}

fn value_at_path<'a>(root: &'a TomlValue, segments: &[String]) -> Option<&'a TomlValue> {
    let mut current = root;
    for segment in segments {
        match current {
            TomlValue::Table(table) => {
                current = table.get(segment)?;
            }
            TomlValue::Array(items) => {
                let idx = segment.parse::<i64>().ok()?;
                let idx = usize::try_from(idx).ok()?;
                current = items.get(idx)?;
            }
            _ => return None,
        }
    }
    Some(current)
}

fn value_at_semantic_path<'a>(root: &'a TomlValue, segments: &[String]) -> Option<&'a TomlValue> {
    shell_environment_filter_entry(root, segments)
        .map(|(_, value)| value)
        .or_else(|| value_at_path(root, segments))
        .or_else(|| {
            let (field, parents) = segments.split_last()?;
            if field != "enabled" {
                return None;
            }
            let is_multi_agent_v2_feature = match parents {
                [features, feature] => features == "features" && feature == "multi_agent_v2",
                [profiles, _, features, feature] => {
                    profiles == "profiles" && features == "features" && feature == "multi_agent_v2"
                }
                _ => false,
            };
            if !is_multi_agent_v2_feature {
                return None;
            }
            let feature = value_at_path(root, parents)?;
            matches!(feature, TomlValue::Boolean(_)).then_some(feature)
        })
}

fn override_message(layer: &ConfigLayerSource) -> String {
    match layer {
        ConfigLayerSource::Mdm { domain, key: _ } => {
            format!("Overridden by managed policy (MDM): {domain}")
        }
        ConfigLayerSource::System { file } => {
            format!("Overridden by managed config (system): {}", file.display())
        }
        ConfigLayerSource::EnterpriseManaged { id: _, name } => {
            format!("Overridden by enterprise-managed config: {name}")
        }
        ConfigLayerSource::Project { dot_codex_folder } => format!(
            "Overridden by project config: {}/{CONFIG_TOML_FILE}",
            dot_codex_folder.display(),
        ),
        ConfigLayerSource::SessionFlags => "Overridden by session flags".to_string(),
        ConfigLayerSource::User { file, .. } => {
            format!("Overridden by user config: {}", file.display())
        }
        ConfigLayerSource::LegacyManagedConfigTomlFromFile { file } => {
            format!(
                "Overridden by legacy managed_config.toml: {}",
                file.display()
            )
        }
        ConfigLayerSource::LegacyManagedConfigTomlFromMdm => {
            "Overridden by legacy managed configuration from MDM".to_string()
        }
    }
}

fn compute_override_metadata(
    layers: &ConfigLayerStack,
    effective: &TomlValue,
    segments: &[String],
) -> Option<OverriddenMetadata> {
    let user_value = match layers.get_active_user_layer() {
        Some(user_layer) => value_at_semantic_path(&user_layer.config, segments),
        None => return None,
    };
    let effective_value = value_at_semantic_path(effective, segments);

    if user_value.is_some() && user_value == effective_value {
        return None;
    }

    if user_value.is_none() && effective_value.is_none() {
        return None;
    }

    let overriding_layer = find_effective_layer(layers, segments)?;
    let message = override_message(&overriding_layer.name);

    Some(OverriddenMetadata {
        message,
        overriding_layer: config_layer_metadata_to_api(overriding_layer),
        effective_value: effective_value
            .and_then(|value| serde_json::to_value(value).ok())
            .unwrap_or(JsonValue::Null),
    })
}

fn first_overridden_edit(
    layers: &ConfigLayerStack,
    effective: &TomlValue,
    edits: &[Vec<String>],
) -> Option<OverriddenMetadata> {
    for segments in edits {
        if let Some(meta) = compute_override_metadata(layers, effective, segments) {
            return Some(meta);
        }
    }
    None
}

fn find_effective_layer(
    layers: &ConfigLayerStack,
    segments: &[String],
) -> Option<ConfigLayerMetadata> {
    for layer in layers.layers_high_to_low() {
        if value_at_semantic_path(&layer.config, segments).is_some() {
            return Some(layer.metadata());
        }

        let Some(layer_representation) = layer
            .config
            .get("shell_environment_policy")
            .and_then(ShellEnvironmentPolicyFilterRepresentation::from_policy)
        else {
            continue;
        };
        if ShellEnvironmentPolicyFilterRepresentation::from_path(segments)
            .is_some_and(|edit_representation| edit_representation != layer_representation)
        {
            return Some(layer.metadata());
        }
    }

    None
}

#[cfg(test)]
#[path = "config_manager_service_tests.rs"]
mod tests;
