use crate::config::test_config;
use crate::shell::Shell;
use crate::shell::ShellType;
use crate::test_support::construct_model_info_offline;
use crate::tools::ToolRouter;
use crate::tools::router::ToolRouterParams;
use codex_app_server_protocol::AppInfo;
use codex_features::Feature;
use codex_features::Features;
use codex_mcp::CODEX_APPS_MCP_SERVER_NAME;
use codex_models_manager::bundled_models_response;
use codex_models_manager::model_info::with_config_overrides;
use codex_protocol::config_types::WebSearchMode;
use codex_protocol::config_types::WindowsSandboxLevel;
use codex_protocol::models::VIEW_IMAGE_TOOL_NAME;
use codex_protocol::openai_models::ConfigShellToolType;
use codex_protocol::openai_models::InputModality;
use codex_protocol::openai_models::ModelInfo;
use codex_protocol::openai_models::ModelsResponse;
use codex_protocol::protocol::SandboxPolicy;
use codex_protocol::protocol::SessionSource;
use codex_tools::AdditionalProperties;
use codex_tools::CommandToolOptions;
use codex_tools::ConfiguredToolSpec;
use codex_tools::DiscoverablePluginInfo;
use codex_tools::DiscoverableTool;
use codex_tools::FreeformTool;
use codex_tools::ResponsesApiTool;
use codex_tools::ResponsesApiWebSearchFilters;
use codex_tools::ResponsesApiWebSearchUserLocation;
use codex_tools::SpawnAgentToolOptions;
use codex_tools::ViewImageToolOptions;
use codex_tools::WaitAgentTimeoutOptions;
use codex_tools::create_close_agent_tool_v1;
use codex_tools::create_close_agent_tool_v2;
use codex_tools::create_exec_command_tool;
use codex_tools::create_request_permissions_tool;
use codex_tools::create_request_user_input_tool;
use codex_tools::create_resume_agent_tool;
use codex_tools::create_send_input_tool_v1;
use codex_tools::create_send_message_tool;
use codex_tools::create_spawn_agent_tool_v1;
use codex_tools::create_spawn_agent_tool_v2;
use codex_tools::create_update_plan_tool;
use codex_tools::create_view_image_tool;
use codex_tools::create_wait_agent_tool_v1;
use codex_tools::create_wait_agent_tool_v2;
use codex_tools::create_write_stdin_tool;
use codex_tools::mcp_tool_to_deferred_responses_api_tool;
use codex_utils_absolute_path::AbsolutePathBuf;
use core_test_support::assert_regex_match;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::path::PathBuf;

use super::*;

fn mcp_tool(name: &str, description: &str, input_schema: serde_json::Value) -> rmcp::model::Tool {
    rmcp::model::Tool {
        name: name.to_string().into(),
        title: None,
        description: Some(description.to_string().into()),
        input_schema: std::sync::Arc::new(rmcp::model::object(input_schema)),
        output_schema: None,
        annotations: None,
        execution: None,
        icons: None,
        meta: None,
    }
}

fn mcp_tool_info(tool: rmcp::model::Tool) -> ToolInfo {
    ToolInfo {
        server_name: "test_server".to_string(),
        callable_name: tool.name.to_string(),
        callable_namespace: "mcp__test_server__".to_string(),
        server_instructions: None,
        tool,
        connector_id: None,
        connector_name: None,
        plugin_display_names: Vec::new(),
        connector_description: None,
    }
}

fn mcp_tool_info_with_display_name(display_name: &str, tool: rmcp::model::Tool) -> ToolInfo {
    let (callable_namespace, callable_name) = display_name
        .rsplit_once('/')
        .map(|(namespace, callable_name)| (format!("{namespace}/"), callable_name.to_string()))
        .unwrap_or_else(|| ("".to_string(), display_name.to_string()));

    ToolInfo {
        server_name: "test_server".to_string(),
        callable_name,
        callable_namespace,
        server_instructions: None,
        tool,
        connector_id: None,
        connector_name: None,
        plugin_display_names: Vec::new(),
        connector_description: None,
    }
}

fn discoverable_connector(id: &str, name: &str, description: &str) -> DiscoverableTool {
    let slug = name.replace(' ', "-").to_lowercase();
    DiscoverableTool::Connector(Box::new(AppInfo {
        id: id.to_string(),
        name: name.to_string(),
        description: Some(description.to_string()),
        logo_url: None,
        logo_url_dark: None,
        distribution_channel: None,
        branding: None,
        app_metadata: None,
        labels: None,
        install_url: Some(format!("https://chatgpt.com/apps/{slug}/{id}")),
        is_accessible: false,
        is_enabled: true,
        plugin_display_names: Vec::new(),
    }))
}

async fn search_capable_model_info() -> ModelInfo {
    let config = test_config().await;
    let mut model_info = construct_model_info_offline("gpt-5-codex", &config);
    model_info.supports_search_tool = true;
    model_info
}

#[test]
fn deferred_responses_api_tool_serializes_with_defer_loading() {
    let tool = mcp_tool(
        "lookup_order",
        "Look up an order",
        serde_json::json!({
            "type": "object",
            "properties": {
                "order_id": {"type": "string"}
            },
            "required": ["order_id"],
            "additionalProperties": false,
        }),
    );

    let serialized = serde_json::to_value(ToolSpec::Function(
        mcp_tool_to_deferred_responses_api_tool(
            &ToolName::namespaced("mcp__codex_apps__", "lookup_order"),
            &tool,
        )
        .expect("convert deferred tool"),
    ))
    .expect("serialize deferred tool");

    assert_eq!(
        serialized,
        serde_json::json!({
            "type": "function",
            "name": "lookup_order",
            "description": "Look up an order",
            "strict": false,
            "defer_loading": true,
            "parameters": {
                "type": "object",
                "properties": {
                    "order_id": {"type": "string"}
                },
                "required": ["order_id"],
                "additionalProperties": false,
            }
        })
    );
}

// Avoid order-based assertions; compare via set containment instead.
fn assert_contains_tool_names(tools: &[ConfiguredToolSpec], expected_subset: &[&str]) {
    use std::collections::HashSet;
    let mut names = HashSet::new();
    let mut duplicates = Vec::new();
    for name in tools.iter().map(ConfiguredToolSpec::name) {
        if !names.insert(name) {
            duplicates.push(name);
        }
    }
    assert!(
        duplicates.is_empty(),
        "duplicate tool entries detected: {duplicates:?}"
    );
    for expected in expected_subset {
        assert!(
            names.contains(expected),
            "expected tool {expected} to be present; had: {names:?}"
        );
    }
}

fn assert_lacks_tool_name(tools: &[ConfiguredToolSpec], expected_absent: &str) {
    let names = tools
        .iter()
        .map(ConfiguredToolSpec::name)
        .collect::<Vec<_>>();
    assert!(
        !names.contains(&expected_absent),
        "expected tool {expected_absent} to be absent; had: {names:?}"
    );
}

fn shell_tool_name(config: &ToolsConfig) -> Option<&'static str> {
    match config.shell_type {
        ConfigShellToolType::Default => Some("shell"),
        ConfigShellToolType::Local => Some("local_shell"),
        ConfigShellToolType::UnifiedExec => None,
        ConfigShellToolType::Disabled => None,
        ConfigShellToolType::ShellCommand => Some("shell_command"),
    }
}

fn request_user_input_tool_spec(default_mode_request_user_input: bool) -> ToolSpec {
    create_request_user_input_tool(request_user_input_tool_description(
        default_mode_request_user_input,
    ))
}

fn spawn_agent_tool_options(config: &ToolsConfig) -> SpawnAgentToolOptions<'_> {
    SpawnAgentToolOptions {
        available_models: &config.available_models,
        agent_type_description: config.agent_type_description.clone(),
    }
}

fn wait_agent_timeout_options() -> WaitAgentTimeoutOptions {
    WaitAgentTimeoutOptions {
        default_timeout_ms: DEFAULT_WAIT_TIMEOUT_MS,
        min_timeout_ms: MIN_WAIT_TIMEOUT_MS,
        max_timeout_ms: MAX_WAIT_TIMEOUT_MS,
    }
}

fn find_tool<'a>(tools: &'a [ConfiguredToolSpec], expected_name: &str) -> &'a ConfiguredToolSpec {
    tools
        .iter()
        .find(|tool| tool.name() == expected_name)
        .unwrap_or_else(|| panic!("expected tool {expected_name}"))
}

fn strip_descriptions_schema(schema: &mut JsonSchema) {
    match schema {
        JsonSchema::Boolean { description }
        | JsonSchema::String { description }
        | JsonSchema::Number { description } => {
            *description = None;
        }
        JsonSchema::Array { items, description } => {
            strip_descriptions_schema(items);
            *description = None;
        }
        JsonSchema::Object {
            properties,
            required: _,
            additional_properties,
        } => {
            for v in properties.values_mut() {
                strip_descriptions_schema(v);
            }
            if let Some(AdditionalProperties::Schema(s)) = additional_properties {
                strip_descriptions_schema(s);
            }
        }
    }
}

fn strip_descriptions_tool(spec: &mut ToolSpec) {
    match spec {
        ToolSpec::ToolSearch { parameters, .. } => strip_descriptions_schema(parameters),
        ToolSpec::Function(ResponsesApiTool { parameters, .. }) => {
            strip_descriptions_schema(parameters);
        }
        ToolSpec::Freeform(_)
        | ToolSpec::LocalShell {}
        | ToolSpec::ImageGeneration { .. }
        | ToolSpec::WebSearch { .. } => {}
    }
}

async fn model_info_from_models_json(slug: &str) -> ModelInfo {
    let config = test_config().await;
    let response = bundled_models_response()
        .unwrap_or_else(|err| panic!("bundled models.json should parse: {err}"));
    let model = response
        .models
        .into_iter()
        .find(|candidate| candidate.slug == slug)
        .unwrap_or_else(|| panic!("model slug {slug} is missing from models.json"));
    with_config_overrides(model, &config.to_models_manager_config())
}

#[test]
fn unified_exec_is_blocked_for_windows_sandboxed_policies_only() {
    assert!(!unified_exec_allowed_in_environment(
        /*is_windows*/ true,
        &SandboxPolicy::new_read_only_policy(),
        WindowsSandboxLevel::RestrictedToken,
    ));
    assert!(!unified_exec_allowed_in_environment(
        /*is_windows*/ true,
        &SandboxPolicy::new_workspace_write_policy(),
        WindowsSandboxLevel::RestrictedToken,
    ));
    assert!(unified_exec_allowed_in_environment(
        /*is_windows*/ true,
        &SandboxPolicy::DangerFullAccess,
        WindowsSandboxLevel::RestrictedToken,
    ));
    assert!(unified_exec_allowed_in_environment(
        /*is_windows*/ true,
        &SandboxPolicy::DangerFullAccess,
        WindowsSandboxLevel::Disabled,
    ));
}

/// Builds the tool registry builder while collecting tool specs for later serialization.
fn build_specs(
    config: &ToolsConfig,
    mcp_tools: Option<HashMap<String, ToolInfo>>,
    deferred_mcp_tools: Option<HashMap<String, ToolInfo>>,
    dynamic_tools: &[DynamicToolSpec],
) -> ToolRegistryBuilder {
    build_specs_with_unavailable_tools(
        config,
        mcp_tools,
        deferred_mcp_tools,
        Vec::new(),
        dynamic_tools,
    )
}

fn build_specs_with_unavailable_tools(
    config: &ToolsConfig,
    mcp_tools: Option<HashMap<String, ToolInfo>>,
    deferred_mcp_tools: Option<HashMap<String, ToolInfo>>,
    unavailable_called_tools: Vec<ToolName>,
    dynamic_tools: &[DynamicToolSpec],
) -> ToolRegistryBuilder {
    build_specs_with_discoverable_tools(
        config,
        mcp_tools,
        deferred_mcp_tools,
        unavailable_called_tools,
        /*discoverable_tools*/ None,
        dynamic_tools,
    )
}

#[tokio::test]
async fn model_provided_unified_exec_is_blocked_for_windows_sandboxed_policies() {
    let mut model_info = model_info_from_models_json("gpt-5-codex").await;
    model_info.shell_type = ConfigShellToolType::UnifiedExec;
    let features = Features::with_defaults();
    let available_models = Vec::new();
    let config = ToolsConfig::new(&ToolsConfigParams {
        model_info: &model_info,
        available_models: &available_models,
        features: &features,
        image_generation_tool_auth_allowed: true,
        web_search_mode: Some(WebSearchMode::Cached),
        session_source: SessionSource::Cli,
        sandbox_policy: &SandboxPolicy::new_workspace_write_policy(),
        windows_sandbox_level: WindowsSandboxLevel::RestrictedToken,
    });

    let expected_shell_type = if cfg!(target_os = "windows") {
        ConfigShellToolType::ShellCommand
    } else {
        ConfigShellToolType::UnifiedExec
    };
    assert_eq!(config.shell_type, expected_shell_type);
}

