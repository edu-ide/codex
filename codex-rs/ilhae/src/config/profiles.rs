//! Human-managed Ilhae profile types, independent of runtime projection.

use crate::settings_types::default_advisor_preset;
use crate::settings_types::default_approval_preset;
use crate::settings_types::default_auto_max_turns;
use crate::settings_types::default_auto_pause_on_error;
use crate::settings_types::default_auto_timebox_minutes;
use crate::settings_types::default_knowledge_mode;
use crate::settings_types::default_knowledge_periodic_interval_secs;
use crate::settings_types::default_knowledge_poll_interval_secs;
use crate::settings_types::default_knowledge_report_relative_path;
use crate::settings_types::default_knowledge_report_target;
use crate::settings_types::default_self_improvement_enabled;
use crate::settings_types::default_self_improvement_preset;
use crate::settings_types::default_team_backend;
use crate::settings_types::default_team_max_retries;
use crate::settings_types::default_team_merge_policy;
use crate::settings_types::default_team_pause_on_error;
use codex_protocol::config_types::TrustLevel;
use std::collections::BTreeMap;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
#[serde(default)]
pub struct IlhaeTomlConfig {
    pub profile: IlhaeActiveProfileConfig,
    pub profiles: BTreeMap<String, IlhaeProfileConfig>,
    pub projects: BTreeMap<String, IlhaeProjectConfig>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
#[serde(default)]
pub struct IlhaeActiveProfileConfig {
    pub active: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
#[serde(default)]
pub struct IlhaeProfileConfig {
    pub agent: IlhaeProfileAgentConfig,
    pub permissions: IlhaeProfilePermissionsConfig,
    pub memory: IlhaeProfileScopeConfig,
    pub task: IlhaeProfileScopeConfig,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub knowledge: Option<IlhaeProfileKnowledgeConfig>,
    pub system2: IlhaeProfileSystem2Config,
    pub native_runtime: IlhaeProfileNativeRuntimeConfig,
    /// Another profile whose native runtime backs this profile's engine (e.g. the
    /// local model behind a router). Ilhae starts and stops it with this profile,
    /// while Codex keeps talking to this profile's own engine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_profile: Option<String>,
    /// Helper services this profile needs while an Ilhae client uses it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sidecars: Vec<IlhaeProfileSidecarConfig>,
    /// Stop the sidecars and native runtimes once the last Ilhae client exits.
    #[serde(default, skip_serializing_if = "is_false")]
    pub stop_when_unused: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

fn default_sidecar_startup_timeout_secs() -> u64 {
    180
}

/// A process Ilhae starts on demand and keeps running while the profile is in use.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct IlhaeProfileSidecarConfig {
    pub name: String,
    /// Program followed by its arguments; no shell is involved.
    pub command: Vec<String>,
    pub cwd: String,
    pub env: BTreeMap<String, String>,
    /// Must answer 2xx once the sidecar is ready.
    pub health_url: String,
    #[serde(default = "default_sidecar_startup_timeout_secs")]
    pub startup_timeout_secs: u64,
    pub log_file: String,
}

impl Default for IlhaeProfileSidecarConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            command: Vec::new(),
            cwd: String::new(),
            env: BTreeMap::new(),
            health_url: String::new(),
            startup_timeout_secs: default_sidecar_startup_timeout_secs(),
            log_file: String::new(),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
#[serde(default)]
pub struct IlhaeProjectConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trust_level: Option<TrustLevel>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct IlhaeProfileAgentConfig {
    #[serde(rename = "engine")]
    pub engine_id: Option<String>,
    pub command: Option<String>,
    pub team_mode: bool,
    #[serde(default)]
    pub dream_mode: bool,
    #[serde(default)]
    pub embed_mode: bool,
    #[serde(default = "default_team_backend")]
    pub team_backend: String,
    #[serde(default = "default_team_merge_policy")]
    pub team_merge_policy: String,
    #[serde(default = "default_team_max_retries")]
    pub team_max_retries: u32,
    #[serde(default = "default_team_pause_on_error")]
    pub team_pause_on_error: bool,
    pub auto_mode: bool,
    pub advisor: bool,
    #[serde(default = "default_advisor_preset")]
    pub advisor_preset: String,
    #[serde(default = "default_auto_max_turns")]
    pub auto_max_turns: u32,
    #[serde(default = "default_auto_timebox_minutes")]
    pub auto_timebox_minutes: u32,
    #[serde(default = "default_auto_pause_on_error")]
    pub auto_pause_on_error: bool,
    pub kairos: bool,
    #[serde(default = "default_self_improvement_enabled")]
    pub self_improvement: bool,
    #[serde(default = "default_self_improvement_preset")]
    pub self_improvement_preset: String,
}

fn default_native_runtime_startup_timeout_secs() -> u64 {
    120
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct IlhaeProfileNativeRuntimeConfig {
    pub enabled: bool,
    pub provider: Option<String>,
    pub health_url: String,
    pub url: Option<String>,
    pub base_url: String,
    /// Runtime proxy origin. Local llama-server profiles default to the local
    /// Ilhae proxy, so only remote profiles need to set this value.
    pub proxy_url: Option<String>,
    /// Explicitly allows cleartext HTTP to a non-loopback runtime proxy.
    /// Authentication tokens and inference traffic are not encrypted.
    pub proxy_allow_insecure_http: bool,
    /// Shared bearer value used by both the proxy control and inference APIs.
    pub proxy_token: Option<String>,
    /// Internal marker used by the proxy host after receiving a runtime spec.
    #[doc(hidden)]
    #[serde(skip)]
    pub proxy_bypass: bool,
    pub server_bin: String,
    pub model_path: String,
    pub chat_template_file: String,
    pub log_file: String,
    pub env: std::collections::BTreeMap<String, String>,
    pub query_params: Option<BTreeMap<String, String>>,
    pub http_headers: Option<BTreeMap<String, String>>,
    pub env_http_headers: Option<BTreeMap<String, String>>,
    pub request_max_retries: Option<u64>,
    pub stream_max_retries: Option<u64>,
    pub context_window: Option<u64>,
    #[serde(default = "default_native_runtime_startup_timeout_secs")]
    pub startup_timeout_secs: u64,
    pub args: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
#[serde(default)]
pub struct IlhaeProfileSystem2Config {
    pub enabled: bool,
    pub profile: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedSystem2TargetConfig {
    pub source_profile_id: String,
    pub target_profile_id: String,
    pub base_url: String,
    pub model_name: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct IlhaeProfilePermissionsConfig {
    pub approval_preset: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct IlhaeProfileScopeConfig {
    pub scope: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct IlhaeProfileKnowledgeConfig {
    pub mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    pub poll_interval_secs: u64,
    pub periodic_interval_secs: u64,
    pub report_target: String,
    pub report_relative_path: String,
}

impl Default for IlhaeProfilePermissionsConfig {
    fn default() -> Self {
        Self {
            approval_preset: default_approval_preset(),
        }
    }
}

impl Default for IlhaeProfileAgentConfig {
    fn default() -> Self {
        Self {
            engine_id: None,
            command: None,
            team_mode: false,
            dream_mode: false,
            embed_mode: false,
            team_backend: default_team_backend(),
            team_merge_policy: default_team_merge_policy(),
            team_max_retries: default_team_max_retries(),
            team_pause_on_error: default_team_pause_on_error(),
            auto_mode: false,
            advisor: false,
            advisor_preset: default_advisor_preset(),
            auto_max_turns: default_auto_max_turns(),
            auto_timebox_minutes: default_auto_timebox_minutes(),
            auto_pause_on_error: default_auto_pause_on_error(),
            kairos: false,
            self_improvement: default_self_improvement_enabled(),
            self_improvement_preset: default_self_improvement_preset(),
        }
    }
}

impl Default for IlhaeProfileScopeConfig {
    fn default() -> Self {
        Self {
            scope: "default".to_string(),
        }
    }
}

impl Default for IlhaeProfileKnowledgeConfig {
    fn default() -> Self {
        Self {
            mode: default_knowledge_mode(),
            workspace_id: None,
            poll_interval_secs: default_knowledge_poll_interval_secs(),
            periodic_interval_secs: default_knowledge_periodic_interval_secs(),
            report_target: default_knowledge_report_target(),
            report_relative_path: default_knowledge_report_relative_path(),
        }
    }
}

impl Default for IlhaeProfileNativeRuntimeConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: None,
            health_url: String::new(),
            url: None,
            base_url: String::new(),
            proxy_url: None,
            proxy_allow_insecure_http: false,
            proxy_token: None,
            proxy_bypass: false,
            server_bin: String::new(),
            model_path: String::new(),
            chat_template_file: String::new(),
            log_file: String::new(),
            env: std::collections::BTreeMap::new(),
            query_params: None,
            http_headers: None,
            env_http_headers: None,
            request_max_retries: None,
            stream_max_retries: None,
            context_window: None,
            startup_timeout_secs: default_native_runtime_startup_timeout_secs(),
            args: Vec::new(),
        }
    }
}
