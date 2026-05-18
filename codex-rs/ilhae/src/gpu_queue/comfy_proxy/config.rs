use std::path::Path;
use std::path::PathBuf;

const DEFAULT_LISTEN_ADDR: &str = "127.0.0.1:8189";
const DEFAULT_BACKEND_URL: &str = "http://127.0.0.1:8188";
const DEFAULT_COMFY_ROOT: &str = "/mnt/sda1/ComfyUI";
const DEFAULT_OWNER: &str = "comfyui-gateway";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComfyProxyConfig {
    pub listen: String,
    pub backend_url: String,
    pub comfy_root: PathBuf,
    pub gpu_queue_addr: String,
    pub owner: String,
    pub start_command: Option<String>,
    pub stop_command: Option<String>,
    pub ttl_seconds: u64,
    pub wait_timeout_seconds: u64,
    pub prompt_poll_interval_ms: u64,
    pub prompt_timeout_seconds: u64,
    pub stop_after_prompt: bool,
    pub start_backend_for_passthrough: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ComfyProxyConfigOverrides {
    pub listen: Option<String>,
    pub backend_url: Option<String>,
    pub comfy_root: Option<PathBuf>,
    pub gpu_queue_addr: Option<String>,
    pub owner: Option<String>,
    pub start_command: Option<String>,
    pub stop_command: Option<String>,
    pub ttl_seconds: Option<u64>,
    pub wait_timeout_seconds: Option<u64>,
    pub prompt_poll_interval_ms: Option<u64>,
    pub prompt_timeout_seconds: Option<u64>,
    pub stop_after_prompt: Option<bool>,
    pub start_backend_for_passthrough: Option<bool>,
}

impl Default for ComfyProxyConfig {
    fn default() -> Self {
        Self {
            listen: DEFAULT_LISTEN_ADDR.to_string(),
            backend_url: DEFAULT_BACKEND_URL.to_string(),
            comfy_root: PathBuf::from(DEFAULT_COMFY_ROOT),
            gpu_queue_addr: super::super::api::default_listen_addr(),
            owner: DEFAULT_OWNER.to_string(),
            start_command: None,
            stop_command: None,
            ttl_seconds: 3600,
            wait_timeout_seconds: 900,
            prompt_poll_interval_ms: 2000,
            prompt_timeout_seconds: 3600,
            stop_after_prompt: true,
            start_backend_for_passthrough: false,
        }
    }
}

impl ComfyProxyConfig {
    pub fn from_sources(
        config_dir_override: Option<&Path>,
        overrides: ComfyProxyConfigOverrides,
    ) -> Self {
        let mut config = Self::default();
        if let Some(table) = load_comfy_proxy_config_table(config_dir_override) {
            apply_config_table(&mut config, &table);
        }
        apply_env(&mut config);
        apply_overrides(&mut config, overrides);
        config.backend_url = normalize_base_url(&config.backend_url);
        config
    }
}

fn load_comfy_proxy_config_table(config_dir_override: Option<&Path>) -> Option<toml::Table> {
    let config_path = config_dir_override
        .map(|dir| dir.join("config.toml"))
        .unwrap_or_else(crate::config::resolve_ilhae_config_toml_path);
    let content = std::fs::read_to_string(config_path).ok()?;
    let value = content.parse::<toml::Value>().ok()?;
    value.get("comfy_proxy")?.as_table().cloned()
}

fn apply_config_table(config: &mut ComfyProxyConfig, table: &toml::Table) {
    if let Some(value) = table.get("listen").and_then(toml::Value::as_str) {
        config.listen = value.to_string();
    }
    if let Some(value) = table.get("backend_url").and_then(toml::Value::as_str) {
        config.backend_url = value.to_string();
    }
    if let Some(value) = table.get("comfy_root").and_then(toml::Value::as_str) {
        config.comfy_root = PathBuf::from(value);
    }
    if let Some(value) = table.get("gpu_queue_addr").and_then(toml::Value::as_str) {
        config.gpu_queue_addr = value.to_string();
    }
    if let Some(value) = table.get("owner").and_then(toml::Value::as_str) {
        config.owner = value.to_string();
    }
    if let Some(value) = table.get("start_command").and_then(toml::Value::as_str) {
        config.start_command = Some(value.to_string());
    }
    if let Some(value) = table.get("stop_command").and_then(toml::Value::as_str) {
        config.stop_command = Some(value.to_string());
    }
    if let Some(value) = table.get("ttl_seconds").and_then(toml::Value::as_integer)
        && value >= 0
    {
        config.ttl_seconds = value as u64;
    }
    if let Some(value) = table
        .get("wait_timeout_seconds")
        .and_then(toml::Value::as_integer)
        && value >= 0
    {
        config.wait_timeout_seconds = value as u64;
    }
    if let Some(value) = table
        .get("prompt_poll_interval_ms")
        .and_then(toml::Value::as_integer)
        && value >= 0
    {
        config.prompt_poll_interval_ms = value as u64;
    }
    if let Some(value) = table
        .get("prompt_timeout_seconds")
        .and_then(toml::Value::as_integer)
        && value >= 0
    {
        config.prompt_timeout_seconds = value as u64;
    }
    if let Some(value) = table
        .get("stop_after_prompt")
        .and_then(toml::Value::as_bool)
    {
        config.stop_after_prompt = value;
    }
    if let Some(value) = table
        .get("start_backend_for_passthrough")
        .and_then(toml::Value::as_bool)
    {
        config.start_backend_for_passthrough = value;
    }
}

fn apply_env(config: &mut ComfyProxyConfig) {
    if let Ok(value) = std::env::var("ILHAE_COMFY_PROXY_LISTEN") {
        config.listen = value;
    }
    if let Ok(value) = std::env::var("ILHAE_COMFY_PROXY_BACKEND_URL")
        .or_else(|_| std::env::var("COMFYUI_BACKEND_URL"))
    {
        config.backend_url = value;
    }
    if let Ok(value) =
        std::env::var("ILHAE_COMFY_PROXY_ROOT").or_else(|_| std::env::var("COMFYUI_DIR"))
    {
        config.comfy_root = PathBuf::from(value);
    }
    if let Ok(value) = std::env::var("ILHAE_GPU_QUEUE_ADDR") {
        config.gpu_queue_addr = value;
    }
    if let Ok(value) = std::env::var("ILHAE_COMFY_PROXY_OWNER") {
        config.owner = value;
    }
    if let Ok(value) = std::env::var("ILHAE_COMFY_PROXY_START_COMMAND") {
        config.start_command = Some(value);
    }
    if let Ok(value) = std::env::var("ILHAE_COMFY_PROXY_STOP_COMMAND") {
        config.stop_command = Some(value);
    }
}

fn apply_overrides(config: &mut ComfyProxyConfig, overrides: ComfyProxyConfigOverrides) {
    if let Some(value) = overrides.listen {
        config.listen = value;
    }
    if let Some(value) = overrides.backend_url {
        config.backend_url = value;
    }
    if let Some(value) = overrides.comfy_root {
        config.comfy_root = value;
    }
    if let Some(value) = overrides.gpu_queue_addr {
        config.gpu_queue_addr = value;
    }
    if let Some(value) = overrides.owner {
        config.owner = value;
    }
    if let Some(value) = overrides.start_command {
        config.start_command = Some(value);
    }
    if let Some(value) = overrides.stop_command {
        config.stop_command = Some(value);
    }
    if let Some(value) = overrides.ttl_seconds {
        config.ttl_seconds = value;
    }
    if let Some(value) = overrides.wait_timeout_seconds {
        config.wait_timeout_seconds = value;
    }
    if let Some(value) = overrides.prompt_poll_interval_ms {
        config.prompt_poll_interval_ms = value;
    }
    if let Some(value) = overrides.prompt_timeout_seconds {
        config.prompt_timeout_seconds = value;
    }
    if let Some(value) = overrides.stop_after_prompt {
        config.stop_after_prompt = value;
    }
    if let Some(value) = overrides.start_backend_for_passthrough {
        config.start_backend_for_passthrough = value;
    }
}

fn normalize_base_url(value: &str) -> String {
    value.trim().trim_end_matches('/').to_string()
}