#[test]
fn test_full_toolset_specs_for_gpt5_codex_unified_exec_web_search() {
    let model_info = model_info_from_models_json("gpt-5-codex");
    let mut features = Features::with_defaults();
    features.enable(Feature::UnifiedExec);
    let available_models = Vec::new();
    let config = ToolsConfig::new(&ToolsConfigParams {
        model_info: &model_info,
        available_models: &available_models,
        features: &features,
        web_search_mode: Some(WebSearchMode::Live),
        session_source: SessionSource::Cli,
        sandbox_policy: &SandboxPolicy::DangerFullAccess,
        windows_sandbox_level: WindowsSandboxLevel::Disabled,
    });
    let (tools, _) = build_specs(
        &config,
        /*mcp_tools*/ None,
        /*app_tools*/ None,
        &[],
    )
    .build();

    // Build actual map name -> spec
    use std::collections::BTreeMap;
    use std::collections::HashSet;
    let mut actual: BTreeMap<String, ToolSpec> = BTreeMap::from([]);
    let mut duplicate_names = Vec::new();
    for t in &tools {
        let name = t.name().to_string();
        if actual.insert(name.clone(), t.spec.clone()).is_some() {
            duplicate_names.push(name);
        }
    }
    assert!(
        duplicate_names.is_empty(),
        "duplicate tool entries detected: {duplicate_names:?}"
    );

    // Build expected from the same helpers used by the builder.
    let mut expected: BTreeMap<String, ToolSpec> = BTreeMap::from([]);
    for spec in [
        create_exec_command_tool(CommandToolOptions {
            allow_login_shell: true,
            exec_permission_approvals_enabled: false,
        }),
        create_write_stdin_tool(),
        create_update_plan_tool(),
        request_user_input_tool_spec(/*default_mode_request_user_input*/ false),
        create_apply_patch_freeform_tool(),
        ToolSpec::WebSearch {
            external_web_access: Some(true),
            filters: None,
            user_location: None,
            search_context_size: None,
            search_content_types: None,
        },
        create_view_image_tool(ViewImageToolOptions {
            can_request_original_image_detail: config.can_request_original_image_detail,
        }),
    ] {
        expected.insert(spec.name().to_string(), spec);
    }
    let collab_specs = if config.multi_agent_v2 {
        vec![
            create_spawn_agent_tool_v2(spawn_agent_tool_options(&config)),
            create_send_message_tool(),
            create_wait_agent_tool_v2(wait_agent_timeout_options()),
            create_close_agent_tool_v2(),
        ]
    } else {
        vec![
            create_spawn_agent_tool_v1(spawn_agent_tool_options(&config)),
            create_send_input_tool_v1(),
            create_wait_agent_tool_v1(wait_agent_timeout_options()),
            create_close_agent_tool_v1(),
        ]
    };
    for spec in collab_specs {
        expected.insert(spec.name().to_string(), spec);
    }
    if !config.multi_agent_v2 {
        let spec = create_resume_agent_tool();
        expected.insert(spec.name().to_string(), spec);
    }

    if config.exec_permission_approvals_enabled {
        let spec = create_request_permissions_tool(request_permissions_tool_description());
        expected.insert(spec.name().to_string(), spec);
    }

    // Exact name set match — this is the only test allowed to fail when tools change.
    let actual_names: HashSet<_> = actual.keys().cloned().collect();
    let expected_names: HashSet<_> = expected.keys().cloned().collect();
    assert_eq!(actual_names, expected_names, "tool name set mismatch");

    // Compare specs ignoring human-readable descriptions.
    for name in expected.keys() {
        let mut a = actual.get(name).expect("present").clone();
        let mut e = expected.get(name).expect("present").clone();
        strip_descriptions_tool(&mut a);
        strip_descriptions_tool(&mut e);
        assert_eq!(a, e, "spec mismatch for {name}");
    }
}

#[test]
fn test_build_specs_collab_tools_enabled() {
    let config = test_config();
    let model_info = ModelsManager::construct_model_info_offline_for_tests("gpt-5-codex", &config);
    let mut features = Features::with_defaults();
    features.enable(Feature::Collab);
    let available_models = Vec::new();
    let tools_config = ToolsConfig::new(&ToolsConfigParams {
        model_info: &model_info,
        available_models: &available_models,
        features: &features,
        web_search_mode: Some(WebSearchMode::Cached),
        session_source: SessionSource::Cli,
        sandbox_policy: &SandboxPolicy::DangerFullAccess,
        windows_sandbox_level: WindowsSandboxLevel::Disabled,
    });
    let (tools, _) = build_specs(
        &tools_config,
        /*mcp_tools*/ None,
        /*app_tools*/ None,
        &[],
    )
    .build();
    assert_contains_tool_names(
        &tools,
        &["spawn_agent", "send_input", "wait_agent", "close_agent"],
    );
    assert_lacks_tool_name(&tools, "spawn_agents_on_csv");
    assert_lacks_tool_name(&tools, "list_agents");
}

#[test]
fn test_build_specs_multi_agent_v2_uses_task_names_and_hides_resume() {
    let config = test_config();
    let model_info = ModelsManager::construct_model_info_offline_for_tests("gpt-5-codex", &config);
    let mut features = Features::with_defaults();
    features.enable(Feature::Collab);
    features.enable(Feature::MultiAgentV2);
    let available_models = Vec::new();
    let tools_config = ToolsConfig::new(&ToolsConfigParams {
        model_info: &model_info,
        available_models: &available_models,
        features: &features,
        web_search_mode: Some(WebSearchMode::Cached),
        session_source: SessionSource::Cli,
        sandbox_policy: &SandboxPolicy::DangerFullAccess,
        windows_sandbox_level: WindowsSandboxLevel::Disabled,
    });
    let (tools, _) = build_specs(
        &tools_config,
        /*mcp_tools*/ None,
        /*app_tools*/ None,
        &[],
    )
    .build();
    assert_contains_tool_names(
        &tools,
        &[
            "spawn_agent",
            "send_message",
            "assign_task",
            "wait_agent",
            "close_agent",
            "list_agents",
        ],
    );

    let spawn_agent = find_tool(&tools, "spawn_agent");
    let ToolSpec::Function(ResponsesApiTool {
        parameters,
        output_schema,
        ..
    }) = &spawn_agent.spec
    else {
        panic!("spawn_agent should be a function tool");
    };
    let JsonSchema::Object {
        properties,
        required,
        ..
    } = parameters
    else {
        panic!("spawn_agent should use object params");
    };
    assert!(properties.contains_key("task_name"));
    assert_eq!(required.as_ref(), Some(&vec!["task_name".to_string()]));
    let output_schema = output_schema
        .as_ref()
        .expect("spawn_agent should define output schema");
    assert_eq!(
        output_schema["required"],
        json!(["agent_id", "task_name", "nickname"])
    );

    let send_message = find_tool(&tools, "send_message");
    let ToolSpec::Function(ResponsesApiTool { parameters, .. }) = &send_message.spec else {
        panic!("send_message should be a function tool");
    };
    let JsonSchema::Object {
        properties,
        required,
        ..
    } = parameters
    else {
        panic!("send_message should use object params");
    };
    assert!(properties.contains_key("target"));
    assert!(!properties.contains_key("message"));
    assert_eq!(
        required.as_ref(),
        Some(&vec!["target".to_string(), "items".to_string()])
    );

    let assign_task = find_tool(&tools, "assign_task");
    let ToolSpec::Function(ResponsesApiTool { parameters, .. }) = &assign_task.spec else {
        panic!("assign_task should be a function tool");
    };
    let JsonSchema::Object {
        properties,
        required,
        ..
    } = parameters
    else {
        panic!("assign_task should use object params");
    };
    assert!(properties.contains_key("target"));
    assert!(!properties.contains_key("message"));
    assert_eq!(
        required.as_ref(),
        Some(&vec!["target".to_string(), "items".to_string()])
    );

    let wait_agent = find_tool(&tools, "wait_agent");
    let ToolSpec::Function(ResponsesApiTool {
        parameters,
        output_schema,
        ..
    }) = &wait_agent.spec
    else {
        panic!("wait_agent should be a function tool");
    };
    let JsonSchema::Object {
        properties,
        required,
        ..
    } = parameters
    else {
        panic!("wait_agent should use object params");
    };
    assert!(properties.contains_key("targets"));
    assert_eq!(required.as_ref(), Some(&vec!["targets".to_string()]));
    let output_schema = output_schema
        .as_ref()
        .expect("wait_agent should define output schema");
    assert_eq!(
        output_schema["properties"]["message"]["description"],
        json!("Brief wait summary without the agent's final content.")
    );

    let list_agents = find_tool(&tools, "list_agents");
    let ToolSpec::Function(ResponsesApiTool {
        parameters,
        output_schema,
        ..
    }) = &list_agents.spec
    else {
        panic!("list_agents should be a function tool");
    };
    let JsonSchema::Object {
        properties,
        required,
        ..
    } = parameters
    else {
        panic!("list_agents should use object params");
    };
    assert!(properties.contains_key("path_prefix"));
    assert_eq!(required.as_ref(), None);
    let output_schema = output_schema
        .as_ref()
        .expect("list_agents should define output schema");
    assert_eq!(
        output_schema["properties"]["agents"]["items"]["required"],
        json!(["agent_name", "agent_status", "last_task_message"])
    );
    assert_lacks_tool_name(&tools, "send_input");
    assert_lacks_tool_name(&tools, "resume_agent");
}

#[test]
fn test_build_specs_enable_fanout_enables_agent_jobs_and_collab_tools() {
    let config = test_config();
    let model_info = ModelsManager::construct_model_info_offline_for_tests("gpt-5-codex", &config);
    let mut features = Features::with_defaults();
    features.enable(Feature::SpawnCsv);
    features.normalize_dependencies();
    let available_models = Vec::new();
    let tools_config = ToolsConfig::new(&ToolsConfigParams {
        model_info: &model_info,
        available_models: &available_models,
        features: &features,
        web_search_mode: Some(WebSearchMode::Cached),
        session_source: SessionSource::Cli,
        sandbox_policy: &SandboxPolicy::DangerFullAccess,
        windows_sandbox_level: WindowsSandboxLevel::Disabled,
    });
    let (tools, _) = build_specs(
        &tools_config,
        /*mcp_tools*/ None,
        /*app_tools*/ None,
        &[],
    )
    .build();
    assert_contains_tool_names(
        &tools,
        &[
            "spawn_agent",
            "send_input",
            "wait_agent",
            "close_agent",
            "spawn_agents_on_csv",
        ],
    );
}

#[test]
fn view_image_tool_omits_detail_without_original_detail_feature() {
    let config = test_config();
    let mut model_info =
        ModelsManager::construct_model_info_offline_for_tests("gpt-5-codex", &config);
    model_info.supports_image_detail_original = true;
    let features = Features::with_defaults();
    let available_models = Vec::new();
    let tools_config = ToolsConfig::new(&ToolsConfigParams {
        model_info: &model_info,
        available_models: &available_models,
        features: &features,
        web_search_mode: Some(WebSearchMode::Cached),
        session_source: SessionSource::Cli,
        sandbox_policy: &SandboxPolicy::DangerFullAccess,
        windows_sandbox_level: WindowsSandboxLevel::Disabled,
    });
    let (tools, _) = build_specs(
        &tools_config,
        /*mcp_tools*/ None,
        /*app_tools*/ None,
        &[],
    )
    .build();
    let view_image = find_tool(&tools, VIEW_IMAGE_TOOL_NAME);
    let ToolSpec::Function(ResponsesApiTool { parameters, .. }) = &view_image.spec else {
        panic!("view_image should be a function tool");
    };
    let JsonSchema::Object { properties, .. } = parameters else {
        panic!("view_image should use an object schema");
    };
    assert!(!properties.contains_key("detail"));
}

#[test]
fn view_image_tool_includes_detail_with_original_detail_feature() {
    let config = test_config();
    let mut model_info =
        ModelsManager::construct_model_info_offline_for_tests("gpt-5-codex", &config);
    model_info.supports_image_detail_original = true;
    let mut features = Features::with_defaults();
    features.enable(Feature::ImageDetailOriginal);
    let available_models = Vec::new();
    let tools_config = ToolsConfig::new(&ToolsConfigParams {
        model_info: &model_info,
        available_models: &available_models,
        features: &features,
        web_search_mode: Some(WebSearchMode::Cached),
        session_source: SessionSource::Cli,
        sandbox_policy: &SandboxPolicy::DangerFullAccess,
        windows_sandbox_level: WindowsSandboxLevel::Disabled,
    });
    let (tools, _) = build_specs(
        &tools_config,
        /*mcp_tools*/ None,
        /*app_tools*/ None,
        &[],
    )
    .build();
    let view_image = find_tool(&tools, VIEW_IMAGE_TOOL_NAME);
    let ToolSpec::Function(ResponsesApiTool { parameters, .. }) = &view_image.spec else {
        panic!("view_image should be a function tool");
    };
    let JsonSchema::Object { properties, .. } = parameters else {
        panic!("view_image should use an object schema");
    };
    assert!(properties.contains_key("detail"));
    let Some(JsonSchema::String {
        description: Some(description),
    }) = properties.get("detail")
    else {
        panic!("view_image detail should include a description");
    };
    assert!(description.contains("only supported value is `original`"));
    assert!(description.contains("omit this field for default resized behavior"));
}

