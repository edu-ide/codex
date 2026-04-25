//! Settings type definitions and defaults.
//!
//! Extracted from `settings_store.rs` — pure data types (structs, enums, Default impls)
//! for all ilhae-proxy configuration. No I/O or store logic here.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// All ilhae settings, persisted to `settings.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Browser automation
    pub browser: BrowserSettings,
    /// Permission / YOLO mode
    pub permissions: PermissionSettings,
    /// MCP server connection config
    pub mcp: McpSettings,
    /// Agent engine selection
    pub agent: AgentSettings,
    /// ASR (speech-to-text) gateway connection
    pub asr: AsrSettings,
    /// Installed plugins
    pub plugins: HashMap<String, bool>,
    /// UI specific settings
    pub ui: UiSettings,
    /// Built-in tool enabled states (default: all enabled)
    pub builtin_tools: HashMap<String, bool>,
    /// Dashboard daily schedules & categories
    pub dashboard: DashboardSettings,
    /// Chat channel integrations (Telegram, KakaoTalk, Discord, etc.)
    pub channels: ChannelsSettings,
    /// Legacy: kept for backward compat with old settings.json that had telegram at top level
    #[serde(default, skip_serializing)]
    pub telegram: Option<TelegramSettings>,
    /// Session → CWD mapping (redundant with DB but kept for quick access)
    pub session_cwd_map: HashMap<String, String>,
    /// Vault management settings
    pub vault: VaultSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct VaultSettings {
    pub active_vault: String,
    pub vaults: HashMap<String, String>,
}

