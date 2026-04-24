use regex::Regex;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use tokio::sync::broadcast;

pub use crate::settings_types::*;

pub struct SettingsStore {
    settings_path: PathBuf,
    ilhae_dir: PathBuf,
    settings: Arc<RwLock<Settings>>,
    persist_on_set: bool,
    pub event_tx: broadcast::Sender<SettingsEvent>,
}

impl SettingsStore {
    pub fn new(ilhae_dir: &Path) -> Self {
        let settings_path = ilhae_dir
            .join("brain")
            .join("settings")
            .join("app_settings.json");
        let settings = load_settings(&settings_path);
        let (event_tx, _) = broadcast::channel(64);
        Self {
            settings_path,
            ilhae_dir: ilhae_dir.to_path_buf(),
            settings: Arc::new(RwLock::new(settings)),
            persist_on_set: true,
            event_tx,
        }
    }

    pub fn new_with_snapshot(ilhae_dir: &Path, settings: Settings) -> Self {
        let settings_path = ilhae_dir
            .join("brain")
            .join("settings")
            .join("app_settings.runtime_overlay.json");
        let (event_tx, _) = broadcast::channel(64);
        Self {
            settings_path,
            ilhae_dir: ilhae_dir.to_path_buf(),
            settings: Arc::new(RwLock::new(settings)),
            persist_on_set: false,
            event_tx,
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<SettingsEvent> {
        self.event_tx.subscribe()
    }

    pub fn get(&self) -> Settings {
        self.settings
            .read()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }

    pub fn get_value(&self, key: &str) -> Value {
        let snapshot = self.get();
        let value = serde_json::to_value(snapshot).unwrap_or(Value::Null);
        get_json_path(&value, key).cloned().unwrap_or(Value::Null)
    }

    pub fn set_value(&self, key: &str, value: Value) -> Result<(), String> {
        let mut json = serde_json::to_value(self.get()).map_err(|e| e.to_string())?;
        set_json_path(&mut json, key, value.clone())?;
        let mut settings: Settings = serde_json::from_value(json).map_err(|e| e.to_string())?;
        migrate_legacy_settings(&mut settings);
        if self.persist_on_set {
            persist_settings(&self.settings_path, &settings)?;
        }

        if let Ok(mut guard) = self.settings.write() {
            *guard = settings;
        }

        let _ = self.event_tx.send(SettingsEvent {
            key: key.to_string(),
            value,
        });
        Ok(())
    }

    pub fn read_brain_mcp_json(&self) -> String {
        fs::read_to_string(self.brain_mcp_json_path()).unwrap_or_else(|_| "{}".to_string())
    }

    pub fn write_brain_mcp_json(&self, content: &str) -> Result<(), String> {
        let path = self.brain_mcp_json_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        fs::write(path, content).map_err(|e| e.to_string())
    }

    pub fn sync_to_brain_mcp_json(&self) {
        let presets = self.get().mcp.presets;
        if let Ok(content) =
            serde_json::to_string_pretty(&serde_json::json!({ "presets": presets }))
        {
            let _ = self.write_brain_mcp_json(&content);
        }
    }

    pub fn emit_current_value(&self, key: &str) {
        let _ = self.event_tx.send(SettingsEvent {
            key: key.to_string(),
            value: self.get_value(key),
        });
    }

    pub fn check_allowlist(&self, tool_title: &str) -> Option<(String, String)> {
        for policy in self.get().permissions.policies {
            let pattern = policy.pattern.trim();
            if pattern.is_empty() {
                continue;
            }
            let matches = Regex::new(pattern)
                .map(|re| re.is_match(tool_title))
                .unwrap_or_else(|_| {
                    tool_title.starts_with(pattern) || tool_title.contains(pattern)
                });
            if !matches {
                continue;
            }
            let kind = if policy.kind.trim().is_empty() {
                "allow_always".to_string()
            } else {
                policy.kind.clone()
            };
            let option_id = if policy.option_id.trim().is_empty() {
                match kind.as_str() {
                    "reject_always" | "deny_always" => "reject_always".to_string(),
                    _ => "allow_always".to_string(),
                }
            } else {
                policy.option_id.clone()
            };
            return Some((option_id, kind));
        }
        None
    }

    pub fn is_full_access(&self) -> bool {
        self.get().permissions.approval_preset == "full-access"
    }

    fn brain_mcp_json_path(&self) -> PathBuf {
        let active_vault = crate::config::get_active_vault_dir();
        if active_vault.is_absolute() {
            active_vault.join("mcp.json")
        } else {
            self.ilhae_dir.join(active_vault).join("mcp.json")
        }
    }
}

fn load_settings(settings_path: &Path) -> Settings {
    let mut settings = fs::read_to_string(settings_path)
        .ok()
        .and_then(|content| serde_json::from_str::<Settings>(&content).ok())
        .unwrap_or_default();
    migrate_legacy_settings(&mut settings);
    settings
}

fn persist_settings(settings_path: &Path, settings: &Settings) -> Result<(), String> {
    if let Some(parent) = settings_path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let content = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    fs::write(settings_path, content).map_err(|e| e.to_string())
}

fn migrate_legacy_settings(settings: &mut Settings) {
    if let Some(telegram) = settings.telegram.take()
        && settings.channels.telegram.bot_token.is_empty()
        && settings.channels.telegram.allowed_chat_ids.is_empty()
    {
        settings.channels.telegram = telegram;
    }
}

fn get_json_path<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    let mut current = value;
    for segment in key.split('.') {
        current = current.get(segment)?;
    }
    Some(current)
}

fn set_json_path(root: &mut Value, key: &str, value: Value) -> Result<(), String> {
    let parts: Vec<&str> = key.split('.').collect();
    if parts.is_empty() {
        return Err("empty settings key".to_string());
    }

    let mut current = root;
    for segment in &parts[..parts.len() - 1] {
        let obj = current
            .as_object_mut()
            .ok_or_else(|| format!("settings path '{key}' is not an object"))?;
        current = obj
            .entry((*segment).to_string())
            .or_insert_with(|| Value::Object(serde_json::Map::new()));
    }

    let obj = current
        .as_object_mut()
        .ok_or_else(|| format!("settings path '{key}' parent is not an object"))?;
    obj.insert(parts[parts.len() - 1].to_string(), value);
    Ok(())
}