#[test]
fn test_build_specs_agent_job_worker_tools_enabled() {
    let config = test_config();
    let model_info = ModelsManager::construct_model_info_offline_for_tests("gpt-5-codex", &config);
    let mut features = Features::with_defaults();
    features.enable(Feature::SpawnCsv);
    features.normalize_dependencies();
    features.enable(Feature::Sqlite);
    let available_models = Vec::new();
    let tools_config = ToolsConfig::new(&ToolsConfigParams {
        model_info: &model_info,
        available_models: &available_models,
        features: &features,
        web_search_mode: Some(WebSearchMode::Cached),
        session_source: SessionSource::SubAgent(SubAgentSource::Other(
            "agent_job:test".to_string(),
        )),
        sandbox_policy: &SandboxPolicy::DangerFullAccess,
        windows_sandbox_level: WindowsSandboxLevel::Disabled,
    });
    let (tools, _) = build_specs(
        &tools_config,
        /*mcp_tools*/ None,
        /*app_tools*/ None,
        &[],
    )
    .build();
    assert_contains_tool_names(
        &tools,
        &[
            "spawn_agent",
            "send_input",
            "resume_agent",
            "wait_agent",
            "close_agent",
            "spawn_agents_on_csv",
            "report_agent_job_result",
        ],
    );
    assert_lacks_tool_name(&tools, "request_user_input");
}

#[test]
fn request_user_input_description_reflects_default_mode_feature_flag() {
    let config = test_config();
    let model_info = ModelsManager::construct_model_info_offline_for_tests("gpt-5-codex", &config);
    let mut features = Features::with_defaults();
    let available_models = Vec::new();
    let tools_config = ToolsConfig::new(&ToolsConfigParams {
        model_info: &model_info,
        available_models: &available_models,
        features: &features,
        web_search_mode: Some(WebSearchMode::Cached),
        session_source: SessionSource::Cli,
        sandbox_policy: &SandboxPolicy::DangerFullAccess,
        windows_sandbox_level: WindowsSandboxLevel::Disabled,
    });
    let (tools, _) = build_specs(
        &tools_config,
        /*mcp_tools*/ None,
        /*app_tools*/ None,
        &[],
    )
    .build();
    let request_user_input_tool = find_tool(&tools, "request_user_input");
    assert_eq!(
        request_user_input_tool.spec,
        request_user_input_tool_spec(/*default_mode_request_user_input*/ false)
    );

    features.enable(Feature::DefaultModeRequestUserInput);
    let available_models = Vec::new();
    let tools_config = ToolsConfig::new(&ToolsConfigParams {
        model_info: &model_info,
        available_models: &available_models,
        features: &features,
        web_search_mode: Some(WebSearchMode::Cached),
        session_source: SessionSource::Cli,
        sandbox_policy: &SandboxPolicy::DangerFullAccess,
        windows_sandbox_level: WindowsSandboxLevel::Disabled,
    });
    let (tools, _) = build_specs(
        &tools_config,
        /*mcp_tools*/ None,
        /*app_tools*/ None,
        &[],
    )
    .build();
    let request_user_input_tool = find_tool(&tools, "request_user_input");
    assert_eq!(
        request_user_input_tool.spec,
        request_user_input_tool_spec(/*default_mode_request_user_input*/ true)
    );
}

#[test]
fn request_permissions_requires_feature_flag() {
    let config = test_config();
    let model_info = ModelsManager::construct_model_info_offline_for_tests("gpt-5-codex", &config);
    let features = Features::with_defaults();
    let available_models = Vec::new();
    let tools_config = ToolsConfig::new(&ToolsConfigParams {
        model_info: &model_info,
        available_models: &available_models,
        features: &features,
        web_search_mode: Some(WebSearchMode::Cached),
        session_source: SessionSource::Cli,
        sandbox_policy: &SandboxPolicy::DangerFullAccess,
        windows_sandbox_level: WindowsSandboxLevel::Disabled,
    });
    let (tools, _) = build_specs(
        &tools_config,
        /*mcp_tools*/ None,
        /*app_tools*/ None,
        &[],
    )
    .build();
    assert_lacks_tool_name(&tools, "request_permissions");

    let mut features = Features::with_defaults();
    features.enable(Feature::RequestPermissionsTool);
    let available_models = Vec::new();
    let tools_config = ToolsConfig::new(&ToolsConfigParams {
        model_info: &model_info,
        available_models: &available_models,
        features: &features,
        web_search_mode: Some(WebSearchMode::Cached),
        session_source: SessionSource::Cli,
        sandbox_policy: &SandboxPolicy::DangerFullAccess,
        windows_sandbox_level: WindowsSandboxLevel::Disabled,
    });
    let (tools, _) = build_specs(
        &tools_config,
        /*mcp_tools*/ None,
        /*app_tools*/ None,
        &[],
    )
    .build();
    let request_permissions_tool = find_tool(&tools, "request_permissions");
    assert_eq!(
        request_permissions_tool.spec,
        create_request_permissions_tool(request_permissions_tool_description())
    );
}

#[test]
fn request_permissions_tool_is_independent_from_additional_permissions() {
    let config = test_config();
    let model_info = ModelsManager::construct_model_info_offline_for_tests("gpt-5-codex", &config);
    let mut features = Features::with_defaults();
    features.enable(Feature::ExecPermissionApprovals);
    let available_models = Vec::new();
    let tools_config = ToolsConfig::new(&ToolsConfigParams {
        model_info: &model_info,
        available_models: &available_models,
        features: &features,
        web_search_mode: Some(WebSearchMode::Cached),
        session_source: SessionSource::Cli,
        sandbox_policy: &SandboxPolicy::DangerFullAccess,
        windows_sandbox_level: WindowsSandboxLevel::Disabled,
    });
    let (tools, _) = build_specs(
        &tools_config,
        /*mcp_tools*/ None,
        /*app_tools*/ None,
        &[],
    )
    .build();

    assert_lacks_tool_name(&tools, "request_permissions");
}

#[test]
fn get_memory_requires_feature_flag() {
    let config = test_config();
    let model_info = construct_model_info_offline("gpt-5-codex", &config);
    let mut features = Features::with_defaults();
    features.disable(Feature::MemoryTool);
    let available_models = Vec::new();
    let tools_config = ToolsConfig::new(&ToolsConfigParams {
        model_info: &model_info,
        available_models: &available_models,
        features: &features,
        image_generation_tool_auth_allowed: true,
        web_search_mode: Some(WebSearchMode::Cached),
        session_source: SessionSource::Cli,
        sandbox_policy: &SandboxPolicy::DangerFullAccess,
        windows_sandbox_level: WindowsSandboxLevel::Disabled,
    });
    let (tools, _) = build_specs(
        &tools_config,
        /*mcp_tools*/ None,
        /*deferred_mcp_tools*/ None,
        &[],
    )
    .build();
    assert!(
        !tools.iter().any(|t| t.spec.name() == "get_memory"),
        "get_memory should be disabled when memory_tool feature is off"
    );
}

#[test]
fn js_repl_requires_feature_flag() {
    let config = test_config();
    let model_info = ModelsManager::construct_model_info_offline_for_tests("gpt-5-codex", &config);
    let features = Features::with_defaults();

    let available_models = Vec::new();
    let tools_config = ToolsConfig::new(&ToolsConfigParams {
        model_info: &model_info,
        available_models: &available_models,
        features: &features,
        web_search_mode: Some(WebSearchMode::Cached),
        session_source: SessionSource::Cli,
        sandbox_policy: &SandboxPolicy::DangerFullAccess,
        windows_sandbox_level: WindowsSandboxLevel::Disabled,
    });
    let (tools, _) = build_specs(
        &tools_config,
        /*mcp_tools*/ None,
        /*app_tools*/ None,
        &[],
    )
    .build();

    assert!(
        !tools.iter().any(|tool| tool.spec.name() == "js_repl"),
        "js_repl should be disabled when the feature is off"
    );
    assert!(
        !tools.iter().any(|tool| tool.spec.name() == "js_repl_reset"),
        "js_repl_reset should be disabled when the feature is off"
    );
}

#[test]
fn js_repl_enabled_adds_tools() {
    let config = test_config();
    let model_info = ModelsManager::construct_model_info_offline_for_tests("gpt-5-codex", &config);
    let mut features = Features::with_defaults();
    features.enable(Feature::JsRepl);

    let available_models = Vec::new();
    let tools_config = ToolsConfig::new(&ToolsConfigParams {
        model_info: &model_info,
        available_models: &available_models,
        features: &features,
        web_search_mode: Some(WebSearchMode::Cached),
        session_source: SessionSource::Cli,
        sandbox_policy: &SandboxPolicy::DangerFullAccess,
        windows_sandbox_level: WindowsSandboxLevel::Disabled,
    });
    let (tools, _) = build_specs(
        &tools_config,
        /*mcp_tools*/ None,
        /*app_tools*/ None,
        &[],
    )
    .build();
    assert_contains_tool_names(&tools, &["js_repl", "js_repl_reset"]);
}

#[test]
fn image_generation_tools_require_feature_and_supported_model() {
    let config = test_config();
    let mut supported_model_info =
        ModelsManager::construct_model_info_offline_for_tests("gpt-5.2", &config);
    supported_model_info.slug = "custom/gpt-5.2-variant".to_string();
    let mut unsupported_model_info = supported_model_info.clone();
    unsupported_model_info.input_modalities = vec![InputModality::Text];
    let default_features = Features::with_defaults();
    let mut image_generation_features = default_features.clone();
    image_generation_features.enable(Feature::ImageGeneration);

    let available_models = Vec::new();
    let default_tools_config = ToolsConfig::new(&ToolsConfigParams {
        model_info: &supported_model_info,
        available_models: &available_models,
        features: &default_features,
        web_search_mode: Some(WebSearchMode::Cached),
        session_source: SessionSource::Cli,
        sandbox_policy: &SandboxPolicy::DangerFullAccess,
        windows_sandbox_level: WindowsSandboxLevel::Disabled,
    });
    let (default_tools, _) = build_specs(
        &default_tools_config,
        /*mcp_tools*/ None,
        /*app_tools*/ None,
        &[],
    )
    .build();
    assert!(
        !default_tools
            .iter()
            .any(|tool| tool.spec.name() == "image_generation"),
        "image_generation should be disabled by default"
    );

    let supported_tools_config = ToolsConfig::new(&ToolsConfigParams {
        model_info: &supported_model_info,
        available_models: &available_models,
        features: &image_generation_features,
        web_search_mode: Some(WebSearchMode::Cached),
        session_source: SessionSource::Cli,
        sandbox_policy: &SandboxPolicy::DangerFullAccess,
        windows_sandbox_level: WindowsSandboxLevel::Disabled,
    });
    let (supported_tools, _) = build_specs(
        &supported_tools_config,
        /*mcp_tools*/ None,
        /*app_tools*/ None,
        &[],
    )
    .build();
    assert_contains_tool_names(&supported_tools, &["image_generation"]);
    let image_generation_tool = find_tool(&supported_tools, "image_generation");
    assert_eq!(
        serde_json::to_value(&image_generation_tool.spec).expect("serialize image tool"),
        serde_json::json!({
            "type": "image_generation",
            "output_format": "png"
        })
    );

    let tools_config = ToolsConfig::new(&ToolsConfigParams {
        model_info: &unsupported_model_info,
        available_models: &available_models,
        features: &image_generation_features,
        web_search_mode: Some(WebSearchMode::Cached),
        session_source: SessionSource::Cli,
        sandbox_policy: &SandboxPolicy::DangerFullAccess,
        windows_sandbox_level: WindowsSandboxLevel::Disabled,
    });
    let (tools, _) = build_specs(
        &tools_config,
        /*mcp_tools*/ None,
        /*app_tools*/ None,
        &[],
    )
    .build();
    assert!(
        !tools
            .iter()
            .any(|tool| tool.spec.name() == "image_generation"),
        "image_generation should be disabled for unsupported models"
    );
}