impl Default for VaultSettings {
    fn default() -> Self {
        let vaults = HashMap::new();
        // default vault fallback will be properly set when it is requested by desktop
        Self {
            active_vault: String::new(),
            vaults,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BrowserSettings {
    pub enabled: bool,
    pub headless: bool,
    pub persistent: bool,
    pub server_url: String,
    /// Browser engine to use: "auto" (prefer BotBrowser → system Chrome),
    /// "botbrowser" (force BotBrowser, lazy-download if absent),
    /// "chrome" (system Chrome only).
    pub browser_type: String,
    pub cdp_port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PermissionSettings {
    /// ACP standard approval preset: "read-only" | "auto" | "full-access"
    #[serde(default = "default_approval_preset")]
    pub approval_preset: String,
    /// Legacy field — migrated to approval_preset on load
    #[serde(default, skip_serializing)]
    pub yolo_mode: Option<bool>,
    /// Per-plugin auto-approve (plugin_id → enabled)
    /// Default: {"memory": true}
    #[serde(default = "default_auto_approve_plugins")]
    pub auto_approve_plugins: HashMap<String, bool>,
    /// Legacy field — migrated to auto_approve_plugins on load
    #[serde(default, skip_serializing)]
    pub memory_auto_approve: Option<bool>,
    /// Global permission policies (tool pattern → allow/deny)
    pub policies: Vec<PermissionPolicy>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionPolicy {
    /// Matching pattern (regex or prefix for tool title)
    pub pattern: String,
    /// Option ID to respond with (e.g. "allow_always")
    #[serde(rename = "optionId", default)]
    pub option_id: String,
    /// Policy kind: "allow_always" or "reject_always"
    #[serde(default = "default_allow_always")]
    pub kind: String,
    /// Created timestamp (optional)
    #[serde(rename = "createdAt", default)]
    pub created_at: u64,
}

pub fn default_allow_always() -> String {
    "allow_always".to_string()
}

pub fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct McpSettings {
    pub transport_type: String,
    pub sse_url: String,
    pub command: String,
    pub args: String,
    pub bearer_token: String,
    pub header_name: String,
    pub oauth_client_id: String,
    pub oauth_scope: String,
    pub oauth_client_secret: String,
    pub oauth_authorization_url: String,
    pub oauth_token_url: String,
    pub custom_headers: HashMap<String, String>,
    pub presets: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DashboardSettings {
    pub schedules: Vec<serde_json::Value>,
    pub categories: Vec<serde_json::Value>,
    pub schedules_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentCapabilitiesOverride {
    pub skills: Vec<String>,
    pub mcps: Vec<String>,
}

impl Default for AgentCapabilitiesOverride {
    fn default() -> Self {
        Self {
            skills: Vec::new(),
            mcps: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct KnowledgeRuntimeStatus {
    pub last_result: String,
    pub last_driver: Option<String>,
    pub last_workspace_id: Option<String>,
    pub last_workspace_name: Option<String>,
    pub last_issue_count: usize,
    pub last_report_path: Option<String>,
    pub last_error: Option<String>,
    pub last_run_reason: Option<String>,
    pub last_success_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentSettings {
    /// Agent command to spawn (e.g. "ilhae", "gemini --experimental-acp", "claude --acp")
    pub command: String,
    /// Active ilhae product profile projected from ~/.ilhae/config.toml
    #[serde(default)]
    pub active_profile: Option<String>,
    /// Local native-runtime thinking mode. One of: on|off.
    #[serde(default = "default_thinking_mode")]
    pub thinking_mode: String,
    /// Optional A2A endpoint URL. When non-empty, uses A2A transport instead of stdio.
    #[serde(default)]
    pub a2a_endpoint: String,
    /// Team mode on/off flag from UI.
    #[serde(default)]
    pub team_mode: bool,
    /// Dream mode (Background hygiene agent) on/off flag.
    #[serde(default)]
    pub dream_mode: bool,
    /// Embed mode (Right Brain vector search) on/off flag.
    #[serde(default)]
    pub embed_mode: bool,
    /// Team execution backend: local | remote | hybrid.
    #[serde(default = "default_team_backend")]
    pub team_backend: String,
    /// Team response merge policy projected from profile.
    #[serde(default = "default_team_merge_policy")]
    pub team_merge_policy: String,
    /// Team delegation retry budget projected from profile.
    #[serde(default = "default_team_max_retries")]
    pub team_max_retries: u32,
    /// Whether team execution pauses immediately when a delegation fails.
    #[serde(default = "default_team_pause_on_error")]
    pub team_pause_on_error: bool,
    /// Autonomous mode on/off flag from UI.
    /// When true, proxy can auto-continue A2A turns that end with input-required.
    #[serde(default)]
    pub autonomous_mode: bool,
    /// Advisor/reviewer mode flag projected from profile.
    #[serde(default)]
    pub advisor_mode: bool,
    /// Advisor response preset projected from profile.
    #[serde(default = "default_advisor_preset")]
    pub advisor_preset: String,
    /// Autonomous loop iteration budget projected from profile.
    #[serde(default = "default_auto_max_turns")]
    pub auto_max_turns: u32,
    /// Autonomous loop time budget in minutes projected from profile.
    #[serde(default = "default_auto_timebox_minutes")]
    pub auto_timebox_minutes: u32,
    /// Whether autonomous execution pauses immediately on execution errors.
    #[serde(default = "default_auto_pause_on_error")]
    pub auto_pause_on_error: bool,
    /// Kairos proactive scheduling enablement projected from profile.
    #[serde(default)]
    pub kairos_enabled: bool,
    /// Knowledge auto-loop mode projected from profile. One of: off|worker|kairos|both.
    #[serde(default = "default_knowledge_mode")]
    pub knowledge_mode: String,
    /// Knowledge workspace id projected from profile.
    #[serde(default)]
    pub knowledge_workspace_id: Option<String>,
    /// Worker-loop polling interval in seconds projected from profile.
    #[serde(default = "default_knowledge_poll_interval_secs")]
    pub knowledge_poll_interval_secs: u64,
    /// Periodic execution interval in seconds projected from profile.
    #[serde(default = "default_knowledge_periodic_interval_secs")]
    pub knowledge_periodic_interval_secs: u64,
    /// file_back target bucket projected from profile.
    #[serde(default = "default_knowledge_report_target")]
    pub knowledge_report_target: String,
    /// file_back relative path projected from profile.
    #[serde(default = "default_knowledge_report_relative_path")]
    pub knowledge_report_relative_path: String,
    /// Runtime status snapshot for the knowledge auto-loop.
    #[serde(default)]
    pub knowledge_runtime: KnowledgeRuntimeStatus,
    /// Hygiene auto-loop mode. One of: off|worker|kairos|both.
    #[serde(default = "default_hygiene_mode")]
    pub hygiene_mode: String,
    /// Runtime status snapshot for the hygiene auto-loop.
    #[serde(default)]
    pub hygiene_runtime: HygieneRuntimeStatus,
    /// Self-improvement loop enablement projected from profile.
    #[serde(default = "default_self_improvement_enabled")]
    pub self_improvement_enabled: bool,
    /// Self-improvement execution preset projected from profile.
    #[serde(default = "default_self_improvement_preset")]
    pub self_improvement_preset: String,
    /// Runtime status snapshot for offline self-improvement optimization.
    #[serde(default)]
    pub self_improvement_runtime: SelfImprovementRuntimeStatus,
    /// Runtime memory scope projected from profile.
    #[serde(default)]
    pub memory_scope: Option<String>,
    /// Runtime task scope projected from profile.
    #[serde(default)]
    pub task_scope: Option<String>,
    /// Desktop-only: enable internal MockAgent instead of real A2A engines.
    /// The desktop app sets this on first run to make headless E2E easier.
    /// Can also be overridden by ILHAE_MOCK env var.
    #[serde(default)]
    pub mock_mode: bool,
    /// List of enabled engines. These engines' capabilities will be merged and available globally.
    #[serde(default = "default_enabled_engines")]
    pub enabled_engines: Vec<String>,
    /// Per-agent capability state for Team Mode.
    /// Keys are agent roles (e.g., "Leader", "Researcher").
    /// Values are lists of disabled skill/MCP names for that specific agent.
    #[serde(default)]
    pub team_agent_disabled_capabilities:
        std::collections::HashMap<String, AgentCapabilitiesOverride>,
}

pub fn default_enabled_engines() -> Vec<String> {
    vec!["gemini".to_string()]
}

pub fn default_thinking_mode() -> String {
    "on".to_string()
}

pub fn normalize_thinking_mode(mode: &str) -> String {
    match mode.trim().to_ascii_lowercase().as_str() {
        "off" | "false" | "0" | "disabled" => "off".to_string(),
        "on" | "true" | "1" | "enabled" | "" => default_thinking_mode(),
        _ => default_thinking_mode(),
    }
}

pub fn thinking_mode_enabled(mode: &str) -> bool {
    normalize_thinking_mode(mode) == "on"
}

pub fn default_advisor_preset() -> String {
    "review_first".to_string()
}

pub fn default_auto_max_turns() -> u32 {
    10
}

pub fn default_auto_timebox_minutes() -> u32 {
    15
}

pub fn default_auto_pause_on_error() -> bool {
    true
}

pub fn default_knowledge_mode() -> String {
    "off".to_string()
}

pub fn default_knowledge_poll_interval_secs() -> u64 {
    60
}

pub fn default_knowledge_periodic_interval_secs() -> u64 {
    1800
}

pub fn default_knowledge_report_target() -> String {
    "index".to_string()
}

pub fn default_knowledge_report_relative_path() -> String {
    "knowledge_loop_health.md".to_string()
}

pub fn default_hygiene_mode() -> String {
    "both".to_string()
}

pub fn default_team_merge_policy() -> String {
    "append_all".to_string()
}

pub fn default_team_max_retries() -> u32 {
    3
}

pub fn default_team_pause_on_error() -> bool {
    true
}

pub fn default_self_improvement_preset() -> String {
    "foreground".to_string()
}

pub fn default_self_improvement_enabled() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct HygieneRuntimeStatus {
    pub last_result: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_driver: Option<String>,
    #[serde(default)]
    pub duplicate_tasks_folded: usize,
    #[serde(default)]
    pub legacy_commands_normalized: usize,
    #[serde(default)]
    pub knowledge_reports_written: usize,
    #[serde(default)]
    pub memory_reports_written: usize,
    #[serde(default)]
    pub orphaned_source_pages: usize,
    #[serde(default)]
    pub orphaned_concept_pages: usize,
    #[serde(default)]
    pub stale_output_candidates: usize,
    #[serde(default)]
    pub duplicate_knowledge_items: usize,
    #[serde(default)]
    pub dream_duplicate_candidates: usize,
    #[serde(default)]
    pub dream_rare_candidates: usize,
    #[serde(default)]
    pub ignored_dream_candidates: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_success_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct SelfImprovementRuntimeStatus {
    pub last_result: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_optimizer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_success_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_instructions: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_score: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_generated_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved_instructions: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved_score: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved_at: Option<u64>,
}

pub fn default_team_backend() -> String {
    "local".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AsrSettings {
    /// ASR Gateway server URL (e.g. "http://gpu-server:8000")
    pub server_url: String,
}

// ─── Channel Settings ────────────────────────────────────────────────────────

/// All chat channel settings, grouped under `channels` key.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ChannelsSettings {
    pub telegram: TelegramSettings,
    pub kakao: KakaoSettings,
    pub discord: DiscordSettings,
    pub slack: GenericChannelSettings,
    pub whatsapp: GenericChannelSettings,
    pub line: GenericChannelSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TelegramSettings {
    /// Enable/disable telegram bot
    pub enabled: bool,
    /// Bot token from @BotFather
    pub bot_token: String,
    /// Allowed Telegram chat IDs (empty = allow all)
    pub allowed_chat_ids: Vec<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct KakaoSettings {
    pub enabled: bool,
    /// KakaoTalk Channel API app key
    pub app_key: String,
    /// Allowed user IDs
    pub allowed_user_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DiscordSettings {
    pub enabled: bool,
    /// Discord bot token
    pub bot_token: String,
    /// Allowed guild (server) IDs
    pub guild_ids: Vec<String>,
}

/// Generic settings for channels not yet fully implemented.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GenericChannelSettings {
    pub enabled: bool,
    /// Channel-specific API token/key
    pub api_token: String,
    /// Extra config as key-value pairs
    pub extra: HashMap<String, serde_json::Value>,
}

impl Default for ChannelsSettings {
    fn default() -> Self {
        Self {
            telegram: TelegramSettings::default(),
            kakao: KakaoSettings::default(),
            discord: DiscordSettings::default(),
            slack: GenericChannelSettings::default(),
            whatsapp: GenericChannelSettings::default(),
            line: GenericChannelSettings::default(),
        }
    }
}

impl Default for TelegramSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            bot_token: String::new(),
            allowed_chat_ids: Vec::new(),
        }
    }
}

impl Default for KakaoSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            app_key: String::new(),
            allowed_user_ids: Vec::new(),
        }
    }
}

impl Default for DiscordSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            bot_token: String::new(),
            guild_ids: Vec::new(),
        }
    }
}

impl Default for GenericChannelSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            api_token: String::new(),
            extra: HashMap::new(),
        }
    }
}

impl Default for AsrSettings {
    fn default() -> Self {
        Self {
            server_url: "http://localhost:8000".to_string(),
        }
    }
}

impl Default for AgentSettings {
    fn default() -> Self {
        Self {
            command: "ilhae".to_string(),
            active_profile: None,
            thinking_mode: default_thinking_mode(),
            a2a_endpoint: String::new(),
            team_mode: false,
            dream_mode: false,
            embed_mode: false,
            team_backend: default_team_backend(),
            team_merge_policy: default_team_merge_policy(),
            team_max_retries: default_team_max_retries(),
            team_pause_on_error: default_team_pause_on_error(),
            autonomous_mode: false,
            advisor_mode: false,
            advisor_preset: default_advisor_preset(),
            auto_max_turns: default_auto_max_turns(),
            auto_timebox_minutes: default_auto_timebox_minutes(),
            auto_pause_on_error: default_auto_pause_on_error(),
            kairos_enabled: false,
            knowledge_mode: default_knowledge_mode(),
            knowledge_workspace_id: None,
            knowledge_poll_interval_secs: default_knowledge_poll_interval_secs(),
            knowledge_periodic_interval_secs: default_knowledge_periodic_interval_secs(),
            knowledge_report_target: default_knowledge_report_target(),
            knowledge_report_relative_path: default_knowledge_report_relative_path(),
            knowledge_runtime: KnowledgeRuntimeStatus::default(),
            hygiene_mode: default_hygiene_mode(),
            hygiene_runtime: HygieneRuntimeStatus::default(),
            self_improvement_enabled: default_self_improvement_enabled(),
            self_improvement_preset: default_self_improvement_preset(),
            self_improvement_runtime: SelfImprovementRuntimeStatus::default(),
            memory_scope: None,
            task_scope: None,
            mock_mode: false,
            enabled_engines: vec!["gemini".to_string()],
            team_agent_disabled_capabilities: std::collections::HashMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UiSettings {
    #[serde(default)]
    pub hide_thinking: bool,
    #[serde(default)]
    pub gui_mode: Option<bool>,
}

impl Default for UiSettings {
    fn default() -> Self {
        Self {
            hide_thinking: false,
            gui_mode: None,
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            browser: BrowserSettings::default(),
            permissions: PermissionSettings::default(),
            mcp: McpSettings::default(),
            agent: AgentSettings::default(),
            asr: AsrSettings::default(),
            plugins: HashMap::new(),
            ui: UiSettings::default(),
            builtin_tools: HashMap::new(),
            dashboard: DashboardSettings::default(),
            channels: ChannelsSettings::default(),
            telegram: None,
            session_cwd_map: HashMap::new(),
            vault: VaultSettings::default(),
        }
    }
}

impl Default for BrowserSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            headless: true,
            persistent: true,
            server_url: String::new(),
            browser_type: "auto".to_string(),
            cdp_port: 19222,
        }
    }
}

pub fn default_approval_preset() -> String {
    "auto".to_string()
}

pub fn default_auto_approve_plugins() -> HashMap<String, bool> {
    let mut m = HashMap::new();
    // All built-in plugins default to auto-approve = true
    for id in &["session", "memory", "task", "ui", "browser", "workflow"] {
        m.insert(id.to_string(), true);
    }
    m
}

impl Default for PermissionSettings {
    fn default() -> Self {
        Self {
            approval_preset: default_approval_preset(),
            yolo_mode: None,
            auto_approve_plugins: default_auto_approve_plugins(),
            memory_auto_approve: None,
            policies: Vec::new(),
        }
    }
}

impl Default for McpSettings {
    fn default() -> Self {
        Self {
            transport_type: "streamable-http".to_string(),
            sse_url: "http://localhost:3004/sse".to_string(),
            command: String::new(),
            args: String::new(),
            bearer_token: String::new(),
            header_name: String::new(),
            oauth_client_id: String::new(),
            oauth_scope: String::new(),
            oauth_client_secret: String::new(),
            oauth_authorization_url: String::new(),
            oauth_token_url: String::new(),
            custom_headers: HashMap::new(),
            presets: vec![
                serde_json::json!({
                    "id": "mail",
                    "name": "mail",
                    "transport_type": "streamable-http",
                    "sse_url": "https://mail.ugot.uk/mcp/sse",
                }),
                serde_json::json!({
                    "id": "onlyoffice",
                    "name": "onlyoffice",
                    "transport_type": "sse",
                    "sse_url": "http://localhost:3004/sse",
                }),
                serde_json::json!({
                    "id": "fortune-v3",
                    "name": "fortune",
                    "transport_type": "streamable-http",
                    "sse_url": "https://fortune.ugot.uk/mcp",
                    "connection_type": "direct",
                }),
            ],
        }
    }
}

impl Default for DashboardSettings {
    fn default() -> Self {
        Self {
            schedules: Vec::new(),
            categories: Vec::new(),
            schedules_version: String::new(),
        }
    }
}

/// Event emitted when any setting is modified.
#[derive(Clone, Debug)]
pub struct SettingsEvent {
    pub key: String,
    pub value: serde_json::Value,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_self_improvement_preset_is_foreground() {
        assert_eq!(default_self_improvement_preset(), "foreground");
        assert_eq!(
            AgentSettings::default().self_improvement_preset,
            "foreground"
        );
        assert!(AgentSettings::default().self_improvement_enabled);
    }
}
