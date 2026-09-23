//! Render product intent into the upstream Codex configuration schema.

use super::super::IlhaeTomlConfig;
use super::super::normalize_team_backend;
use super::super::settings_for_managed_profile;
use super::ILHAE_RUNTIME_SYSTEM2_PROJECTION_KEY;
use super::mcp::native_mcp_defaults;
use super::mcp::user_mcp_servers_for_managed_config;
use super::providers::codex_profile_table_for_ilhae_profile;
use super::providers::insert_native_model_provider;
use super::schema::system2_projection_table_from;
use std::path::Path;

pub(super) fn user_model_providers_for_managed_config(
    user_config: &toml::Value,
) -> toml::value::Table {
    user_table_for_managed_config(user_config, "model_providers")
}

pub(super) fn user_features_for_managed_config(user_config: &toml::Value) -> toml::value::Table {
    user_table_for_managed_config(user_config, "features")
}

pub(super) fn user_web_search_for_managed_config(user_config: &toml::Value) -> Option<toml::Value> {
    user_config_value_for_managed_config(user_config, "web_search")
}

pub(super) fn user_tools_for_managed_config(user_config: &toml::Value) -> toml::value::Table {
    let user_tools = user_table_for_managed_config(user_config, "tools");
    let mut tools = toml::value::Table::new();
    for key in ["web_search", "view_image"] {
        if let Some(value) = user_tools.get(key).cloned() {
            tools.insert(key.to_string(), value);
        }
    }
    tools
}

pub(super) fn user_table_for_managed_config(
    user_config: &toml::Value,
    key: &str,
) -> toml::value::Table {
    user_config_value_for_managed_config(user_config, key)
        .and_then(|value| value.as_table().cloned())
        .unwrap_or_default()
}

pub(super) fn user_config_value_for_managed_config(
    user_config: &toml::Value,
    key: &str,
) -> Option<toml::Value> {
    user_config.get(key).cloned()
}