fn assert_model_tools(
    model_slug: &str,
    features: &Features,
    web_search_mode: Option<WebSearchMode>,
    expected_tools: &[&str],
) {
    let _config = test_config().await;
    let model_info = model_info_from_models_json(model_slug).await;
    let available_models = Vec::new();
    let tools_config = ToolsConfig::new(&ToolsConfigParams {
        model_info: &model_info,
        available_models: &available_models,
        features,
        image_generation_tool_auth_allowed: true,
        web_search_mode,
        session_source: SessionSource::Cli,
        sandbox_policy: &SandboxPolicy::DangerFullAccess,
        windows_sandbox_level: WindowsSandboxLevel::Disabled,
    });
    let router = ToolRouter::from_config(
        &tools_config,
        ToolRouterParams {
            mcp_tools: None,
            deferred_mcp_tools: None,
            unavailable_called_tools: Vec::new(),
            parallel_mcp_server_names: std::collections::HashSet::new(),
            discoverable_tools: None,
            dynamic_tools: &[],
        },
    );
    let model_visible_specs = router.model_visible_specs();
    let tool_names = model_visible_specs
        .iter()
        .map(ToolSpec::name)
        .collect::<Vec<_>>();
    assert_eq!(&tool_names, &expected_tools,);
}

async fn assert_default_model_tools(
    model_slug: &str,
    features: &Features,
    web_search_mode: Option<WebSearchMode>,
    shell_tool: &'static str,
    expected_tail: &[&str],
) {
    let mut expected = if features.enabled(Feature::UnifiedExec) {
        vec!["exec_command", "write_stdin"]
    } else {
        vec![shell_tool]
    };
    expected.extend(expected_tail);
    assert_model_tools(model_slug, features, web_search_mode, &expected).await;
}

#[test]
fn web_search_mode_cached_sets_external_web_access_false() {
    let config = test_config();
    let model_info = ModelsManager::construct_model_info_offline_for_tests("gpt-5-codex", &config);
    let features = Features::with_defaults();

    let available_models = Vec::new();
    let tools_config = ToolsConfig::new(&ToolsConfigParams {
        model_info: &model_info,
        available_models: &available_models,
        features: &features,
        web_search_mode: Some(WebSearchMode::Cached),
        session_source: SessionSource::Cli,
        sandbox_policy: &SandboxPolicy::DangerFullAccess,
        windows_sandbox_level: WindowsSandboxLevel::Disabled,
    });
    let (tools, _) = build_specs(
        &tools_config,
        /*mcp_tools*/ None,
        /*app_tools*/ None,
        &[],
    )
    .build();

    let tool = find_tool(&tools, "web_search");
    assert_eq!(
        tool.spec,
        ToolSpec::WebSearch {
            external_web_access: Some(false),
            filters: None,
            user_location: None,
            search_context_size: None,
            search_content_types: None,
        }
    );
}

#[test]
fn web_search_mode_live_sets_external_web_access_true() {
    let config = test_config();
    let model_info = ModelsManager::construct_model_info_offline_for_tests("gpt-5-codex", &config);
    let features = Features::with_defaults();

    let available_models = Vec::new();
    let tools_config = ToolsConfig::new(&ToolsConfigParams {
        model_info: &model_info,
        available_models: &available_models,
        features: &features,
        web_search_mode: Some(WebSearchMode::Live),
        session_source: SessionSource::Cli,
        sandbox_policy: &SandboxPolicy::DangerFullAccess,
        windows_sandbox_level: WindowsSandboxLevel::Disabled,
    });
    let (tools, _) = build_specs(
        &tools_config,
        /*mcp_tools*/ None,
        /*app_tools*/ None,
        &[],
    )
    .build();

    let tool = find_tool(&tools, "web_search");
    assert_eq!(
        tool.spec,
        ToolSpec::WebSearch {
            external_web_access: Some(true),
            filters: None,
            user_location: None,
            search_context_size: None,
            search_content_types: None,
        }
    );
}

#[test]
fn web_search_config_is_forwarded_to_tool_spec() {
    let config = test_config();
    let model_info = ModelsManager::construct_model_info_offline_for_tests("gpt-5-codex", &config);
    let features = Features::with_defaults();
    let web_search_config = WebSearchConfig {
        filters: Some(codex_protocol::config_types::WebSearchFilters {
            allowed_domains: Some(vec!["example.com".to_string()]),
        }),
        user_location: Some(codex_protocol::config_types::WebSearchUserLocation {
            r#type: codex_protocol::config_types::WebSearchUserLocationType::Approximate,
            country: Some("US".to_string()),
            region: Some("California".to_string()),
            city: Some("San Francisco".to_string()),
            timezone: Some("America/Los_Angeles".to_string()),
        }),
        search_context_size: Some(codex_protocol::config_types::WebSearchContextSize::High),
    };

    let available_models = Vec::new();
    let tools_config = ToolsConfig::new(&ToolsConfigParams {
        model_info: &model_info,
        available_models: &available_models,
        features: &features,
        web_search_mode: Some(WebSearchMode::Live),
        session_source: SessionSource::Cli,
        sandbox_policy: &SandboxPolicy::DangerFullAccess,
        windows_sandbox_level: WindowsSandboxLevel::Disabled,
    })
    .with_web_search_config(Some(web_search_config.clone()));
    let (tools, _) = build_specs(
        &tools_config,
        /*mcp_tools*/ None,
        /*app_tools*/ None,
        &[],
    )
    .build();

    let tool = find_tool(&tools, "web_search");
    assert_eq!(
        tool.spec,
        ToolSpec::WebSearch {
            external_web_access: Some(true),
            filters: web_search_config
                .filters
                .map(ResponsesApiWebSearchFilters::from),
            user_location: web_search_config
                .user_location
                .map(ResponsesApiWebSearchUserLocation::from),
            search_context_size: web_search_config.search_context_size,
            search_content_types: None,
        }
    );
}

#[test]
fn web_search_tool_type_text_and_image_sets_search_content_types() {
    let config = test_config();
    let mut model_info =
        ModelsManager::construct_model_info_offline_for_tests("gpt-5-codex", &config);
    model_info.web_search_tool_type = WebSearchToolType::TextAndImage;
    let features = Features::with_defaults();

    let available_models = Vec::new();
    let tools_config = ToolsConfig::new(&ToolsConfigParams {
        model_info: &model_info,
        available_models: &available_models,
        features: &features,
        web_search_mode: Some(WebSearchMode::Live),
        session_source: SessionSource::Cli,
        sandbox_policy: &SandboxPolicy::DangerFullAccess,
        windows_sandbox_level: WindowsSandboxLevel::Disabled,
    });
    let (tools, _) = build_specs(
        &tools_config,
        /*mcp_tools*/ None,
        /*app_tools*/ None,
        &[],
    )
    .build();

    let tool = find_tool(&tools, "web_search");
    assert_eq!(
        tool.spec,
        ToolSpec::WebSearch {
            external_web_access: Some(true),
            filters: None,
            user_location: None,
            search_context_size: None,
            search_content_types: Some(
                WEB_SEARCH_CONTENT_TYPES
                    .into_iter()
                    .map(str::to_string)
                    .collect()
            ),
        }
    );
}

#[test]
fn mcp_resource_tools_are_hidden_without_mcp_servers() {
    let config = test_config();
    let model_info = ModelsManager::construct_model_info_offline_for_tests("gpt-5-codex", &config);
    let features = Features::with_defaults();
    let available_models = Vec::new();
    let tools_config = ToolsConfig::new(&ToolsConfigParams {
        model_info: &model_info,
        available_models: &available_models,
        features: &features,
        web_search_mode: Some(WebSearchMode::Cached),
        session_source: SessionSource::Cli,
        sandbox_policy: &SandboxPolicy::DangerFullAccess,
        windows_sandbox_level: WindowsSandboxLevel::Disabled,
    });
    let (tools, _) = build_specs(
        &tools_config,
        /*mcp_tools*/ None,
        /*app_tools*/ None,
        &[],
    )
    .build();

    assert!(
        !tools.iter().any(|tool| matches!(
            tool.spec.name(),
            "list_mcp_resources" | "list_mcp_resource_templates" | "read_mcp_resource"
        )),
        "MCP resource tools should be omitted when no MCP servers are configured"
    );
}

#[test]
fn mcp_resource_tools_are_included_when_mcp_servers_are_present() {
    let config = test_config();
    let model_info = ModelsManager::construct_model_info_offline_for_tests("gpt-5-codex", &config);
    let features = Features::with_defaults();
    let available_models = Vec::new();
    let tools_config = ToolsConfig::new(&ToolsConfigParams {
        model_info: &model_info,
        available_models: &available_models,
        features: &features,
        web_search_mode: Some(WebSearchMode::Cached),
        session_source: SessionSource::Cli,
        sandbox_policy: &SandboxPolicy::DangerFullAccess,
        windows_sandbox_level: WindowsSandboxLevel::Disabled,
    });
    let (tools, _) = build_specs(
        &tools_config,
        Some(HashMap::new()),
        /*app_tools*/ None,
        &[],
    )
    .build();

    assert_contains_tool_names(
        &tools,
        &[
            "list_mcp_resources",
            "list_mcp_resource_templates",
            "read_mcp_resource",
        ],
    );
}

#[test]
fn test_build_specs_gpt5_codex_default() {
    let features = Features::with_defaults();
    assert_default_model_tools(
        "gpt-5-codex",
        &features,
        Some(WebSearchMode::Cached),
        "shell_command",
        &[
            "update_plan",
            "request_user_input",
            "apply_patch",
            "web_search",
            "view_image",
            "spawn_agent",
            "send_input",
            "resume_agent",
            "wait_agent",
            "close_agent",
        ],
    )
    .await;
}

#[tokio::test]
async fn test_build_specs_gpt51_codex_default() {
    let features = Features::with_defaults();
    assert_default_model_tools(
        "gpt-5.1-codex",
        &features,
        Some(WebSearchMode::Cached),
        "shell_command",
        &[
            "update_plan",
            "request_user_input",
            "apply_patch",
            "web_search",
            "view_image",
            "spawn_agent",
            "send_input",
            "resume_agent",
            "wait_agent",
            "close_agent",
        ],
    )
    .await;
}

#[tokio::test]
async fn test_build_specs_gpt5_codex_unified_exec_web_search() {
    let mut features = Features::with_defaults();
    features.enable(Feature::UnifiedExec);
    assert_model_tools(
        "gpt-5-codex",
        &features,
        Some(WebSearchMode::Live),
        &[
            "exec_command",
            "write_stdin",
            "update_plan",
            "request_user_input",
            "apply_patch",
            "web_search",
            "view_image",
            "spawn_agent",
            "send_input",
            "resume_agent",
            "wait_agent",
            "close_agent",
        ],
    )
    .await;
}

#[tokio::test]
async fn test_build_specs_gpt51_codex_unified_exec_web_search() {
    let mut features = Features::with_defaults();
    features.enable(Feature::UnifiedExec);
    assert_model_tools(
        "gpt-5.1-codex",
        &features,
        Some(WebSearchMode::Live),
        &[
            "exec_command",
            "write_stdin",
            "update_plan",
            "request_user_input",
            "apply_patch",
            "web_search",
            "view_image",
            "spawn_agent",
            "send_input",
            "resume_agent",
            "wait_agent",
            "close_agent",
        ],
    )
    .await;
}

#[tokio::test]
async fn test_gpt_5_1_codex_max_defaults() {
    let features = Features::with_defaults();
    assert_default_model_tools(
        "gpt-5.1-codex-max",
        &features,
        Some(WebSearchMode::Cached),
        "shell_command",
        &[
            "update_plan",
            "request_user_input",
            "apply_patch",
            "web_search",
            "view_image",
            "spawn_agent",
            "send_input",
            "resume_agent",
            "wait_agent",
            "close_agent",
        ],
    )
    .await;
}

#[tokio::test]
async fn test_codex_5_1_mini_defaults() {
    let features = Features::with_defaults();
    assert_default_model_tools(
        "gpt-5.1-codex-mini",
        &features,
        Some(WebSearchMode::Cached),
        "shell_command",
        &[
            "update_plan",
            "request_user_input",
            "apply_patch",
            "web_search",
            "view_image",
            "spawn_agent",
            "send_input",
            "resume_agent",
            "wait_agent",
            "close_agent",
        ],
    )
    .await;
}

#[tokio::test]
async fn test_gpt_5_defaults() {
    let features = Features::with_defaults();
    assert_default_model_tools(
        "gpt-5",
        &features,
        Some(WebSearchMode::Cached),
        "shell",
        &[
            "update_plan",
            "request_user_input",
            "web_search",
            "view_image",
            "spawn_agent",
            "send_input",
            "resume_agent",
            "wait_agent",
            "close_agent",
        ],
    )
    .await;
}

#[tokio::test]
async fn test_gpt_5_1_defaults() {
    let features = Features::with_defaults();
    assert_default_model_tools(
        "gpt-5.1",
        &features,
        Some(WebSearchMode::Cached),
        "shell_command",
        &[
            "update_plan",
            "request_user_input",
            "apply_patch",
            "web_search",
            "view_image",
            "spawn_agent",
            "send_input",
            "resume_agent",
            "wait_agent",
            "close_agent",
        ],
    )
    .await;
}

