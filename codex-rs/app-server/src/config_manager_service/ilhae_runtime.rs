//! Ilhae's generated-config transaction boundary.
//!
//! The upstream service owns parsing, validation, and config persistence. This
//! adapter owns the shared desktop lock, external-writer detection, protected
//! projections, and MCP ownership sidecars. Hold the transaction until the service
//! has computed its final response so the lock spans the complete edit operation.

use super::ConfigManagerError;
use super::value_at_path;
use codex_app_server_protocol::ConfigWriteErrorCode;
use codex_utils_absolute_path::AbsolutePathBuf;
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
use tokio::task;
use toml::Value as TomlValue;

mod ownership;

const ILHAE_RUNTIME_CONFIG_LOCK_FILE: &str = ".config.toml.ilhae-runtime.lock";
const ILHAE_RUNTIME_CONFIG_LOCK_TIMEOUT: Duration = Duration::from_secs(3);
const ILHAE_RUNTIME_CONFIG_LOCK_POLL_INTERVAL: Duration = Duration::from_millis(10);
const ILHAE_RUNTIME_SYSTEM2_PROJECTION_KEY: &str = "ilhae_runtime_system2_projection";
const ILHAE_RUNTIME_CONFIG_LOCK_CONTENTION_MESSAGE: &str =
    "Runtime configuration is being updated; retry this edit.";

/// A generated-config edit, optionally coordinated with the Ilhae desktop.
pub(super) struct RuntimeConfigTransaction {
    codex_home: PathBuf,
    lock: Option<File>,
    snapshot: Option<Option<Vec<u8>>>,
}

impl RuntimeConfigTransaction {
    pub(super) async fn begin(codex_home: &Path) -> Result<Self, ConfigManagerError> {
        let lock = acquire_ilhae_runtime_config_lock_if_enabled(codex_home.to_path_buf()).await?;
        Ok(Self {
            codex_home: codex_home.to_path_buf(),
            lock,
            snapshot: None,
        })
    }

    pub(super) async fn capture(
        &mut self,
        config_path: &AbsolutePathBuf,
        original: &TomlValue,
    ) -> Result<(), ConfigManagerError> {
        if self.lock.is_some() {
            self.snapshot = Some(capture_runtime_config_cas_snapshot(config_path, original).await?);
        }
        Ok(())
    }

    pub(super) async fn before_persist(
        &self,
        config_path: &AbsolutePathBuf,
        original: &TomlValue,
        updated: &TomlValue,
    ) -> Result<(), ConfigManagerError> {
        let changed_servers = ownership::changed_runtime_mcp_servers(original, updated);
        let ownership_write = if self.lock.is_some() && !changed_servers.is_empty() {
            ownership::prepare_runtime_mcp_ownership_write(
                &self.codex_home,
                original,
                updated,
                &changed_servers,
            )?
        } else {
            None
        };
        if let Some(expected_snapshot) = self.snapshot.as_ref() {
            ensure_runtime_config_cas_snapshot_unchanged(config_path, expected_snapshot).await?;
        }
        if let Some(write) = ownership_write {
            ownership::persist_runtime_mcp_ownership_cas(self.codex_home.clone(), write).await?;
        }
        Ok(())
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

pub(super) fn reject_direct_runtime_projection_path(
    segments: &[String],
) -> Result<(), ConfigManagerError> {
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

pub(super) fn ensure_runtime_projection_unchanged(
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

#[cfg(test)]
#[path = "ilhae_runtime_tests.rs"]
mod tests;
