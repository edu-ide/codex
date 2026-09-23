//! Codex adapter: translate human intent and install validated runtime generations.
//! Product policy remains in the parent module; upstream schema dependencies stay here.

use super::IlhaeTomlConfig;
use super::resolve_ilhae_codex_home_dir;
use super::resolve_ilhae_config_dir;
use super::resolve_ilhae_config_toml_path;
use super::resolve_ilhae_data_dir;
use std::path::Path;
use std::path::PathBuf;
use tracing::warn;
mod catalog;
mod mcp;
mod projection;
mod providers;
mod schema;
mod storage;

use self::catalog::absolute_native_model_catalog_path;
use self::catalog::native_model_catalog;
use self::projection::default_ilhae_codex_home_table;
use self::schema::read_valid_ilhae_codex_runtime_config;
use self::schema::read_valid_ilhae_codex_runtime_config_with_system2;
use self::schema::validate_ilhae_codex_runtime_config;
use self::storage::acquire_ilhae_codex_runtime_config_lock;
use self::storage::install_ilhae_codex_runtime_generation_locked;
use self::storage::write_ilhae_codex_runtime_file_atomically;

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::sync::Mutex;
use std::time::Duration;

pub const ILHAE_CODEX_RUNTIME_CONFIG_LKG_FILE: &str = ".config.toml.ilhae-lkg";
pub const ILHAE_CODEX_RUNTIME_CONFIG_LOCK_FILE: &str = ".config.toml.ilhae-runtime.lock";
pub(super) const ILHAE_CODEX_MODEL_CATALOG_FILE: &str = "model_catalog.json";
pub(super) const ILHAE_RUNTIME_SYSTEM2_PROJECTION_KEY: &str = "ilhae_runtime_system2_projection";
pub(super) const ILHAE_RUNTIME_SYSTEM2_PROJECTION_SCHEMA_VERSION: i64 = 1;
pub(super) const RETIRED_EXCEL_MCP_SERVER_NAME: &str = "excel-mcp";
pub(super) const ILHAE_CODEX_RUNTIME_CONFIG_LOCK_TIMEOUT: Duration = Duration::from_secs(5);
pub(super) const ILHAE_CODEX_RUNTIME_CONFIG_LOCK_POLL_INTERVAL: Duration =
    Duration::from_millis(10);
pub(super) static ILHAE_CODEX_RUNTIME_CONFIG_LOCK: Mutex<()> = Mutex::new(());

pub(super) fn comparable_config_path(path: &Path) -> Option<PathBuf> {
    if path.exists() {
        return std::fs::canonicalize(path).ok();
    }
    let parent = path.parent()?;
    let file_name = path.file_name()?;
    std::fs::canonicalize(parent)
        .ok()
        .map(|canonical_parent| canonical_parent.join(file_name))
}

pub(super) fn paths_reference_same_config_file(left: &Path, right: &Path) -> bool {
    if comparable_config_path(left)
        .zip(comparable_config_path(right))
        .is_some_and(|(left, right)| left == right)
    {
        return true;
    }

    #[cfg(unix)]
    {
        if let (Ok(left), Ok(right)) = (std::fs::metadata(left), std::fs::metadata(right)) {
            return left.dev() == right.dev() && left.ino() == right.ino();
        }
    }

    false
}

pub(super) fn runtime_home_aliases_human_config(runtime_home: &Path) -> bool {
    let runtime_config = runtime_home.join("config.toml");
    let primary_source = resolve_ilhae_config_toml_path();
    paths_reference_same_config_file(&primary_source, &runtime_config)
        || resolve_human_ilhae_config_path()
            .filter(|source| source != &primary_source)
            .is_some_and(|source| paths_reference_same_config_file(&source, &runtime_config))
}

/// Human intent and the validated Codex runtime snapshot must never share a
/// file identity. If an explicit home aliases the human config directory (even
/// through a symlink or hardlink), move only the generated runtime state into a
/// stable child directory and leave the source bytes untouched.
pub(super) fn resolve_non_aliasing_ilhae_codex_home_dir() -> Result<PathBuf, String> {
    let requested = resolve_ilhae_codex_home_dir();
    if !runtime_home_aliases_human_config(&requested) {
        return Ok(requested);
    }

    let isolated = if requested.is_file() {
        requested
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(".ilhae-runtime-home")
    } else {
        requested.join(".ilhae-runtime-home")
    };
    if runtime_home_aliases_human_config(&isolated) {
        return Err("Human configuration and Codex runtime storage could not be isolated".into());
    }
    warn!("Separated human configuration from generated Codex runtime storage");
    Ok(isolated)
}

#[derive(Debug)]
pub(super) struct HumanIlhaeConfigSnapshot {
    typed: IlhaeTomlConfig,
    document: toml::Value,
}

impl Default for HumanIlhaeConfigSnapshot {
    fn default() -> Self {
        Self {
            typed: IlhaeTomlConfig::default(),
            document: toml::Value::Table(toml::value::Table::new()),
        }
    }
}