#[tokio::test]
async fn test_gpt_5_1_codex_max_unified_exec_web_search() {
    let mut features = Features::with_defaults();
    features.enable(Feature::UnifiedExec);
    assert_model_tools(
        "gpt-5.1-codex-max",
        &features,
        Some(WebSearchMode::Live),
        &[
            "exec_command",
            "write_stdin",
            "update_plan",
            "request_user_input",
            "apply_patch",
            "web_search",
            "view_image",
            "spawn_agent",
            "send_input",
            "resume_agent",
            "wait_agent",
            "close_agent",
        ],
    )
    .await;
}

#[tokio::test]
async fn test_build_specs_default_shell_present() {
    let config = test_config().await;
    let model_info = construct_model_info_offline("o3", &config);
    let mut features = Features::with_defaults();
    features.enable(Feature::UnifiedExec);
    let available_models = Vec::new();
    let tools_config = ToolsConfig::new(&ToolsConfigParams {
        model_info: &model_info,
        available_models: &available_models,
        features: &features,
        image_generation_tool_auth_allowed: true,
        web_search_mode: Some(WebSearchMode::Live),
        session_source: SessionSource::Cli,
        sandbox_policy: &SandboxPolicy::DangerFullAccess,
        windows_sandbox_level: WindowsSandboxLevel::Disabled,
    });
    let (tools, _) = build_specs(
        &tools_config,
        Some(HashMap::new()),
        /*deferred_mcp_tools*/ None,
        &[],
    )
    .build();

    // Only check the shell variant and a couple of core tools.
    let mut subset = vec!["exec_command", "write_stdin", "update_plan"];
    if let Some(shell_tool) = shell_tool_name(&tools_config) {
        subset.push(shell_tool);
    }
    assert_contains_tool_names(&tools, &subset);
}

#[tokio::test]
async fn shell_zsh_fork_prefers_shell_command_over_unified_exec() {
    let config = test_config().await;
    let model_info = construct_model_info_offline("o3", &config);
    let mut features = Features::with_defaults();
    features.enable(Feature::UnifiedExec);
    features.enable(Feature::ShellZshFork);

    let available_models = Vec::new();
    let tools_config = ToolsConfig::new(&ToolsConfigParams {
        model_info: &model_info,
        available_models: &available_models,
        features: &features,
        image_generation_tool_auth_allowed: true,
        web_search_mode: Some(WebSearchMode::Live),
        session_source: SessionSource::Cli,
        sandbox_policy: &SandboxPolicy::DangerFullAccess,
        windows_sandbox_level: WindowsSandboxLevel::Disabled,
    });
    let user_shell = Shell {
        shell_type: ShellType::Zsh,
        shell_path: PathBuf::from("/bin/zsh"),
        shell_snapshot: crate::shell::empty_shell_snapshot_receiver(),
    };

    assert_eq!(tools_config.shell_type, ConfigShellToolType::ShellCommand);
    assert_eq!(
        tools_config.shell_command_backend,
        ShellCommandBackendConfig::ZshFork
    );
    assert_eq!(
        tools_config.unified_exec_shell_mode,
        UnifiedExecShellMode::Direct
    );
    assert_eq!(
        tools_config
            .with_unified_exec_shell_mode_for_session(
                &user_shell,
                Some(&PathBuf::from(if cfg!(windows) {
                    r"C:\opt\codex\zsh"
                } else {
                    "/opt/codex/zsh"
                })),
                Some(&PathBuf::from(if cfg!(windows) {
                    r"C:\opt\codex\codex-execve-wrapper"
                } else {
                    "/opt/codex/codex-execve-wrapper"
                })),
            )
            .unified_exec_shell_mode,
        if cfg!(unix) {
            UnifiedExecShellMode::ZshFork(ZshForkConfig {
                shell_zsh_path: AbsolutePathBuf::from_absolute_path("/opt/codex/zsh").unwrap(),
                main_execve_wrapper_exe: AbsolutePathBuf::from_absolute_path(
                    "/opt/codex/codex-execve-wrapper",
                )
                .unwrap(),
            })
        } else {
            UnifiedExecShellMode::Direct
        }
    );
}

#[test]
#[ignore]
fn test_parallel_support_flags() {
    let config = test_config();
    let model_info = ModelsManager::construct_model_info_offline_for_tests("gpt-5-codex", &config);
    let mut features = Features::with_defaults();
    features.enable(Feature::UnifiedExec);
    let available_models = Vec::new();
    let tools_config = ToolsConfig::new(&ToolsConfigParams {
        model_info: &model_info,
        available_models: &available_models,
        features: &features,
        web_search_mode: Some(WebSearchMode::Cached),
        session_source: SessionSource::Cli,
        sandbox_policy: &SandboxPolicy::DangerFullAccess,
        windows_sandbox_level: WindowsSandboxLevel::Disabled,
    });
    let (tools, _) = build_specs(
        &tools_config,
        /*mcp_tools*/ None,
        /*app_tools*/ None,
        &[],
    )
    .build();

    assert!(find_tool(&tools, "exec_command").supports_parallel_tool_calls);
    assert!(!find_tool(&tools, "write_stdin").supports_parallel_tool_calls);
}

#[test]
fn test_test_model_info_includes_sync_tool() {
    let _config = test_config();
    let mut model_info = model_info_from_models_json("gpt-5-codex");
    model_info.experimental_supported_tools = vec!["test_sync_tool".to_string()];
    let features = Features::with_defaults();
    let available_models = Vec::new();
    let tools_config = ToolsConfig::new(&ToolsConfigParams {
        model_info: &model_info,
        available_models: &available_models,
        features: &features,
        web_search_mode: Some(WebSearchMode::Cached),
        session_source: SessionSource::Cli,
        sandbox_policy: &SandboxPolicy::DangerFullAccess,
        windows_sandbox_level: WindowsSandboxLevel::Disabled,
    });
    let (tools, _) = build_specs(
        &tools_config,
        /*mcp_tools*/ None,
        /*app_tools*/ None,
        &[],
    )
    .build();

    assert!(tools.iter().any(|tool| tool.name() == "test_sync_tool"));
}

#[test]
fn test_build_specs_mcp_tools_converted() {
    let config = test_config();
    let model_info = ModelsManager::construct_model_info_offline_for_tests("o3", &config);
    let mut features = Features::with_defaults();
    features.enable(Feature::UnifiedExec);
    let available_models = Vec::new();
    let tools_config = ToolsConfig::new(&ToolsConfigParams {
        model_info: &model_info,
        available_models: &available_models,
        features: &features,
        web_search_mode: Some(WebSearchMode::Live),
        session_source: SessionSource::Cli,
        sandbox_policy: &SandboxPolicy::DangerFullAccess,
        windows_sandbox_level: WindowsSandboxLevel::Disabled,
    });
    let (tools, _) = build_specs(
        &tools_config,
        Some(HashMap::from([(
            "test_server/do_something_cool".to_string(),
            mcp_tool(
                "do_something_cool",
                "Do something cool",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "string_argument": { "type": "string" },
                        "number_argument": { "type": "number" },
                        "object_argument": {
                            "type": "object",
                            "properties": {
                                "string_property": { "type": "string" },
                                "number_property": { "type": "number" },
                            },
                            "required": ["string_property", "number_property"],
                            "additionalProperties": false,
                        },
                    },
                }),
            ),
        )])),
        /*app_tools*/ None,
        &[],
    )
    .build();

    let tool = find_tool(&tools, "test_server/do_something_cool");
    assert_eq!(
        &tool.spec,
        &ToolSpec::Function(ResponsesApiTool {
            name: "test_server/do_something_cool".to_string(),
            parameters: JsonSchema::Object {
                properties: BTreeMap::from([
                    (
                        "string_argument".to_string(),
                        JsonSchema::String { description: None }
                    ),
                    (
                        "number_argument".to_string(),
                        JsonSchema::Number { description: None }
                    ),
                    (
                        "object_argument".to_string(),
                        JsonSchema::Object {
                            properties: BTreeMap::from([
                                (
                                    "string_property".to_string(),
                                    JsonSchema::String { description: None }
                                ),
                                (
                                    "number_property".to_string(),
                                    JsonSchema::Number { description: None }
                                ),
                            ]),
                            required: Some(vec![
                                "string_property".to_string(),
                                "number_property".to_string(),
                            ]),
                            additional_properties: Some(false.into()),
                        },
                    ),
                ]),
                required: None,
                additional_properties: None,
            },
            description: "Do something cool".to_string(),
            strict: false,
            output_schema: Some(mcp_call_tool_result_output_schema(serde_json::json!({}))),
            defer_loading: None,
        })
    );
}

#[test]
fn test_build_specs_mcp_tools_sorted_by_name() {
    let config = test_config();
    let model_info = ModelsManager::construct_model_info_offline_for_tests("o3", &config);
    let mut features = Features::with_defaults();
    features.enable(Feature::UnifiedExec);
    let available_models = Vec::new();
    let tools_config = ToolsConfig::new(&ToolsConfigParams {
        model_info: &model_info,
        available_models: &available_models,
        features: &features,
        web_search_mode: Some(WebSearchMode::Cached),
        session_source: SessionSource::Cli,
        sandbox_policy: &SandboxPolicy::DangerFullAccess,
        windows_sandbox_level: WindowsSandboxLevel::Disabled,
    });

    // Intentionally construct a map with keys that would sort alphabetically.
    let tools_map: HashMap<String, rmcp::model::Tool> = HashMap::from([
        (
            "test_server/do".to_string(),
            mcp_tool("a", "a", serde_json::json!({"type": "object"})),
        ),
        (
            "test_server/something".to_string(),
            mcp_tool("b", "b", serde_json::json!({"type": "object"})),
        ),
        (
            "test_server/cool".to_string(),
            mcp_tool("c", "c", serde_json::json!({"type": "object"})),
        ),
    ]);

    let (tools, _) = build_specs(&tools_config, Some(tools_map), /*app_tools*/ None, &[]).build();

    // Only assert that the MCP tools themselves are sorted by fully-qualified name.
    let mcp_names: Vec<_> = tools
        .iter()
        .map(|t| t.name().to_string())
        .filter(|n| n.starts_with("test_server/"))
        .collect();
    let expected = vec![
        "test_server/cool".to_string(),
        "test_server/do".to_string(),
        "test_server/something".to_string(),
    ];
    assert_eq!(mcp_names, expected);
}

#[test]
fn search_tool_description_lists_each_codex_apps_connector_once() {
    let model_info = search_capable_model_info();
    let mut features = Features::with_defaults();
    features.enable(Feature::Apps);
    features.enable(Feature::ToolSearch);
    let available_models = Vec::new();
    let tools_config = ToolsConfig::new(&ToolsConfigParams {
        model_info: &model_info,
        available_models: &available_models,
        features: &features,
        web_search_mode: Some(WebSearchMode::Cached),
        session_source: SessionSource::Cli,
        sandbox_policy: &SandboxPolicy::DangerFullAccess,
        windows_sandbox_level: WindowsSandboxLevel::Disabled,
    });

    let (tools, _) = build_specs(
        &tools_config,
        Some(HashMap::from([
            (
                "mcp__codex_apps__calendar_create_event".to_string(),
                mcp_tool(
                    "calendar_create_event",
                    "Create calendar event",
                    serde_json::json!({"type": "object"}),
                ),
            ),
            (
                "mcp__rmcp__echo".to_string(),
                mcp_tool("echo", "Echo", serde_json::json!({"type": "object"})),
            ),
        ])),
        Some(HashMap::from([
            (
                "mcp__codex_apps__calendar_create_event".to_string(),
                ToolInfo {
                    server_name: codex_mcp::mcp::CODEX_APPS_MCP_SERVER_NAME.to_string(),
                    tool_name: "_create_event".to_string(),
                    tool_namespace: "mcp__codex_apps__calendar".to_string(),
                    tool: mcp_tool(
                        "calendar-create-event",
                        "Create calendar event",
                        serde_json::json!({"type": "object"}),
                    ),
                    connector_id: Some("calendar".to_string()),
                    connector_name: Some("Calendar".to_string()),
                    plugin_display_names: Vec::new(),
                    connector_description: Some(
                        "Plan events and manage your calendar.".to_string(),
                    ),
                },
            ),
            (
                "mcp__codex_apps__calendar_list_events".to_string(),
                ToolInfo {
                    server_name: codex_mcp::mcp::CODEX_APPS_MCP_SERVER_NAME.to_string(),
                    tool_name: "_list_events".to_string(),
                    tool_namespace: "mcp__codex_apps__calendar".to_string(),
                    tool: mcp_tool(
                        "calendar-list-events",
                        "List calendar events",
                        serde_json::json!({"type": "object"}),
                    ),
                    connector_id: Some("calendar".to_string()),
                    connector_name: Some("Calendar".to_string()),
                    plugin_display_names: Vec::new(),
                    connector_description: Some(
                        "Plan events and manage your calendar.".to_string(),
                    ),
                },
            ),
            (
                "mcp__codex_apps__gmail_search_threads".to_string(),
                ToolInfo {
                    server_name: codex_mcp::mcp::CODEX_APPS_MCP_SERVER_NAME.to_string(),
                    tool_name: "_search_threads".to_string(),
                    tool_namespace: "mcp__codex_apps__gmail".to_string(),
                    tool: mcp_tool(
                        "gmail-search-threads",
                        "Search email threads",
                        serde_json::json!({"type": "object"}),
                    ),
                    connector_id: Some("gmail".to_string()),
                    connector_name: Some("Gmail".to_string()),
                    plugin_display_names: Vec::new(),
                    connector_description: Some("Find and summarize email threads.".to_string()),
                },
            ),
            (
                "mcp__rmcp__echo".to_string(),
                ToolInfo {
                    server_name: "rmcp".to_string(),
                    tool_name: "echo".to_string(),
                    tool_namespace: "rmcp".to_string(),
                    tool: mcp_tool("echo", "Echo", serde_json::json!({"type": "object"})),
                    connector_id: None,
                    connector_name: None,
                    plugin_display_names: Vec::new(),
                    connector_description: None,
                },
            ),
        ])),
        &[],
    )
    .build();

    let search_tool = find_tool(&tools, TOOL_SEARCH_TOOL_NAME);
    let ToolSpec::ToolSearch { description, .. } = &search_tool.spec else {
        panic!("expected tool_search tool");
    };
    let description = description.as_str();
    assert!(description.contains("- Calendar: Plan events and manage your calendar."));
    assert!(description.contains("- Gmail: Find and summarize email threads."));
    assert_eq!(
        description
            .matches("- Calendar: Plan events and manage your calendar.")
            .count(),
        1
    );
    assert!(!description.contains("mcp__rmcp__echo"));
}

