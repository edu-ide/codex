//! Atomic runtime generation installation and interprocess locking.

use super::ILHAE_CODEX_RUNTIME_CONFIG_LKG_FILE;
use super::ILHAE_CODEX_RUNTIME_CONFIG_LOCK;
use super::ILHAE_CODEX_RUNTIME_CONFIG_LOCK_FILE;
use super::ILHAE_CODEX_RUNTIME_CONFIG_LOCK_POLL_INTERVAL;
use super::ILHAE_CODEX_RUNTIME_CONFIG_LOCK_TIMEOUT;
use super::catalog::absolute_native_model_catalog_path;
use super::catalog::serialize_native_model_catalog;
use super::catalog::transition_native_model_catalog;
use super::schema::validate_ilhae_codex_runtime_config;
use std::fs::File;
use std::fs::OpenOptions;
use std::fs::TryLockError as FileTryLockError;
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::MutexGuard;
use std::sync::TryLockError;
use std::time::Instant;
use tracing::warn;

pub(super) struct IlhaeCodexRuntimeConfigLockGuard {
    _process: MutexGuard<'static, ()>,
    _file: File,
}

pub(super) fn acquire_ilhae_codex_runtime_config_lock(
    codex_home: &Path,
) -> Result<Option<IlhaeCodexRuntimeConfigLockGuard>, String> {
    let deadline = Instant::now() + ILHAE_CODEX_RUNTIME_CONFIG_LOCK_TIMEOUT;
    let process_guard = loop {
        match ILHAE_CODEX_RUNTIME_CONFIG_LOCK.try_lock() {
            Ok(guard) => break guard,
            Err(TryLockError::Poisoned(poisoned)) => break poisoned.into_inner(),
            Err(TryLockError::WouldBlock) => {
                if Instant::now() >= deadline {
                    return Ok(None);
                }
                std::thread::sleep(ILHAE_CODEX_RUNTIME_CONFIG_LOCK_POLL_INTERVAL);
            }
        }
    };

    let lock_path = codex_home.join(ILHAE_CODEX_RUNTIME_CONFIG_LOCK_FILE);
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    options.mode(0o600);
    let file = options.open(&lock_path).map_err(|error| {
        format!(
            "Failed to open Codex runtime config lock ({}): {error}",
            lock_path.display()
        )
    })?;
    #[cfg(unix)]
    file.set_permissions(std::fs::Permissions::from_mode(0o600))
        .map_err(|error| {
            format!(
                "Failed to secure Codex runtime config lock ({}): {error}",
                lock_path.display()
            )
        })?;

    loop {
        match file.try_lock() {
            Ok(()) => {
                return Ok(Some(IlhaeCodexRuntimeConfigLockGuard {
                    _process: process_guard,
                    _file: file,
                }));
            }
            Err(FileTryLockError::WouldBlock) => {
                if Instant::now() >= deadline {
                    return Ok(None);
                }
                std::thread::sleep(ILHAE_CODEX_RUNTIME_CONFIG_LOCK_POLL_INTERVAL);
            }
            Err(FileTryLockError::Error(error)) => {
                return Err(format!(
                    "Failed to acquire Codex runtime config lock ({}): {error}",
                    lock_path.display()
                ));
            }
        }
    }
}

pub(super) fn write_ilhae_codex_runtime_file_atomically(
    destination: &Path,
    bytes: &[u8],
) -> Result<(), String> {
    let parent = destination.parent().ok_or_else(|| {
        format!(
            "Codex runtime config path has no parent: {}",
            destination.display()
        )
    })?;
    std::fs::create_dir_all(parent).map_err(|error| {
        format!(
            "Failed to create Codex runtime config directory ({}): {error}",
            parent.display()
        )
    })?;
    let temporary_path = parent.join(format!(
        ".ilhae-runtime.{}.{}.tmp",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    ));
    let result = (|| -> Result<(), String> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut temporary = options.open(&temporary_path).map_err(|error| {
            format!(
                "Failed to create Codex runtime config temporary file ({}): {error}",
                temporary_path.display()
            )
        })?;
        temporary.write_all(bytes).map_err(|error| {
            format!(
                "Failed to write Codex runtime config temporary file ({}): {error}",
                temporary_path.display()
            )
        })?;
        temporary.sync_all().map_err(|error| {
            format!(
                "Failed to sync Codex runtime config temporary file ({}): {error}",
                temporary_path.display()
            )
        })?;
        drop(temporary);
        replace_ilhae_codex_runtime_file(&temporary_path, destination).map_err(|error| {
            format!(
                "Failed to atomically replace Codex runtime config ({}): {error}",
                destination.display()
            )
        })?;
        OpenOptions::new()
            .write(true)
            .open(destination)
            .and_then(|file| file.sync_all())
            .map_err(|error| {
                format!(
                    "Failed to sync Codex runtime config ({}): {error}",
                    destination.display()
                )
            })?;
        sync_ilhae_codex_runtime_config_parent(parent)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary_path);
    }
    result
}