pub(super) fn resolve_human_ilhae_config_path() -> Option<PathBuf> {
    let primary = resolve_ilhae_config_toml_path();
    if primary.exists() {
        return Some(primary);
    }
    let legacy = resolve_ilhae_data_dir().join("config.toml");
    legacy.exists().then_some(legacy)
}

/// Reads the human-managed source exactly once. Projection always derives from
/// this immutable generation, so a concurrent editor cannot mix MCP, model,
/// feature, and profile values from different file generations.
pub(super) fn load_human_ilhae_config_snapshot() -> Result<HumanIlhaeConfigSnapshot, String> {
    let Some(path) = resolve_human_ilhae_config_path() else {
        return Ok(HumanIlhaeConfigSnapshot::default());
    };
    let bytes = std::fs::read(&path).map_err(|error| {
        format!(
            "Failed to read human-managed Ilhae config snapshot ({}): {error}",
            path.display()
        )
    })?;
    let content = std::str::from_utf8(&bytes).map_err(|error| {
        format!(
            "Human-managed Ilhae config is not valid UTF-8 ({}): {error}",
            path.display()
        )
    })?;
    let document = content.parse::<toml::Value>().map_err(|error| {
        format!(
            "Human-managed Ilhae config is not valid TOML ({}): {error}",
            path.display()
        )
    })?;
    let typed = document
        .clone()
        .try_into::<IlhaeTomlConfig>()
        .map_err(|error| {
            format!(
                "Human-managed Ilhae config has an invalid typed shape ({}): {error}",
                path.display()
            )
        })?;
    Ok(HumanIlhaeConfigSnapshot { typed, document })
}

pub(super) fn render_ilhae_codex_runtime_candidate(
    snapshot: &HumanIlhaeConfigSnapshot,
    model_catalog_path: &Path,
) -> Result<String, String> {
    let root =
        default_ilhae_codex_home_table(&snapshot.typed, &snapshot.document, model_catalog_path);
    let rendered =
        toml::to_string_pretty(&toml::Value::Table(root)).map_err(|error| error.to_string())?;
    validate_ilhae_codex_runtime_config(&rendered)?;
    Ok(rendered)
}

pub(super) fn preserve_valid_active_runtime_locked(codex_home: &Path) -> bool {
    let config_path = codex_home.join("config.toml");
    read_valid_ilhae_codex_runtime_config(&config_path).is_some()
}

pub(super) fn restore_valid_runtime_lkg_locked(codex_home: &Path) -> Result<bool, String> {
    let lkg_path = codex_home.join(ILHAE_CODEX_RUNTIME_CONFIG_LKG_FILE);
    let Some(lkg) = read_valid_ilhae_codex_runtime_config(&lkg_path) else {
        return Ok(false);
    };
    write_ilhae_codex_runtime_file_atomically(&codex_home.join("config.toml"), &lkg)?;
    Ok(true)
}

pub(super) fn recover_or_bootstrap_ilhae_codex_runtime_locked(
    codex_home: &Path,
    reason: &str,
) -> Result<(), String> {
    warn!("{reason}");
    if preserve_valid_active_runtime_locked(codex_home) {
        return Ok(());
    }
    if restore_valid_runtime_lkg_locked(codex_home)? {
        return Ok(());
    }

    let model_catalog_path = absolute_native_model_catalog_path(codex_home)?;
    let safe = render_ilhae_codex_runtime_candidate(
        &HumanIlhaeConfigSnapshot::default(),
        &model_catalog_path,
    )
    .map_err(|error| format!("Failed to build safe Codex runtime bootstrap: {error}"))?;
    install_ilhae_codex_runtime_generation_locked(codex_home, &safe, None)
}

pub(super) fn prepare_ilhae_codex_runtime_config_locked(codex_home: &Path) -> Result<(), String> {
    let snapshot = match load_human_ilhae_config_snapshot() {
        Ok(snapshot) => snapshot,
        Err(_) => {
            recover_or_bootstrap_ilhae_codex_runtime_locked(
                codex_home,
                "Human configuration was unreadable; keeping a validated Codex runtime snapshot",
            )?;
            return Ok(());
        }
    };
    let model_catalog_path = absolute_native_model_catalog_path(codex_home)?;
    let candidate = match render_ilhae_codex_runtime_candidate(&snapshot, &model_catalog_path) {
        Ok(candidate) => candidate,
        Err(_) => {
            recover_or_bootstrap_ilhae_codex_runtime_locked(
                codex_home,
                "Ignored invalid human configuration overrides; keeping a validated Codex runtime snapshot",
            )?;
            return Ok(());
        }
    };

    let model_catalog = native_model_catalog(&snapshot.typed, &snapshot.document);
    if let Err(error) = install_ilhae_codex_runtime_generation_locked(
        codex_home,
        &candidate,
        model_catalog.as_ref(),
    ) {
        if preserve_valid_active_runtime_locked(codex_home) {
            warn!(
                "Keeping the previous validated Codex runtime snapshot after install failure: {error}"
            );
            return Ok(());
        }
        if restore_valid_runtime_lkg_locked(codex_home)? {
            warn!("Restored the validated Codex runtime LKG after install failure: {error}");
            return Ok(());
        }
        return Err(error);
    }
    Ok(())
}