#[test]
fn search_tool_requires_model_capability_and_feature_flag() {
    let model_info = search_capable_model_info();
    let app_tools = Some(HashMap::from([(
        "mcp__codex_apps__calendar_create_event".to_string(),
        ToolInfo {
            server_name: codex_mcp::mcp::CODEX_APPS_MCP_SERVER_NAME.to_string(),
            tool_name: "calendar_create_event".to_string(),
            tool_namespace: "mcp__codex_apps__calendar".to_string(),
            tool: mcp_tool(
                "calendar_create_event",
                "Create calendar event",
                serde_json::json!({"type": "object"}),
            ),
            connector_id: Some("calendar".to_string()),
            connector_name: Some("Calendar".to_string()),
            connector_description: None,
            plugin_display_names: Vec::new(),
        },
    )]));

    let features = Features::with_defaults();
    let available_models = Vec::new();
    let tools_config = ToolsConfig::new(&ToolsConfigParams {
        model_info: &ModelInfo {
            supports_search_tool: false,
            ..model_info.clone()
        },
        available_models: &available_models,
        features: &features,
        web_search_mode: Some(WebSearchMode::Cached),
        session_source: SessionSource::Cli,
        sandbox_policy: &SandboxPolicy::DangerFullAccess,
        windows_sandbox_level: WindowsSandboxLevel::Disabled,
    });
    let (tools, _) = build_specs(
        &tools_config,
        /*mcp_tools*/ None,
        app_tools.clone(),
        &[],
    )
    .build();
    assert_lacks_tool_name(&tools, TOOL_SEARCH_TOOL_NAME);

    let available_models = Vec::new();
    let tools_config = ToolsConfig::new(&ToolsConfigParams {
        model_info: &model_info,
        available_models: &available_models,
        features: &features,
        web_search_mode: Some(WebSearchMode::Cached),
        session_source: SessionSource::Cli,
        sandbox_policy: &SandboxPolicy::DangerFullAccess,
        windows_sandbox_level: WindowsSandboxLevel::Disabled,
    });
    let (tools, _) = build_specs(
        &tools_config,
        /*mcp_tools*/ None,
        app_tools.clone(),
        &[],
    )
    .build();
    assert_lacks_tool_name(&tools, TOOL_SEARCH_TOOL_NAME);

    let mut features = Features::with_defaults();
    features.enable(Feature::ToolSearch);
    let available_models = Vec::new();
    let tools_config = ToolsConfig::new(&ToolsConfigParams {
        model_info: &model_info,
        available_models: &available_models,
        features: &features,
        web_search_mode: Some(WebSearchMode::Cached),
        session_source: SessionSource::Cli,
        sandbox_policy: &SandboxPolicy::DangerFullAccess,
        windows_sandbox_level: WindowsSandboxLevel::Disabled,
    });
    let (tools, _) = build_specs(&tools_config, /*mcp_tools*/ None, app_tools, &[]).build();
    assert_contains_tool_names(&tools, &[TOOL_SEARCH_TOOL_NAME]);
}

#[test]
fn tool_suggest_is_not_registered_without_feature_flag() {
    let model_info = search_capable_model_info();
    let mut features = Features::with_defaults();
    features.enable(Feature::ToolSearch);
    features.enable(Feature::Apps);
    features.enable(Feature::Plugins);
    features.disable(Feature::ToolSuggest);
    let available_models = Vec::new();
    let tools_config = ToolsConfig::new(&ToolsConfigParams {
        model_info: &model_info,
        available_models: &available_models,
        features: &features,
        web_search_mode: Some(WebSearchMode::Cached),
        session_source: SessionSource::Cli,
        sandbox_policy: &SandboxPolicy::DangerFullAccess,
        windows_sandbox_level: WindowsSandboxLevel::Disabled,
    });
    let (tools, _) = build_specs_with_discoverable_tools(
        &tools_config,
        /*mcp_tools*/ None,
        /*app_tools*/ None,
        Some(vec![discoverable_connector(
            "connector_2128aebfecb84f64a069897515042a44",
            "Google Calendar",
            "Plan events and schedules.",
        )]),
        &[],
    )
    .build();

    assert!(
        !tools
            .iter()
            .any(|tool| tool.name() == TOOL_SUGGEST_TOOL_NAME)
    );
}

#[test]
fn tool_suggest_can_be_registered_without_search_tool() {
    let model_info = ModelInfo {
        supports_search_tool: false,
        ..search_capable_model_info()
    };
    let mut features = Features::with_defaults();
    features.enable(Feature::Apps);
    features.enable(Feature::Plugins);
    features.enable(Feature::ToolSuggest);
    let available_models = Vec::new();
    let tools_config = ToolsConfig::new(&ToolsConfigParams {
        model_info: &model_info,
        available_models: &available_models,
        features: &features,
        web_search_mode: Some(WebSearchMode::Cached),
        session_source: SessionSource::Cli,
        sandbox_policy: &SandboxPolicy::DangerFullAccess,
        windows_sandbox_level: WindowsSandboxLevel::Disabled,
    });
    let (tools, _) = build_specs_with_discoverable_tools(
        &tools_config,
        /*mcp_tools*/ None,
        /*app_tools*/ None,
        Some(vec![discoverable_connector(
            "connector_2128aebfecb84f64a069897515042a44",
            "Google Calendar",
            "Plan events and schedules.",
        )]),
        &[],
    )
    .build();

    assert_contains_tool_names(&tools, &[TOOL_SUGGEST_TOOL_NAME]);
    assert_lacks_tool_name(&tools, TOOL_SEARCH_TOOL_NAME);

    let tool_suggest = find_tool(&tools, TOOL_SUGGEST_TOOL_NAME);
    let ToolSpec::Function(ResponsesApiTool { description, .. }) = &tool_suggest.spec else {
        panic!("expected function tool");
    };
    assert!(description.contains(
        "Suggests a missing connector in an installed plugin, or in narrower cases a not installed but discoverable plugin"
    ));
    assert!(description.contains(
        "You've already tried to find a matching available tool for the user's request but couldn't find a good match. This includes `tool_search` (if available) and other means."
    ));
}

#[tokio::test]
async fn tool_suggest_requires_apps_and_plugins_features() {
    let model_info = search_capable_model_info().await;
    let discoverable_tools = Some(vec![discoverable_connector(
        "connector_2128aebfecb84f64a069897515042a44",
        "Google Calendar",
        "Plan events and schedules.",
    )]);
    let available_models = Vec::new();

    for disabled_feature in [Feature::Apps, Feature::Plugins] {
        let mut features = Features::with_defaults();
        features.enable(Feature::ToolSearch);
        features.enable(Feature::ToolSuggest);
        features.enable(Feature::Apps);
        features.enable(Feature::Plugins);
        features.disable(disabled_feature);

        let tools_config = ToolsConfig::new(&ToolsConfigParams {
            model_info: &model_info,
            available_models: &available_models,
            features: &features,
            image_generation_tool_auth_allowed: true,
            web_search_mode: Some(WebSearchMode::Cached),
            session_source: SessionSource::Cli,
            sandbox_policy: &SandboxPolicy::DangerFullAccess,
            windows_sandbox_level: WindowsSandboxLevel::Disabled,
        });
        let (tools, _) = build_specs_with_discoverable_tools(
            &tools_config,
            /*mcp_tools*/ None,
            /*deferred_mcp_tools*/ None,
            Vec::new(),
            discoverable_tools.clone(),
            &[],
        )
        .build();

        assert!(
            !tools
                .iter()
                .any(|tool| tool.name() == TOOL_SUGGEST_TOOL_NAME),
            "tool_suggest should be absent when {disabled_feature:?} is disabled"
        );
    }
}

#[tokio::test]
async fn search_tool_description_handles_no_enabled_mcp_tools() {
    let model_info = search_capable_model_info().await;
    let mut features = Features::with_defaults();
    features.enable(Feature::Apps);
    features.enable(Feature::ToolSearch);
    let available_models = Vec::new();
    let tools_config = ToolsConfig::new(&ToolsConfigParams {
        model_info: &model_info,
        available_models: &available_models,
        features: &features,
        image_generation_tool_auth_allowed: true,
        web_search_mode: Some(WebSearchMode::Cached),
        session_source: SessionSource::Cli,
        sandbox_policy: &SandboxPolicy::DangerFullAccess,
        windows_sandbox_level: WindowsSandboxLevel::Disabled,
    });

    let (tools, _) = build_specs(
        &tools_config,
        /*mcp_tools*/ None,
        Some(HashMap::new()),
        &[],
    )
    .build();
    let search_tool = find_tool(&tools, TOOL_SEARCH_TOOL_NAME);
    let ToolSpec::ToolSearch { description, .. } = &search_tool.spec else {
        panic!("expected tool_search tool");
    };

    assert!(description.contains("None currently enabled."));
    assert!(!description.contains("{{source_descriptions}}"));
}

#[tokio::test]
async fn search_tool_description_falls_back_to_connector_name_without_description() {
    let model_info = search_capable_model_info().await;
    let mut features = Features::with_defaults();
    features.enable(Feature::Apps);
    features.enable(Feature::ToolSearch);
    let available_models = Vec::new();
    let tools_config = ToolsConfig::new(&ToolsConfigParams {
        model_info: &model_info,
        available_models: &available_models,
        features: &features,
        image_generation_tool_auth_allowed: true,
        web_search_mode: Some(WebSearchMode::Cached),
        session_source: SessionSource::Cli,
        sandbox_policy: &SandboxPolicy::DangerFullAccess,
        windows_sandbox_level: WindowsSandboxLevel::Disabled,
    });

    let (tools, _) = build_specs(
        &tools_config,
        /*mcp_tools*/ None,
        Some(HashMap::from([(
            "mcp__codex_apps__calendar_create_event".to_string(),
            ToolInfo {
                server_name: codex_mcp::mcp::CODEX_APPS_MCP_SERVER_NAME.to_string(),
                tool_name: "_create_event".to_string(),
                tool_namespace: "mcp__codex_apps__calendar".to_string(),
                tool: mcp_tool(
                    "calendar_create_event",
                    "Create calendar event",
                    serde_json::json!({"type": "object"}),
                ),
                connector_id: Some("calendar".to_string()),
                connector_name: Some("Calendar".to_string()),
                plugin_display_names: Vec::new(),
                connector_description: None,
            },
        )])),
        &[],
    )
    .build();
    let search_tool = find_tool(&tools, TOOL_SEARCH_TOOL_NAME);
    let ToolSpec::ToolSearch { description, .. } = &search_tool.spec else {
        panic!("expected tool_search tool");
    };

    assert!(description.contains("- Calendar"));
    assert!(!description.contains("- Calendar:"));
}