pub(super) fn default_ilhae_codex_home_table(
    config: &IlhaeTomlConfig,
    user_config: &toml::Value,
    model_catalog_path: &Path,
) -> toml::value::Table {
    let active_profile_name = config
        .profile
        .active
        .clone()
        .filter(|value| config.profiles.contains_key(value));
    let active_profile = active_profile_name
        .as_ref()
        .and_then(|id| config.profiles.get(id))
        .cloned()
        .unwrap_or_default();
    let active_profile_provider_id = active_profile_name
        .as_deref()
        .unwrap_or("ilhae-active")
        .to_string();

    let mut root = toml::value::Table::new();

    // 승인·샌드박스는 사용자 설정(permissions.approval_preset)을 따른다.
    //
    // 2026-09-09 까지 여기에 "never" / "danger-full-access" 가 하드코딩돼 있었다. 제품은
    // "승인은 사람이 한다"고 광고하는데, 앱이 기동할 때마다 이 생성기가 codex 런타임
    // 설정을 그 값으로 덮어써서 실제로는 아무것도 묻지 않았다. 데스크톱이 thread/start
    // 에 정책을 실어 보내도록 고쳤지만, 정책을 싣지 않는 경로(CLI 등)는 이 파일의 값을
    // 그대로 쓴다. 그래서 여기서도 같은 매핑을 쓴다. 값이 비었거나 모르는 값이면 안전한
    // 쪽으로 떨어진다(fail-safe) — 전체 접근은 "full-access" 라고 정확히 적혔을 때만.
    let (approval_policy, sandbox_mode) = match active_profile.permissions.approval_preset.trim() {
        "full-access" => ("never", "danger-full-access"),
        _ => ("on-request", "workspace-write"),
    };
    root.insert(
        "approval_policy".to_string(),
        toml::Value::String(approval_policy.to_string()),
    );
    root.insert(
        "sandbox_mode".to_string(),
        toml::Value::String(sandbox_mode.to_string()),
    );
    // Keep credentials in a file: the keyring backend is unavailable in the
    // headless/desktop-spawned sessions this config is generated for.
    root.insert(
        "cli_auth_credentials_store".to_string(),
        toml::Value::String("file".to_string()),
    );
    let active_codex_profile = codex_profile_table_for_ilhae_profile(
        &active_profile_provider_id,
        &active_profile,
        user_config,
        model_catalog_path,
    );
    for key in [
        "model",
        "model_provider",
        "model_context_window",
        "model_catalog_json",
    ] {
        if let Some(value) = active_codex_profile.get(key).cloned() {
            root.insert(key.to_string(), value);
        }
    }
    if let Some(web_search) = user_web_search_for_managed_config(user_config) {
        root.insert("web_search".to_string(), web_search);
    }
    let user_tools = user_tools_for_managed_config(user_config);
    if !user_tools.is_empty() {
        root.insert("tools".to_string(), toml::Value::Table(user_tools));
    }

    let mut agent = toml::value::Table::new();
    agent.insert(
        "active_profile".to_string(),
        toml::Value::String(
            active_profile_name
                .clone()
                .unwrap_or_else(|| "ilhae-active".to_string()),
        ),
    );
    if let Some(command) = active_profile.agent.command.clone() {
        agent.insert("command".to_string(), toml::Value::String(command));
    }
    agent.insert(
        "team_mode".to_string(),
        toml::Value::Boolean(active_profile.agent.team_mode),
    );
    agent.insert(
        "team_backend".to_string(),
        toml::Value::String(normalize_team_backend(&active_profile.agent.team_backend)),
    );
    agent.insert(
        "team_merge_policy".to_string(),
        toml::Value::String(active_profile.agent.team_merge_policy.clone()),
    );
    agent.insert(
        "team_max_retries".to_string(),
        toml::Value::Integer(active_profile.agent.team_max_retries as i64),
    );
    agent.insert(
        "team_pause_on_error".to_string(),
        toml::Value::Boolean(active_profile.agent.team_pause_on_error),
    );
    agent.insert(
        "autonomous_mode".to_string(),
        toml::Value::Boolean(active_profile.agent.auto_mode),
    );
    agent.insert(
        "advisor_mode".to_string(),
        toml::Value::Boolean(active_profile.agent.advisor),
    );
    agent.insert(
        "advisor_preset".to_string(),
        toml::Value::String(active_profile.agent.advisor_preset.clone()),
    );
    agent.insert(
        "auto_max_turns".to_string(),
        toml::Value::Integer(active_profile.agent.auto_max_turns as i64),
    );
    agent.insert(
        "auto_timebox_minutes".to_string(),
        toml::Value::Integer(active_profile.agent.auto_timebox_minutes as i64),
    );
    agent.insert(
        "auto_pause_on_error".to_string(),
        toml::Value::Boolean(active_profile.agent.auto_pause_on_error),
    );
    agent.insert(
        "kairos_enabled".to_string(),
        toml::Value::Boolean(active_profile.agent.kairos),
    );
    agent.insert(
        "self_improvement_enabled".to_string(),
        toml::Value::Boolean(active_profile.agent.self_improvement),
    );
    agent.insert(
        "self_improvement_preset".to_string(),
        toml::Value::String(active_profile.agent.self_improvement_preset.clone()),
    );
    root.insert("agent".to_string(), toml::Value::Table(agent));
    let managed_settings =
        settings_for_managed_profile(active_profile_name.clone(), &active_profile);
    if let Some(developer_instructions) =
        crate::session_context_service::build_runtime_loop_developer_instructions(&managed_settings)
    {
        root.insert(
            "developer_instructions".to_string(),
            toml::Value::String(developer_instructions),
        );
    }

    let mut features = toml::value::Table::new();
    features.insert(
        "apply_patch_freeform".to_string(),
        toml::Value::Boolean(true),
    );
    features.insert(
        "apply_patch_streaming_events".to_string(),
        toml::Value::Boolean(true),
    );
    features.insert("fast_mode".to_string(), toml::Value::Boolean(true));
    features.insert("multi_agent".to_string(), toml::Value::Boolean(true));
    for (key, value) in user_features_for_managed_config(user_config) {
        features.insert(key, value);
    }
    root.insert("features".to_string(), toml::Value::Table(features));
    root.insert(
        "mcp_oauth_credentials_store".to_string(),
        toml::Value::String("file".to_string()),
    );
    if let Some(system2_projection) = system2_projection_table_from(config) {
        let mut desktop = toml::value::Table::new();
        desktop.insert(
            ILHAE_RUNTIME_SYSTEM2_PROJECTION_KEY.to_string(),
            toml::Value::Table(system2_projection),
        );
        root.insert("desktop".to_string(), toml::Value::Table(desktop));
    }

    let mut mcp_servers = user_mcp_servers_for_managed_config(user_config);

    if std::env::var("ILHAE_DREAM_MODE").is_err() {
        native_mcp_defaults(&mut mcp_servers, user_config);

        let mut computer = toml::value::Table::new();
        computer.insert(
            "command".to_string(),
            toml::Value::String("computer".to_string()),
        );
        computer.insert(
            "args".to_string(),
            toml::Value::Array(vec![toml::Value::String("mcp".to_string())]),
        );
        mcp_servers
            .entry("computer".to_string())
            .or_insert(toml::Value::Table(computer));
    }

    root.insert("mcp_servers".to_string(), toml::Value::Table(mcp_servers));

    let mut plugins = toml::value::Table::new();
    for plugin in [
        "canva@openai-curated",
        "github@openai-curated",
        "gmail@openai-curated",
    ] {
        let mut entry = toml::value::Table::new();
        entry.insert("enabled".to_string(), toml::Value::Boolean(true));
        plugins.insert(plugin.to_string(), toml::Value::Table(entry));
    }
    root.insert("plugins".to_string(), toml::Value::Table(plugins));

    let mut model_providers = user_model_providers_for_managed_config(user_config);
    for (profile_id, profile) in &config.profiles {
        insert_native_model_provider(&mut model_providers, profile_id, profile);
    }
    insert_native_model_provider(
        &mut model_providers,
        &active_profile_provider_id,
        &active_profile,
    );
    if !model_providers.is_empty() {
        root.insert(
            "model_providers".to_string(),
            toml::Value::Table(model_providers),
        );
    }

    let mut profiles = toml::value::Table::new();
    for (profile_id, profile) in &config.profiles {
        profiles.insert(
            profile_id.clone(),
            toml::Value::Table(codex_profile_table_for_ilhae_profile(
                profile_id,
                profile,
                user_config,
                model_catalog_path,
            )),
        );
    }
    profiles.insert(
        "ilhae-active".to_string(),
        toml::Value::Table(active_codex_profile),
    );
    root.insert("profiles".to_string(), toml::Value::Table(profiles));

    let mut projects = toml::value::Table::new();
    for (project_path, project) in &config.projects {
        let Some(trust_level) = project.trust_level else {
            continue;
        };
        let mut project_table = toml::value::Table::new();
        project_table.insert(
            "trust_level".to_string(),
            toml::Value::String(trust_level.to_string()),
        );
        projects.insert(project_path.clone(), toml::Value::Table(project_table));
    }
    if !projects.is_empty() {
        root.insert("projects".to_string(), toml::Value::Table(projects));
    }

    root
}