pub fn prepare_ilhae_codex_home() -> Result<PathBuf, String> {
    let codex_home = resolve_non_aliasing_ilhae_codex_home_dir()?;
    std::fs::create_dir_all(&codex_home).map_err(|err| err.to_string())?;

    let runtime_system2 = {
        match acquire_ilhae_codex_runtime_config_lock(&codex_home)? {
            Some(_guard) => {
                prepare_ilhae_codex_runtime_config_locked(&codex_home)?;
                let config_path = codex_home.join("config.toml");
                read_valid_ilhae_codex_runtime_config_with_system2(&config_path)
                    .map(|(_, system2)| system2)
                    .ok_or_else(|| {
                        format!(
                            "The prepared Codex runtime snapshot is not valid ({})",
                            config_path.display()
                        )
                    })?
            }
            None => {
                let config_path = codex_home.join("config.toml");
                let (_, system2) = read_valid_ilhae_codex_runtime_config_with_system2(&config_path)
                    .ok_or_else(|| {
                        format!(
                            "Timed out waiting for the Codex runtime config lock and the latest active snapshot is not valid ({})",
                            config_path.display()
                        )
                    })?;
                warn!(
                    "Timed out waiting for the Codex runtime config lock; using the latest validated active snapshot"
                );
                system2
            }
        }
    };
    let _ = std::fs::remove_file(codex_home.join("managed_config.toml"));

    let ilhae_config_dir = resolve_ilhae_config_dir();
    let codex_config_dir = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".codex");
    let auth_source_dirs = [ilhae_config_dir, codex_config_dir];
    for auth_file in ["auth.json", ".credentials.json"] {
        let current_auth = codex_home.join(auth_file);
        if current_auth.exists() {
            continue;
        }
        for source_dir in &auth_source_dirs {
            let source_auth = source_dir.join(auth_file);
            if !source_auth.exists() {
                continue;
            }
            if let Err(err) = std::fs::copy(&source_auth, &current_auth) {
                warn!(
                    "Failed to seed {:?} -> {:?}: {}",
                    source_auth, current_auth, err
                );
            }
            break;
        }
    }

    // SAFETY: ilhae sets CODEX_HOME once during single-threaded CLI startup,
    // before any worker threads or async tasks that could concurrently depend
    // on environment mutation are spawned.
    unsafe {
        std::env::set_var("CODEX_HOME", &codex_home);
        std::env::set_var("ILHAE_RUNTIME", "1");
    }
    // System2 routing is projected from the final validated active snapshot,
    // never from a separately re-read human config generation.
    if let Some(system2) = runtime_system2 {
        unsafe {
            std::env::set_var("ILHAE_SYSTEM2_ENABLED", "1");
            std::env::set_var("ILHAE_SYSTEM2_SOURCE_PROFILE", system2.source_profile_id);
            std::env::set_var("ILHAE_SYSTEM2_PROFILE", system2.target_profile_id);
            std::env::set_var("ILHAE_SYSTEM2_BASE_URL", system2.base_url);
            std::env::set_var("ILHAE_SYSTEM2_MODEL", system2.model_name);
        }
    } else {
        unsafe {
            std::env::remove_var("ILHAE_SYSTEM2_ENABLED");
            std::env::remove_var("ILHAE_SYSTEM2_SOURCE_PROFILE");
            std::env::remove_var("ILHAE_SYSTEM2_PROFILE");
            std::env::remove_var("ILHAE_SYSTEM2_BASE_URL");
            std::env::remove_var("ILHAE_SYSTEM2_MODEL");
        }
    }
    Ok(codex_home)
}

#[cfg(test)]
mod model_tests;
#[cfg(test)]
mod tests;

pub(super) fn codex_model_from_root_config(profile_id: &str) -> Option<String> {
    let config_path = dirs::home_dir().map(|h| h.join(".codex/config.toml"))?;
    if !config_path.exists() {
        return None;
    }

    let config = std::fs::read_to_string(config_path).ok()?;
    let config = config.parse::<toml::Value>().ok()?;

    config
        .get("profiles")
        .and_then(|profiles| profiles.get(profile_id))
        .and_then(|profile| profile.get("model"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .map(str::to_string)
        .filter(|model| !model.is_empty() && !model.eq_ignore_ascii_case("default"))
        .or_else(|| {
            config
                .get("model")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .map(str::to_string)
                .filter(|model| !model.is_empty() && !model.eq_ignore_ascii_case("default"))
        })
}