#[tokio::test]
async fn search_tool_registers_namespaced_mcp_tool_aliases() {
    let model_info = search_capable_model_info().await;
    let mut features = Features::with_defaults();
    features.enable(Feature::Apps);
    features.enable(Feature::ToolSearch);
    let available_models = Vec::new();
    let tools_config = ToolsConfig::new(&ToolsConfigParams {
        model_info: &model_info,
        available_models: &available_models,
        features: &features,
        image_generation_tool_auth_allowed: true,
        web_search_mode: Some(WebSearchMode::Cached),
        session_source: SessionSource::Cli,
        sandbox_policy: &SandboxPolicy::DangerFullAccess,
        windows_sandbox_level: WindowsSandboxLevel::Disabled,
    });

    let (_, registry) = build_specs(
        &tools_config,
        /*mcp_tools*/ None,
        Some(HashMap::from([
            (
                "mcp__codex_apps__calendar_create_event".to_string(),
                ToolInfo {
                    server_name: codex_mcp::mcp::CODEX_APPS_MCP_SERVER_NAME.to_string(),
                    tool_name: "_create_event".to_string(),
                    tool_namespace: "mcp__codex_apps__calendar".to_string(),
                    tool: mcp_tool(
                        "calendar-create-event",
                        "Create calendar event",
                        serde_json::json!({"type": "object"}),
                    ),
                    connector_id: Some("calendar".to_string()),
                    connector_name: Some("Calendar".to_string()),
                    connector_description: None,
                    plugin_display_names: Vec::new(),
                },
            ),
            (
                "mcp__codex_apps__calendar_list_events".to_string(),
                ToolInfo {
                    server_name: codex_mcp::mcp::CODEX_APPS_MCP_SERVER_NAME.to_string(),
                    tool_name: "_list_events".to_string(),
                    tool_namespace: "mcp__codex_apps__calendar".to_string(),
                    tool: mcp_tool(
                        "calendar-list-events",
                        "List calendar events",
                        serde_json::json!({"type": "object"}),
                    ),
                    connector_id: Some("calendar".to_string()),
                    connector_name: Some("Calendar".to_string()),
                    connector_description: None,
                    plugin_display_names: Vec::new(),
                },
            ),
            (
                "mcp__rmcp__echo".to_string(),
                ToolInfo {
                    server_name: "rmcp".to_string(),
                    callable_name: "echo".to_string(),
                    callable_namespace: "mcp__rmcp__".to_string(),
                    server_instructions: None,
                    tool: mcp_tool("echo", "Echo", serde_json::json!({"type": "object"})),
                    connector_id: None,
                    connector_name: None,
                    connector_description: None,
                    plugin_display_names: Vec::new(),
                },
            ),
        ])),
        &[],
    )
    .build();

    let app_alias = ToolName::namespaced("mcp__codex_apps__calendar", "_create_event");
    let mcp_alias = ToolName::namespaced("mcp__rmcp__", "echo");

    assert!(registry.has_handler(&ToolName::plain(TOOL_SEARCH_TOOL_NAME)));
    assert!(registry.has_handler(&app_alias));
    assert!(registry.has_handler(&mcp_alias));
}

#[test]
fn tool_suggest_description_lists_discoverable_tools() {
    let model_info = search_capable_model_info();
    let mut features = Features::with_defaults();
    features.enable(Feature::Apps);
    features.enable(Feature::Plugins);
    features.enable(Feature::ToolSearch);
    features.enable(Feature::ToolSuggest);
    let available_models = Vec::new();
    let tools_config = ToolsConfig::new(&ToolsConfigParams {
        model_info: &model_info,
        available_models: &available_models,
        features: &features,
        web_search_mode: Some(WebSearchMode::Cached),
        session_source: SessionSource::Cli,
        sandbox_policy: &SandboxPolicy::DangerFullAccess,
        windows_sandbox_level: WindowsSandboxLevel::Disabled,
    });

    let discoverable_tools = vec![
        discoverable_connector(
            "connector_2128aebfecb84f64a069897515042a44",
            "Google Calendar",
            "Plan events and schedules.",
        ),
        discoverable_connector(
            "connector_68df038e0ba48191908c8434991bbac2",
            "Gmail",
            "Find and summarize email threads.",
        ),
        DiscoverableTool::Plugin(Box::new(DiscoverablePluginInfo {
            id: "sample@test".to_string(),
            name: "Sample Plugin".to_string(),
            description: None,
            has_skills: true,
            mcp_server_names: vec!["sample-docs".to_string()],
            app_connector_ids: vec!["connector_sample".to_string()],
        })),
    ];

    let (tools, _) = build_specs_with_discoverable_tools(
        &tools_config,
        /*mcp_tools*/ None,
        /*app_tools*/ None,
        Some(discoverable_tools),
        &[],
    )
    .build();

    let tool_suggest = find_tool(&tools, TOOL_SUGGEST_TOOL_NAME);
    let ToolSpec::Function(ResponsesApiTool {
        description,
        parameters,
        ..
    }) = &tool_suggest.spec
    else {
        panic!("expected function tool");
    };
    assert!(description.contains(
        "Suggests a missing connector in an installed plugin, or in narrower cases a not installed but discoverable plugin"
    ));
    assert!(description.contains("Google Calendar"));
    assert!(description.contains("Gmail"));
    assert!(description.contains("Sample Plugin"));
    assert!(description.contains("Plan events and schedules."));
    assert!(description.contains("Find and summarize email threads."));
    assert!(description.contains("id: `sample@test`, type: plugin, action: install"));
    assert!(description.contains("`action_type`: `install` or `enable`"));
    assert!(
        description.contains("skills; MCP servers: sample-docs; app connectors: connector_sample")
    );
    assert!(
        description.contains(
            "You've already tried to find a matching available tool for the user's request but couldn't find a good match. This includes `tool_search` (if available) and other means."
        )
    );
    assert!(description.contains(
        "For connectors/apps that are not installed but needed for an installed plugin, suggest to install them if the task requirements match precisely."
    ));
    assert!(description.contains(
        "For plugins that are not installed but discoverable, only suggest discoverable and installable plugins when the user's intent very explicitly and unambiguously matches that plugin itself."
    ));
    assert!(description.contains(
        "Do not suggest a plugin just because one of its connectors or capabilities seems relevant."
    ));
    assert!(description.contains(
        "Apply the stricter explicit-and-unambiguous rule for *discoverable tools* like plugin install suggestions; *missing tools* like connector install suggestions continue to use the normal clear-fit standard."
    ));
    assert!(description.contains("DO NOT explore or recommend tools that are not on this list."));
    assert!(!description.contains("{{discoverable_tools}}"));
    assert!(!description.contains("tool_search fails to find a good match"));
    let JsonSchema::Object { required, .. } = parameters else {
        panic!("expected object parameters");
    };
    assert_eq!(
        required.as_ref(),
        Some(&vec![
            "tool_type".to_string(),
            "action_type".to_string(),
            "tool_id".to_string(),
            "suggest_reason".to_string(),
        ])
    );
}

#[test]
fn test_mcp_tool_property_missing_type_defaults_to_string() {
    let config = test_config();
    let model_info = construct_model_info_offline("gpt-5-codex", &config);
    let mut features = Features::with_defaults();
    features.enable(Feature::UnifiedExec);
    let available_models = Vec::new();
    let tools_config = ToolsConfig::new(&ToolsConfigParams {
        model_info: &model_info,
        available_models: &available_models,
        features: &features,
        image_generation_tool_auth_allowed: true,
        web_search_mode: Some(WebSearchMode::Cached),
        session_source: SessionSource::Cli,
        sandbox_policy: &SandboxPolicy::DangerFullAccess,
        windows_sandbox_level: WindowsSandboxLevel::Disabled,
    });

    let (tools, _) = build_specs(
        &tools_config,
        Some(HashMap::from([(
            "dash/search".to_string(),
            mcp_tool_info_with_display_name(
                "dash/search",
                mcp_tool(
                    "search",
                    "Search docs",
                    serde_json::json!({
                        "type": "object",
                        "properties": {
                            "query": {"description": "search query"}
                        }
                    }),
                ),
            ),
        )])),
        /*deferred_mcp_tools*/ None,
        &[],
    )
    .build();

    let tool = find_namespace_function_tool(&tools, "dash/", "search");
    assert_eq!(
        *tool,
        ResponsesApiTool {
            name: "search".to_string(),
            parameters: JsonSchema::object(
                /*properties*/
                BTreeMap::from([(
                    "query".to_string(),
                    JsonSchema::string(Some("search query".to_string())),
                )]),
                /*required*/ None,
                /*additional_properties*/ None
            ),
            description: "Search docs".to_string(),
            strict: false,
            output_schema: Some(mcp_call_tool_result_output_schema(serde_json::json!({}))),
            defer_loading: None,
        }
    );
}

#[tokio::test]
async fn test_mcp_tool_preserves_integer_schema() {
    let config = test_config().await;
    let model_info = construct_model_info_offline("gpt-5-codex", &config);
    let mut features = Features::with_defaults();
    features.enable(Feature::UnifiedExec);
    let available_models = Vec::new();
    let tools_config = ToolsConfig::new(&ToolsConfigParams {
        model_info: &model_info,
        available_models: &available_models,
        features: &features,
        image_generation_tool_auth_allowed: true,
        web_search_mode: Some(WebSearchMode::Cached),
        session_source: SessionSource::Cli,
        sandbox_policy: &SandboxPolicy::DangerFullAccess,
        windows_sandbox_level: WindowsSandboxLevel::Disabled,
    });

    let (tools, _) = build_specs(
        &tools_config,
        Some(HashMap::from([(
            "dash/paginate".to_string(),
            mcp_tool_info_with_display_name(
                "dash/paginate",
                mcp_tool(
                    "paginate",
                    "Pagination",
                    serde_json::json!({
                        "type": "object",
                        "properties": {"page": {"type": "integer"}}
                    }),
                ),
            ),
        )])),
        /*deferred_mcp_tools*/ None,
        &[],
    )
    .build();

    let tool = find_namespace_function_tool(&tools, "dash/", "paginate");
    assert_eq!(
        *tool,
        ResponsesApiTool {
            name: "paginate".to_string(),
            parameters: JsonSchema::object(
                /*properties*/
                BTreeMap::from([(
                    "page".to_string(),
                    JsonSchema::integer(/*description*/ None),
                )]),
                /*required*/ None,
                /*additional_properties*/ None
            ),
            description: "Pagination".to_string(),
            strict: false,
            output_schema: Some(mcp_call_tool_result_output_schema(serde_json::json!({}))),
            defer_loading: None,
        }
    );
}

#[tokio::test]
async fn test_mcp_tool_array_without_items_gets_default_string_items() {
    let config = test_config().await;
    let model_info = construct_model_info_offline("gpt-5-codex", &config);
    let mut features = Features::with_defaults();
    features.enable(Feature::UnifiedExec);
    features.enable(Feature::ApplyPatchFreeform);
    let available_models = Vec::new();
    let tools_config = ToolsConfig::new(&ToolsConfigParams {
        model_info: &model_info,
        available_models: &available_models,
        features: &features,
        image_generation_tool_auth_allowed: true,
        web_search_mode: Some(WebSearchMode::Cached),
        session_source: SessionSource::Cli,
        sandbox_policy: &SandboxPolicy::DangerFullAccess,
        windows_sandbox_level: WindowsSandboxLevel::Disabled,
    });

    let (tools, _) = build_specs(
        &tools_config,
        Some(HashMap::from([(
            "dash/tags".to_string(),
            mcp_tool_info_with_display_name(
                "dash/tags",
                mcp_tool(
                    "tags",
                    "Tags",
                    serde_json::json!({
                        "type": "object",
                        "properties": {"tags": {"type": "array"}}
                    }),
                ),
            ),
        )])),
        /*deferred_mcp_tools*/ None,
        &[],
    )
    .build();

    let tool = find_namespace_function_tool(&tools, "dash/", "tags");
    assert_eq!(
        *tool,
        ResponsesApiTool {
            name: "tags".to_string(),
            parameters: JsonSchema::object(
                /*properties*/
                BTreeMap::from([(
                    "tags".to_string(),
                    JsonSchema::array(
                        JsonSchema::string(/*description*/ None),
                        /*description*/ None,
                    ),
                )]),
                /*required*/ None,
                /*additional_properties*/ None
            ),
            description: "Tags".to_string(),
            strict: false,
            output_schema: Some(mcp_call_tool_result_output_schema(serde_json::json!({}))),
            defer_loading: None,
        }
    );
}