#[cfg(unix)]
pub(super) fn sync_ilhae_codex_runtime_config_parent(parent: &Path) -> Result<(), String> {
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| {
            format!(
                "Failed to sync Codex runtime config directory ({}): {error}",
                parent.display()
            )
        })
}

#[cfg(not(unix))]
pub(super) fn sync_ilhae_codex_runtime_config_parent(_parent: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(not(windows))]
pub(super) fn replace_ilhae_codex_runtime_file(
    temporary: &Path,
    destination: &Path,
) -> std::io::Result<()> {
    std::fs::rename(temporary, destination)
}

#[cfg(windows)]
pub(super) fn replace_ilhae_codex_runtime_file(
    temporary: &Path,
    destination: &Path,
) -> std::io::Result<()> {
    if !destination.exists() {
        return std::fs::rename(temporary, destination);
    }

    use std::os::windows::ffi::OsStrExt;
    use std::ptr;

    #[link(name = "Kernel32")]
    unsafe extern "system" {
        fn ReplaceFileW(
            replaced_file_name: *const u16,
            replacement_file_name: *const u16,
            backup_file_name: *const u16,
            replace_flags: u32,
            exclude: *mut std::ffi::c_void,
            reserved: *mut std::ffi::c_void,
        ) -> i32;
    }

    let destination_wide = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let temporary_wide = temporary
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let replaced = unsafe {
        ReplaceFileW(
            destination_wide.as_ptr(),
            temporary_wide.as_ptr(),
            ptr::null(),
            0,
            ptr::null_mut(),
            ptr::null_mut(),
        )
    };
    if replaced == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub(super) fn install_ilhae_codex_runtime_snapshot_locked(
    codex_home: &Path,
    rendered: &str,
) -> Result<(), String> {
    validate_ilhae_codex_runtime_config(rendered)?;
    let lkg_path = codex_home.join(ILHAE_CODEX_RUNTIME_CONFIG_LKG_FILE);
    let config_path = codex_home.join("config.toml");
    write_ilhae_codex_runtime_file_atomically(&config_path, rendered.as_bytes())?;
    write_ilhae_codex_runtime_file_atomically(&lkg_path, rendered.as_bytes())
}

pub(super) fn install_ilhae_codex_runtime_generation_locked(
    codex_home: &Path,
    rendered: &str,
    model_catalog: Option<&codex_protocol::openai_models::ModelsResponse>,
) -> Result<(), String> {
    let catalog_path = absolute_native_model_catalog_path(codex_home)?;
    let Some(model_catalog) = model_catalog else {
        install_ilhae_codex_runtime_snapshot_locked(codex_home, rendered)?;
        match std::fs::remove_file(&catalog_path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => warn!(
                "Failed to remove unused native model catalog ({}): {error}",
                catalog_path.display()
            ),
        }
        return Ok(());
    };

    // First publish the union of the old and new catalogs. Both the currently
    // active config and the incoming config remain loadable if the process is
    // interrupted between the independently atomic file replacements.
    let transition_catalog = transition_native_model_catalog(&catalog_path, model_catalog);
    let transition_bytes = serialize_native_model_catalog(&transition_catalog)?;
    write_ilhae_codex_runtime_file_atomically(&catalog_path, &transition_bytes)?;
    install_ilhae_codex_runtime_snapshot_locked(codex_home, rendered)?;

    // Active and LKG now describe the same generation, so the catalog can be
    // pruned back to exactly the native models present in that generation.
    let current_bytes = serialize_native_model_catalog(model_catalog)?;
    if let Err(error) = write_ilhae_codex_runtime_file_atomically(&catalog_path, &current_bytes) {
        warn!("Kept the compatible transition model catalog after final pruning failed: {error}");
    }
    Ok(())
}
