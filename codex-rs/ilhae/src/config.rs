use std::path::{Path, PathBuf};
use tracing::{info, warn};

/// Resolve the ilhae data directory (~⁄ilhae), using the generic name.
pub fn resolve_ilhae_data_dir() -> PathBuf {
    if let Ok(from_env) = std::env::var("ILHAE_DATA_DIR") {
        let trimmed = from_env.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }

    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    let data_dir = home.join("ilhae");
    let legacy_dir = home.join(crate::helpers::ILHAE_DIR_NAME);

    if legacy_dir.exists() {
        let _ = std::fs::create_dir_all(&data_dir);
        if let Ok(entries) = std::fs::read_dir(&legacy_dir) {
            for entry in entries.flatten() {
                let dest = data_dir.join(entry.file_name());
                if !dest.exists() && entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                    info!("Migrating {:?} → {:?}", entry.path(), dest);
                    let _ = std::fs::copy(entry.path(), &dest);
                }
            }
        }
    }

    data_dir
}

#[derive(serde::Deserialize)]
struct MinimalSettings {
    #[serde(default)]
    vault: VaultConfig,
}

#[derive(serde::Deserialize, Default)]
struct VaultConfig {
    #[serde(default)]
    active_vault: Option<String>,
}

/// Get the currently active vault directory path.
/// Defaults to `~/ilhae/brain` if not configured.
pub fn get_active_vault_dir() -> PathBuf {
    let ilhae_dir = resolve_ilhae_data_dir();
    let default_vault = ilhae_dir.join("brain");

    let settings_path = ilhae_dir
        .join("brain")
        .join("settings")
        .join("app_settings.json");
    if let Ok(content) = std::fs::read_to_string(&settings_path) {
        if let Ok(settings) = serde_json::from_str::<MinimalSettings>(&content) {
            if let Some(active) = settings.vault.active_vault {
                if !active.trim().is_empty() {
                    return PathBuf::from(active);
                }
            }
        }
    }
    default_vault
}

/// Copy codex auth files from ~/.codex to the workspace CODEX_HOME directory.
pub fn sync_codex_auth_to_workspace(home: &str, workspace: &PathBuf) {
    let source_dir = PathBuf::from(home).join(".codex");
    if !source_dir.exists() {
        return;
    }

    if let Err(err) = std::fs::create_dir_all(workspace) {
        warn!(
            "Failed to create CODEX_HOME workspace directory {:?}: {}",
            workspace, err
        );
        return;
    }

    for file in ["auth.json", "config.toml", ".credentials.json"] {
        let src = source_dir.join(file);
        if !src.exists() {
            continue;
        }
        let dst = workspace.join(file);
        if let Err(err) = std::fs::copy(&src, &dst) {
            warn!("Failed to copy {:?} -> {:?}: {}", src, dst, err);
        }
    }
}

/// Build the context prefix from IDENTITY.md, SOUL.md, USER.md, and memory/global/ folder files.
pub fn build_context_prefix(_ilhae_dir_unused: &Path) -> String {
    let vault_dir = get_active_vault_dir();
    let global_dir = vault_dir.join("memory").join("global");
    let legacy_context_dir = vault_dir.join("context"); // legacy fallback
    let ilhae_dir = resolve_ilhae_data_dir();

    // Core identity files: memory/global/ > legacy context/ > active vault/ > ilhae root
    let read_with_fallback = |name: &str| -> String {
        std::fs::read_to_string(global_dir.join(name))
            .or_else(|_| std::fs::read_to_string(legacy_context_dir.join(name)))
            .or_else(|_| std::fs::read_to_string(vault_dir.join(name)))
            .or_else(|_| std::fs::read_to_string(ilhae_dir.join(name)))
            .unwrap_or_default()
    };

    let system = read_with_fallback("SYSTEM.md");
    let identity = read_with_fallback("IDENTITY.md");
    let soul = read_with_fallback("SOUL.md");
    let user = read_with_fallback("USER.md");

    // Collect additional memory/global/ folder .md files (exclude core + README)
    let mut context_parts = Vec::new();
    for dir in [&global_dir, &legacy_context_dir] {
        if dir.is_dir() {
            if let Ok(entries) = std::fs::read_dir(dir) {
                let excluded = [
                    "SYSTEM.md",
                    "IDENTITY.md",
                    "SOUL.md",
                    "USER.md",
                    "README.md",
                ];
                let mut paths: Vec<_> = entries
                    .flatten()
                    .filter(|e| {
                        let name = e.file_name().to_string_lossy().to_string();
                        name.ends_with(".md") && !excluded.contains(&name.as_str())
                    })
                    .collect();
                paths.sort_by_key(|e| e.file_name());
                for entry in paths {
                    if let Ok(content) = std::fs::read_to_string(entry.path()) {
                        if !content.trim().is_empty() {
                            context_parts.push(content);
                        }
                    }
                }
            }
        }
    }

    let context_section = if context_parts.is_empty() {
        String::new()
    } else {
        format!("\n### CONTEXT\n{}\n", context_parts.join("\n---\n"))
    };

    let system_section = if system.trim().is_empty() {
        String::new()
    } else {
        format!("### SYSTEM\n{}\n", system)
    };

    format!(
        "\n<agent_context>\n{}### IDENTITY\n{}\n### SOUL\n{}\n### USER\n{}{}\n</agent_context>\n\n",
        system_section, identity, soul, user, context_section
    )
}