#[tokio::test]
async fn test_mcp_tool_anyof_defaults_to_string() {
    let config = test_config().await;
    let model_info = construct_model_info_offline("gpt-5-codex", &config);
    let mut features = Features::with_defaults();
    features.enable(Feature::UnifiedExec);
    let available_models = Vec::new();
    let tools_config = ToolsConfig::new(&ToolsConfigParams {
        model_info: &model_info,
        available_models: &available_models,
        features: &features,
        image_generation_tool_auth_allowed: true,
        web_search_mode: Some(WebSearchMode::Cached),
        session_source: SessionSource::Cli,
        sandbox_policy: &SandboxPolicy::DangerFullAccess,
        windows_sandbox_level: WindowsSandboxLevel::Disabled,
    });

    let (tools, _) = build_specs(
        &tools_config,
        Some(HashMap::from([(
            "dash/value".to_string(),
            mcp_tool_info_with_display_name(
                "dash/value",
                mcp_tool(
                    "value",
                    "AnyOf Value",
                    serde_json::json!({
                        "type": "object",
                        "properties": {
                            "value": {"anyOf": [{"type": "string"}, {"type": "number"}]}
                        }
                    }),
                ),
            ),
        )])),
        /*deferred_mcp_tools*/ None,
        &[],
    )
    .build();

    let tool = find_namespace_function_tool(&tools, "dash/", "value");
    assert_eq!(
        *tool,
        ResponsesApiTool {
            name: "value".to_string(),
            parameters: JsonSchema::object(
                /*properties*/
                BTreeMap::from([(
                    "value".to_string(),
                    JsonSchema::any_of(
                        vec![
                            JsonSchema::string(/*description*/ None),
                            JsonSchema::number(/*description*/ None),
                        ],
                        /*description*/ None,
                    ),
                )]),
                /*required*/ None,
                /*additional_properties*/ None
            ),
            description: "AnyOf Value".to_string(),
            strict: false,
            output_schema: Some(mcp_call_tool_result_output_schema(serde_json::json!({}))),
            defer_loading: None,
        }
    );
}

#[tokio::test]
async fn test_get_openai_tools_mcp_tools_with_additional_properties_schema() {
    let config = test_config().await;
    let model_info = construct_model_info_offline("gpt-5-codex", &config);
    let mut features = Features::with_defaults();
    features.enable(Feature::UnifiedExec);
    let available_models = Vec::new();
    let tools_config = ToolsConfig::new(&ToolsConfigParams {
        model_info: &model_info,
        available_models: &available_models,
        features: &features,
        image_generation_tool_auth_allowed: true,
        web_search_mode: Some(WebSearchMode::Cached),
        session_source: SessionSource::Cli,
        sandbox_policy: &SandboxPolicy::DangerFullAccess,
        windows_sandbox_level: WindowsSandboxLevel::Disabled,
    });
    let (tools, _) = build_specs(
        &tools_config,
        Some(HashMap::from([(
            "test_server/do_something_cool".to_string(),
            mcp_tool_info_with_display_name(
                "test_server/do_something_cool",
                mcp_tool(
                    "do_something_cool",
                    "Do something cool",
                    serde_json::json!({
                        "type": "object",
                        "properties": {
                        "string_argument": {"type": "string"},
                        "number_argument": {"type": "number"},
                        "object_argument": {
                            "type": "object",
                            "properties": {
                                "string_property": {"type": "string"},
                                "number_property": {"type": "number"}
                            },
                            "required": ["string_property", "number_property"],
                            "additionalProperties": {
                                "type": "object",
                                "properties": {
                                    "addtl_prop": {"type": "string"}
                                },
                                "required": ["addtl_prop"],
                                "additionalProperties": false
                                }
                            }
                        }
                    }),
                ),
            ),
        )])),
        /*deferred_mcp_tools*/ None,
        &[],
    )
    .build();

    let tool = find_namespace_function_tool(&tools, "test_server/", "do_something_cool");
    assert_eq!(
        *tool,
        ResponsesApiTool {
            name: "do_something_cool".to_string(),
            parameters: JsonSchema::object(
                /*properties*/
                BTreeMap::from([
                    (
                        "string_argument".to_string(),
                        JsonSchema::string(/*description*/ None),
                    ),
                    (
                        "number_argument".to_string(),
                        JsonSchema::number(/*description*/ None),
                    ),
                    (
                        "object_argument".to_string(),
                        JsonSchema::object(
                            BTreeMap::from([
                                (
                                    "string_property".to_string(),
                                    JsonSchema::string(/*description*/ None),
                                ),
                                (
                                    "number_property".to_string(),
                                    JsonSchema::number(/*description*/ None),
                                ),
                            ]),
                            Some(vec![
                                "string_property".to_string(),
                                "number_property".to_string(),
                            ]),
                            Some(
                                JsonSchema::object(
                                    BTreeMap::from([(
                                        "addtl_prop".to_string(),
                                        JsonSchema::string(/*description*/ None),
                                    )]),
                                    Some(vec!["addtl_prop".to_string()]),
                                    Some(false.into()),
                                )
                                .into(),
                            ),
                        ),
                    ),
                ]),
                /*required*/ None,
                /*additional_properties*/ None
            ),
            description: "Do something cool".to_string(),
            strict: false,
            output_schema: Some(mcp_call_tool_result_output_schema(serde_json::json!({}))),
            defer_loading: None,
        }
    );
}

#[test]
fn code_mode_augments_builtin_tool_descriptions_with_typed_sample() {
    let config = test_config();
    let model_info = ModelsManager::construct_model_info_offline_for_tests("gpt-5-codex", &config);
    let mut features = Features::with_defaults();
    features.enable(Feature::CodeMode);
    features.enable(Feature::UnifiedExec);
    let available_models = Vec::new();
    let tools_config = ToolsConfig::new(&ToolsConfigParams {
        model_info: &model_info,
        available_models: &available_models,
        features: &features,
        web_search_mode: Some(WebSearchMode::Cached),
        session_source: SessionSource::Cli,
        sandbox_policy: &SandboxPolicy::DangerFullAccess,
        windows_sandbox_level: WindowsSandboxLevel::Disabled,
    });

    let (tools, _) = build_specs(
        &tools_config,
        /*mcp_tools*/ None,
        /*app_tools*/ None,
        &[],
    )
    .build();
    let ToolSpec::Function(ResponsesApiTool { description, .. }) =
        &find_tool(&tools, "view_image").spec
    else {
        panic!("expected function tool");
    };

    assert_eq!(
        description,
        "View a local image from the filesystem (only use if given a full filepath by the user, and the image isn't already attached to the thread context within <image ...> tags).\n\nexec tool declaration:\n```ts\ndeclare const tools: { view_image(args: { path: string; }): Promise<{ detail: string | null; image_url: string; }>; };\n```"
    );
}

#[test]
fn code_mode_augments_mcp_tool_descriptions_with_namespaced_sample() {
    let config = test_config();
    let model_info = ModelsManager::construct_model_info_offline_for_tests("gpt-5-codex", &config);
    let mut features = Features::with_defaults();
    features.enable(Feature::CodeMode);
    features.enable(Feature::UnifiedExec);
    let available_models = Vec::new();
    let tools_config = ToolsConfig::new(&ToolsConfigParams {
        model_info: &model_info,
        available_models: &available_models,
        features: &features,
        web_search_mode: Some(WebSearchMode::Cached),
        session_source: SessionSource::Cli,
        sandbox_policy: &SandboxPolicy::DangerFullAccess,
        windows_sandbox_level: WindowsSandboxLevel::Disabled,
    });

    let (tools, _) = build_specs(
        &tools_config,
        Some(HashMap::from([(
            "mcp__sample__echo".to_string(),
            mcp_tool(
                "echo",
                "Echo text",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "message": {"type": "string"}
                    },
                    "required": ["message"],
                    "additionalProperties": false
                }),
            ),
        )])),
        /*app_tools*/ None,
        &[],
    )
    .build();

    let ToolSpec::Function(ResponsesApiTool { description, .. }) =
        &find_tool(&tools, "mcp__sample__echo").spec
    else {
        panic!("expected function tool");
    };

    assert_eq!(
        description,
        "Echo text\n\nexec tool declaration:\n```ts\ndeclare const tools: { mcp__sample__echo(args: { message: string; }): Promise<{ _meta?: unknown; content: Array<unknown>; isError?: boolean; structuredContent?: unknown; }>; };\n```"
    );
}

#[test]
fn code_mode_only_restricts_model_tools_to_exec_tools() {
    let mut features = Features::with_defaults();
    features.enable(Feature::CodeMode);
    features.enable(Feature::CodeModeOnly);

    assert_model_tools(
        "gpt-5.1-codex",
        &features,
        Some(WebSearchMode::Live),
        &["exec", "wait"],
    )
    .await;
}

#[test]
fn code_mode_only_exec_description_includes_full_nested_tool_details() {
    let config = test_config();
    let model_info = ModelsManager::construct_model_info_offline_for_tests("gpt-5-codex", &config);
    let mut features = Features::with_defaults();
    features.enable(Feature::CodeMode);
    features.enable(Feature::CodeModeOnly);
    let available_models = Vec::new();
    let tools_config = ToolsConfig::new(&ToolsConfigParams {
        model_info: &model_info,
        available_models: &available_models,
        features: &features,
        web_search_mode: Some(WebSearchMode::Cached),
        session_source: SessionSource::Cli,
        sandbox_policy: &SandboxPolicy::DangerFullAccess,
        windows_sandbox_level: WindowsSandboxLevel::Disabled,
    });

    let (tools, _) = build_specs(
        &tools_config,
        /*mcp_tools*/ None,
        /*app_tools*/ None,
        &[],
    )
    .build();
    let ToolSpec::Freeform(FreeformTool { description, .. }) = &find_tool(&tools, "exec").spec
    else {
        panic!("expected freeform tool");
    };

    assert!(!description.contains("Enabled nested tools:"));
    assert!(!description.contains("Nested tool reference:"));
    assert!(description.starts_with(
        "Use `exec/wait` tool to run all other tools, do not attempt to use any other tools directly"
    ));
    assert!(description.contains("### `update_plan` (`update_plan`)"));
    assert!(description.contains("### `view_image` (`view_image`)"));
}

#[test]
fn code_mode_exec_description_omits_nested_tool_details_when_not_code_mode_only() {
    let config = test_config();
    let model_info = ModelsManager::construct_model_info_offline_for_tests("gpt-5-codex", &config);
    let mut features = Features::with_defaults();
    features.enable(Feature::CodeMode);
    let available_models = Vec::new();
    let tools_config = ToolsConfig::new(&ToolsConfigParams {
        model_info: &model_info,
        available_models: &available_models,
        features: &features,
        web_search_mode: Some(WebSearchMode::Cached),
        session_source: SessionSource::Cli,
        sandbox_policy: &SandboxPolicy::DangerFullAccess,
        windows_sandbox_level: WindowsSandboxLevel::Disabled,
    });

    let (tools, _) = build_specs(
        &tools_config,
        /*mcp_tools*/ None,
        /*app_tools*/ None,
        &[],
    )
    .build();
    let ToolSpec::Freeform(FreeformTool { description, .. }) = &find_tool(&tools, "exec").spec
    else {
        panic!("expected freeform tool");
    };

    assert!(!description.starts_with(
        "Use `exec/wait` tool to run all other tools, do not attempt to use any other tools directly"
    ));
    assert!(!description.contains("### `update_plan` (`update_plan`)"));
    assert!(!description.contains("### `view_image` (`view_image`)"));
}
#[test]
fn chat_tools_include_top_level_name() {
    let properties =
        BTreeMap::from([("foo".to_string(), JsonSchema::String { description: None })]);
    let tools = vec![ToolSpec::Function(ResponsesApiTool {
        name: "demo".to_string(),
        description: "A demo tool".to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::Object {
            properties,
            required: None,
            additional_properties: None,
        },
        output_schema: None,
    })];

    let responses_json = codex_tools::create_tools_json_for_responses_api(&tools).unwrap();
    assert_eq!(
        responses_json,
        vec![json!({
            "type": "function",
            "name": "demo",
            "description": "A demo tool",
            "strict": false,
            "parameters": {
                "type": "object",
                "properties": {
                    "foo": { "type": "string" }
                },
            },
        })]
    );
}

#[test]
fn llama_server_tool_serialization_converts_supported_tools_to_functions_only() {
    let tools = vec![
        ToolSpec::LocalShell {},
        create_apply_patch_freeform_tool(),
        create_js_repl_tool(),
        ToolSpec::WebSearch {
            external_web_access: Some(false),
            filters: None,
            user_location: None,
            search_context_size: None,
            search_content_types: None,
        },
    ];

    let responses_json = create_tools_json_for_responses_api_with_provider(
        &tools,
        crate::model_provider_info::LLAMA_SERVER_OSS_PROVIDER_ID,
    )
    .unwrap();

    let tool_names = responses_json
        .iter()
        .filter_map(|tool| tool.get("name").and_then(|name| name.as_str()))
        .collect::<Vec<_>>();

    assert_eq!(tool_names, vec!["local_shell", "apply_patch", "js_repl"]);
    assert!(
        responses_json
            .iter()
            .all(|tool| tool.get("type") == Some(&json!("function")))
    );
}
