use crate::shell::Shell;
use crate::shell::ShellType;
use crate::tools::code_mode::PUBLIC_TOOL_NAME;
use crate::tools::code_mode::WAIT_TOOL_NAME;
use crate::tools::handlers::agent_jobs::BatchJobHandler;
use crate::tools::handlers::multi_agents_common::DEFAULT_WAIT_TIMEOUT_MS;
use crate::tools::handlers::multi_agents_common::MAX_WAIT_TIMEOUT_MS;
use crate::tools::handlers::multi_agents_common::MIN_WAIT_TIMEOUT_MS;
use crate::tools::registry::ToolRegistryBuilder;
use codex_mcp::ToolInfo;
use codex_protocol::config_types::WebSearchMode;
use codex_protocol::dynamic_tools::DynamicToolSpec;
use codex_protocol::openai_models::ApplyPatchToolType;
use codex_protocol::openai_models::ConfigShellToolType;
use codex_protocol::openai_models::WebSearchToolType;
use codex_protocol::{CommandCategory, CommandMeta};
use codex_tools::CommandToolOptions;
use codex_tools::DiscoverableTool;
use codex_tools::ShellToolOptions;
use codex_tools::SpawnAgentToolOptions;
use codex_tools::ViewImageToolOptions;
use codex_tools::ToolHandlerKind;
use codex_tools::ToolName;
use codex_tools::ToolNamespace;
use codex_tools::ToolRegistryPlanDeferredTool;
use codex_tools::ToolRegistryPlanMcpTool;
use codex_tools::ToolRegistryPlanParams;
use codex_tools::ToolSearchSource;
use codex_tools::ToolsConfig;
use codex_tools::WaitAgentTimeoutOptions;
use codex_tools::augment_tool_spec_for_code_mode;
use codex_tools::collect_tool_search_source_infos;
use codex_tools::collect_tool_suggest_entries;
use codex_tools::create_apply_patch_freeform_tool;
use codex_tools::create_apply_patch_json_tool;
use codex_tools::create_followup_task_tool;
use codex_tools::create_close_agent_tool_v1;
use codex_tools::create_close_agent_tool_v2;
use codex_tools::create_code_mode_tool;
use codex_tools::create_exec_command_tool;
use codex_tools::create_js_repl_reset_tool;
use codex_tools::create_js_repl_tool;
use codex_tools::create_list_agents_tool;
use codex_tools::create_list_dir_tool;
use codex_tools::create_lsp_tool;
use codex_tools::create_list_mcp_resource_templates_tool;
use codex_tools::create_list_mcp_resources_tool;
use codex_tools::create_read_mcp_resource_tool;
use codex_tools::create_report_agent_job_result_tool;
use codex_tools::create_request_permissions_tool;
use codex_tools::create_request_user_input_tool;
use codex_tools::create_resume_agent_tool;
use codex_tools::create_send_input_tool_v1;
use codex_tools::create_send_message_tool;
use codex_tools::create_shell_command_tool;
use codex_tools::create_shell_tool;
use codex_tools::create_spawn_agent_tool_v1;
use codex_tools::create_spawn_agent_tool_v2;
use codex_tools::create_spawn_agents_on_csv_tool;
use codex_tools::create_test_sync_tool;
use codex_tools::create_tool_search_tool;
use codex_tools::create_tool_suggest_tool;
use codex_tools::create_update_plan_tool;
use codex_tools::create_view_image_tool;
use codex_tools::create_wait_agent_tool_v1;
use codex_tools::create_wait_agent_tool_v2;
use codex_tools::create_wait_tool;
use codex_tools::create_write_stdin_tool;
use codex_tools::dynamic_tool_to_responses_api_tool;
use codex_tools::mcp_tool_to_responses_api_tool;
use codex_tools::request_permissions_tool_description;
use codex_tools::request_user_input_tool_description;
use codex_tools::tool_spec_to_code_mode_tool_definition;
use codex_tools::ShellCommandBackendConfig;
use codex_tools::ToolSpec;
use codex_tools::ToolUserShellType;
use codex_tools::TOOL_SEARCH_DEFAULT_LIMIT;
use codex_tools::TOOL_SEARCH_TOOL_NAME;
use codex_tools::TOOL_SUGGEST_TOOL_NAME;
use serde::Deserialize;
use serde::Serialize;
use std::collections::BTreeMap;
use std::collections::HashMap;

mod helper_builders;

pub type JsonSchema = codex_tools::JsonSchema;
pub(crate) use codex_tools::ToolsConfigParams;
pub use helper_builders::create_tools_json_for_responses_api_with_provider;
use helper_builders::create_advisor_request_tool;
use helper_builders::create_self_check_tool;

#[cfg(test)]
pub(crate) use codex_tools::mcp_call_tool_result_output_schema;

const WEB_SEARCH_CONTENT_TYPES: [&str; 2] = ["text", "image"];

pub(crate) fn tool_user_shell_type(user_shell: &Shell) -> ToolUserShellType {
    match user_shell.shell_type {
        ShellType::Zsh => ToolUserShellType::Zsh,
        ShellType::Bash => ToolUserShellType::Bash,
        ShellType::PowerShell => ToolUserShellType::PowerShell,
        ShellType::Sh => ToolUserShellType::Sh,
        ShellType::Cmd => ToolUserShellType::Cmd,
    }
}

/// TODO(dylan): deprecate once we get rid of json tool
#[derive(Serialize, Deserialize)]
pub(crate) struct ApplyPatchToolArgs {
    pub(crate) input: String,
    /// The expected last modified time of the file being patched.
    /// If provided, the tool will fail if the file has been modified since this time.
    pub(crate) expected_mtime: Option<String>,
}

fn push_tool_spec(
    builder: &mut ToolRegistryBuilder,
    spec: ToolSpec,
    supports_parallel_tool_calls: bool,
    code_mode_enabled: bool,
) {
    let spec = if code_mode_enabled {
        augment_tool_spec_for_code_mode(spec)
    } else {
        spec
    };
    if supports_parallel_tool_calls {
        builder.push_spec_with_parallel_support(spec, /*supports_parallel_tool_calls*/ true);
    } else {
        builder.push_spec(spec);
    }
}

/// Builds the tool registry builder while collecting tool specs for later serialization.
#[cfg(test)]
pub(crate) fn build_specs(
    config: &ToolsConfig,
    mcp_tools: Option<HashMap<String, rmcp::model::Tool>>,
    app_tools: Option<HashMap<String, ToolInfo>>,
    dynamic_tools: &[DynamicToolSpec],
) -> ToolRegistryBuilder {
    build_specs_with_discoverable_tools(
        config,
        mcp_tools,
        app_tools,
        /*discoverable_tools*/ None,
        dynamic_tools,
    )
}

pub(crate) fn build_specs_with_discoverable_tools(
    config: &ToolsConfig,
    mcp_tools: Option<HashMap<String, ToolInfo>>,
    deferred_mcp_tools: Option<HashMap<String, ToolInfo>>,
    unavailable_called_tools: Vec<ToolName>,
    discoverable_tools: Option<Vec<DiscoverableTool>>,
    dynamic_tools: &[DynamicToolSpec],
) -> ToolRegistryBuilder {
    use crate::tools::handlers::ApplyPatchHandler;
    use crate::tools::handlers::CodeModeExecuteHandler;
    use crate::tools::handlers::CodeModeWaitHandler;
    use crate::tools::handlers::DynamicToolHandler;
    use crate::tools::handlers::JsReplHandler;
    use crate::tools::handlers::JsReplResetHandler;
    use crate::tools::handlers::ListDirHandler;
    use crate::tools::handlers::LspToolHandler;
    use crate::tools::handlers::McpHandler;
    use crate::tools::handlers::McpResourceHandler;
    use crate::tools::handlers::PlanHandler;
    use crate::tools::handlers::AdvisorRequestHandler;
    use crate::tools::handlers::RequestPermissionsHandler;
    use crate::tools::handlers::RequestUserInputHandler;
    use crate::tools::handlers::SelfCheckHandler;
    use crate::tools::handlers::ShellCommandHandler;
    use crate::tools::handlers::ShellHandler;
    use crate::tools::handlers::TestSyncHandler;
    use crate::tools::handlers::ToolSearchHandler;
    use crate::tools::handlers::ToolSuggestHandler;
    use crate::tools::handlers::UnavailableToolHandler;
    use crate::tools::handlers::UnifiedExecHandler;
    use crate::tools::handlers::ViewImageHandler;
    use crate::tools::handlers::WebSearchHandler;
    use crate::tools::handlers::multi_agents::CloseAgentHandler;
    use crate::tools::handlers::multi_agents::ResumeAgentHandler;
    use crate::tools::handlers::multi_agents::SendInputHandler;
    use crate::tools::handlers::multi_agents::SpawnAgentHandler;
    use crate::tools::handlers::multi_agents::WaitAgentHandler;
    use crate::tools::handlers::multi_agents_v2::FollowupTaskHandler as FollowupTaskHandlerV2;
    use crate::tools::handlers::multi_agents_v2::CloseAgentHandler as CloseAgentHandlerV2;
    use crate::tools::handlers::multi_agents_v2::ListAgentsHandler as ListAgentsHandlerV2;
    use crate::tools::handlers::multi_agents_v2::SendMessageHandler as SendMessageHandlerV2;
    use crate::tools::handlers::multi_agents_v2::SpawnAgentHandler as SpawnAgentHandlerV2;
    use crate::tools::handlers::multi_agents_v2::WaitAgentHandler as WaitAgentHandlerV2;
    use std::sync::Arc;

    let mut builder = ToolRegistryBuilder::new();

    let lsp_manager = crate::tools::handlers::lsp_manager::LspServerManager::new();

    let shell_handler = Arc::new(ShellHandler);
    let unified_exec_handler = Arc::new(UnifiedExecHandler);
    let plan_handler = Arc::new(PlanHandler);
    let apply_patch_handler = Arc::new(ApplyPatchHandler);
    let dynamic_tool_handler = Arc::new(DynamicToolHandler);
    let view_image_handler = Arc::new(ViewImageHandler);
    let mcp_handler = Arc::new(McpHandler);
    let mcp_resource_handler = Arc::new(McpResourceHandler);
    let shell_command_handler = Arc::new(ShellCommandHandler::from(
        match config.shell_command_backend {
            ShellCommandBackendConfig::Classic => codex_tools::ShellCommandBackendConfig::Classic,
            ShellCommandBackendConfig::ZshFork => codex_tools::ShellCommandBackendConfig::ZshFork,
        },
    ));
    let request_permissions_handler = Arc::new(RequestPermissionsHandler);
    let request_user_input_handler = Arc::new(RequestUserInputHandler {
        default_mode_request_user_input: config.default_mode_request_user_input,
    });
    let self_check_handler = Arc::new(SelfCheckHandler);
    let advisor_request_handler = Arc::new(AdvisorRequestHandler);
    let tool_suggest_handler = Arc::new(ToolSuggestHandler);
    let code_mode_handler = Arc::new(CodeModeExecuteHandler);
    let code_mode_wait_handler = Arc::new(CodeModeWaitHandler);
    let js_repl_handler = Arc::new(JsReplHandler);
    let js_repl_reset_handler = Arc::new(JsReplResetHandler);
    let lsp_handler = Arc::new(LspToolHandler::new(Arc::clone(&lsp_manager)));
    let exec_permission_approvals_enabled = config.exec_permission_approvals_enabled;

    if config.code_mode_enabled {
        let nested_config = config.for_code_mode_nested_tools();
        let (nested_specs, _) = build_specs_with_discoverable_tools(
            &nested_config,
            mcp_tools.clone(),
            deferred_mcp_tools.clone(),
            unavailable_called_tools.clone(),
            None,
            dynamic_tools,
        )
        .build();
        let mut enabled_tools = nested_specs
            .into_iter()
            .filter_map(|spec| tool_spec_to_code_mode_tool_definition(&spec.spec))
            .collect::<Vec<_>>();
        enabled_tools.sort_by(|left, right| left.name.cmp(&right.name));
        enabled_tools.dedup_by(|left, right| left.name == right.name);
        push_tool_spec(
            &mut builder,
            create_code_mode_tool(
                &enabled_tools,
                &BTreeMap::new(),
                config.code_mode_only_enabled,
                config.search_tool
                    && deferred_mcp_tools.as_ref().is_some_and(|tools| !tools.is_empty()),
            ),
            /*supports_parallel_tool_calls*/ false,
            config.code_mode_enabled,
        );
        builder.register_handler(
            PUBLIC_TOOL_NAME,
            code_mode_handler,
            CommandMeta {
                name: PUBLIC_TOOL_NAME.to_string(),
                help_text: "Enters Code Mode to execute a sequence of edits.".to_string(),
                usage_example: None,
                is_experimental: false,
                is_visible: true,
                available_during_task: false,
                category: CommandCategory::FileOps,
                tags: None,
                linked_files: None,
                version: None,
                compatibility: None,
            },
        );
        push_tool_spec(
            &mut builder,
            create_wait_tool(),
            /*supports_parallel_tool_calls*/ false,
            config.code_mode_enabled,
        );
        builder.register_handler(
            WAIT_TOOL_NAME,
            code_mode_wait_handler,
            CommandMeta {
                name: WAIT_TOOL_NAME.to_string(),
                help_text: "Wait for a background task to complete.".to_string(),
                usage_example: None,
                is_experimental: false,
                is_visible: true,
                available_during_task: false,
                category: CommandCategory::System,
                tags: None,
                linked_files: None,
                version: None,
                compatibility: None,
            },
        );
    }

    match &config.shell_type {
        ConfigShellToolType::Default => {
            push_tool_spec(
                &mut builder,
                create_shell_tool(ShellToolOptions {
                    exec_permission_approvals_enabled,
                }),
                /*supports_parallel_tool_calls*/ true,
                config.code_mode_enabled,
            );
        }
        ConfigShellToolType::Local => {
            push_tool_spec(
                &mut builder,
                ToolSpec::LocalShell {},
                /*supports_parallel_tool_calls*/ true,
                config.code_mode_enabled,
            );
        }
        ConfigShellToolType::UnifiedExec => {
            push_tool_spec(
                &mut builder,
                create_exec_command_tool(CommandToolOptions {
                    allow_login_shell: config.allow_login_shell,
                    exec_permission_approvals_enabled,
                }),
                /*supports_parallel_tool_calls*/ true,
                config.code_mode_enabled,
            );
            push_tool_spec(
                &mut builder,
                create_write_stdin_tool(),
                /*supports_parallel_tool_calls*/ false,
                config.code_mode_enabled,
            );
            builder.register_handler(
                "exec_command",
                unified_exec_handler.clone(),
                CommandMeta {
                    name: "exec_command".to_string(),
                    help_text: "Execute a command in the unified shell.".to_string(),
                    usage_example: Some("ls -la".to_string()),
                    is_experimental: false,
                    is_visible: true,
                    available_during_task: true,
                    category: CommandCategory::System,
                    tags: None,
                    linked_files: None,
                    version: None,
                    compatibility: None,
                },
            );
            builder.register_handler(
                "write_stdin",
                unified_exec_handler,
                CommandMeta {
                    name: "write_stdin".to_string(),
                    help_text: "Write input to a running command's stdin.".to_string(),
                    usage_example: None,
                    is_experimental: false,
                    is_visible: true,
                    available_during_task: true,
                    category: CommandCategory::System,
                    tags: None,
                    linked_files: None,
                    version: None,
                    compatibility: None,
                },
            );
        }
        ConfigShellToolType::Disabled => {
            // Do nothing.
        }
        ConfigShellToolType::ShellCommand => {
            push_tool_spec(
                &mut builder,
                create_shell_command_tool(CommandToolOptions {
                    allow_login_shell: config.allow_login_shell,
                    exec_permission_approvals_enabled,
                }),
                /*supports_parallel_tool_calls*/ true,
                config.code_mode_enabled,
            );
        }
    }

    if config.shell_type != ConfigShellToolType::Disabled {
        let shell_meta = CommandMeta {
            name: "shell".to_string(),
            help_text: "Execute a command in the local shell.".to_string(),
            usage_example: Some("ls".to_string()),
            is_experimental: false,
            is_visible: true,
            available_during_task: true,
            category: CommandCategory::System,
            tags: None,
            linked_files: None,
            version: None,
            compatibility: None,
        };
        // Always register shell aliases so older prompts remain compatible.
        builder.register_handler("shell", shell_handler.clone(), shell_meta.clone());
        builder.register_handler("container.exec", shell_handler.clone(), shell_meta.clone());
        builder.register_handler("local_shell", shell_handler, shell_meta);
        builder.register_handler(
            "shell_command",
            shell_command_handler,
            CommandMeta {
                name: "shell_command".to_string(),
                help_text: "Execute a shell command with specific options.".to_string(),
                usage_example: None,
                is_experimental: false,
                is_visible: true,
                available_during_task: true,
                category: CommandCategory::System,
                tags: None,
                linked_files: None,
                version: None,
                compatibility: None,
            },
        );
    }

    if mcp_tools.is_some() {
        push_tool_spec(
            &mut builder,
            create_list_mcp_resources_tool(),
            /*supports_parallel_tool_calls*/ true,
            config.code_mode_enabled,
        );
        push_tool_spec(
            &mut builder,
            create_list_mcp_resource_templates_tool(),
            /*supports_parallel_tool_calls*/ true,
            config.code_mode_enabled,
        );
        push_tool_spec(
            &mut builder,
            create_read_mcp_resource_tool(),
            /*supports_parallel_tool_calls*/ true,
            config.code_mode_enabled,
        );
        let mcp_res_meta = CommandMeta {
            name: "mcp_resource".to_string(),
            help_text: "Interact with MCP resources.".to_string(),
            usage_example: None,
            is_experimental: false,
            is_visible: true,
            available_during_task: true,
            category: CommandCategory::Mcp,
            tags: None,
            linked_files: None,
            version: None,
            compatibility: None,
        };
        builder.register_handler(
            "list_mcp_resources",
            mcp_resource_handler.clone(),
            mcp_res_meta.clone(),
        );
        builder.register_handler(
            "list_mcp_resource_templates",
            mcp_resource_handler.clone(),
            mcp_res_meta.clone(),
        );
        builder.register_handler("read_mcp_resource", mcp_resource_handler, mcp_res_meta);
    }

    push_tool_spec(
        &mut builder,
        create_update_plan_tool(),
        /*supports_parallel_tool_calls*/ false,
        config.code_mode_enabled,
    );
    builder.register_handler(
        "update_plan",
        plan_handler,
        CommandMeta {
            name: "update_plan".to_string(),
            help_text: "Update the shared execution plan.".to_string(),
            usage_example: None,
            is_experimental: false,
            is_visible: true,
            available_during_task: false,
            category: CommandCategory::System,
            tags: None,
            linked_files: None,
            version: None,
            compatibility: None,
        },
    );

    if config.js_repl_enabled {
        push_tool_spec(
            &mut builder,
            create_js_repl_tool(),
            /*supports_parallel_tool_calls*/ false,
            config.code_mode_enabled,
        );
        push_tool_spec(
            &mut builder,
            create_js_repl_reset_tool(),
            /*supports_parallel_tool_calls*/ false,
            config.code_mode_enabled,
        );
        builder.register_handler(
            "js_repl",
            js_repl_handler,
            CommandMeta {
                name: "js_repl".to_string(),
                help_text: "Execute JavaScript in a persistent Node.js REPL.".to_string(),
                usage_example: Some("1 + 1".to_string()),
                is_experimental: false,
                is_visible: true,
                available_during_task: false,
                category: CommandCategory::Experimental,
                tags: None,
                linked_files: None,
                version: None,
                compatibility: None,
            },
        );
        builder.register_handler(
            "js_repl_reset",
            js_repl_reset_handler,
            CommandMeta {
                name: "js_repl_reset".to_string(),
                help_text: "Reset the JavaScript REPL state.".to_string(),
                usage_example: None,
                is_experimental: false,
                is_visible: true,
                available_during_task: false,
                category: CommandCategory::Experimental,
                tags: None,
                linked_files: None,
                version: None,
                compatibility: None,
            },
        );
    }

    if config.default_mode_request_user_input {
        push_tool_spec(
            &mut builder,
            create_request_user_input_tool(request_user_input_tool_description(
                config.default_mode_request_user_input,
            )),
            /*supports_parallel_tool_calls*/ false,
            config.code_mode_enabled,
        );
        builder.register_handler(
            "request_user_input",
            request_user_input_handler,
            CommandMeta {
                name: "request_user_input".to_string(),
                help_text: "Ask the user for clarification or input.".to_string(),
                usage_example: None,
                is_experimental: false,
                is_visible: true,
                available_during_task: true,
                category: CommandCategory::System,
                tags: None,
                linked_files: None,
                version: None,
                compatibility: None,
            },
        );
    }

    if config.request_permissions_tool_enabled {
        push_tool_spec(
            &mut builder,
            create_request_permissions_tool(request_permissions_tool_description()),
            /*supports_parallel_tool_calls*/ false,
            config.code_mode_enabled,
        );
        builder.register_handler(
            "request_permissions",
            request_permissions_handler,
            CommandMeta {
                name: "request_permissions".to_string(),
                help_text: "Request broad permissions for a series of actions.".to_string(),
                usage_example: None,
                is_experimental: false,
                is_visible: true,
                available_during_task: true,
                category: CommandCategory::System,
                tags: None,
                linked_files: None,
                version: None,
                compatibility: None,
            },
        );
    }

    push_tool_spec(
        &mut builder,
        create_self_check_tool(),
        /*supports_parallel_tool_calls*/ false,
        config.code_mode_enabled,
    );
    builder.register_handler(
        "self_check",
        self_check_handler,
        CommandMeta {
            name: "self_check".to_string(),
            help_text: "Self-check execution state, allowing retries and autonomous recovery."
                .to_string(),
            usage_example: None,
            is_experimental: false,
            is_visible: true,
            available_during_task: true,
            category: CommandCategory::System,
            tags: None,
            linked_files: None,
            version: None,
            compatibility: None,
        },
    );

    if crate::tools::handlers::advisor_target_from_env().is_some() {
        push_tool_spec(
            &mut builder,
            create_advisor_request_tool(),
            /*supports_parallel_tool_calls*/ false,
            config.code_mode_enabled,
        );
        builder.register_handler(
            "advisor_request",
            advisor_request_handler,
            CommandMeta {
                name: "advisor_request".to_string(),
                help_text: "Request planning advice from the configured system2 advisor."
                    .to_string(),
                usage_example: None,
                is_experimental: false,
                is_visible: true,
                available_during_task: true,
                category: CommandCategory::System,
                tags: None,
                linked_files: None,
                version: None,
                compatibility: None,
            },
        );
    }

    if config.search_tool
        && let Some(deferred_mcp_tools) = deferred_mcp_tools
    {
        let search_tool_handler = Arc::new(ToolSearchHandler::new(deferred_mcp_tools.clone()));
        push_tool_spec(
            &mut builder,
            create_tool_search_tool(
                &collect_tool_search_source_infos(
                    deferred_mcp_tools.values().map(|tool| ToolSearchSource {
                        server_name: &tool.server_name,
                        connector_name: tool.connector_name.as_deref(),
                        connector_description: tool.connector_description.as_deref(),
                    }),
                ),
                TOOL_SEARCH_DEFAULT_LIMIT,
            ),
            /*supports_parallel_tool_calls*/ true,
            config.code_mode_enabled,
        );
        builder.register_handler(
            TOOL_SEARCH_TOOL_NAME,
            search_tool_handler,
            CommandMeta {
                name: TOOL_SEARCH_TOOL_NAME.to_string(),
                help_text: "Search for available tools and their capabilities.".to_string(),
                usage_example: None,
                is_experimental: false,
                is_visible: true,
                available_during_task: true,
                category: CommandCategory::System,
                tags: None,
                linked_files: None,
                version: None,
                compatibility: None,
            },
        );

        for tool in deferred_mcp_tools.values() {
            let alias_name =
                codex_tools::ToolName { name: tool.callable_name.clone(), namespace: Some(tool.callable_namespace.clone()) }.display();

            builder.register_handler(
                alias_name.clone(),
                mcp_handler.clone(),
                CommandMeta {
                    name: alias_name,
                    help_text: "MCP tool alias.".to_string(),
                    usage_example: None,
                    is_experimental: false,
                    is_visible: false,
                    available_during_task: true,
                    category: CommandCategory::Mcp,
                    tags: None,
                    linked_files: None,
                    version: None,
                    compatibility: None,
                },
            );
        }
    }

    if config.tool_suggest
        && let Some(discoverable_tools) = discoverable_tools
            .as_ref()
            .filter(|tools| !tools.is_empty())
    {
        builder.push_spec_with_parallel_support(
            create_tool_suggest_tool(&collect_tool_suggest_entries(discoverable_tools)),
            /*supports_parallel_tool_calls*/ true,
        );
        builder.register_handler(
            TOOL_SUGGEST_TOOL_NAME,
            tool_suggest_handler,
            CommandMeta {
                name: TOOL_SUGGEST_TOOL_NAME.to_string(),
                help_text: "Suggest relevant tools based on user intent.".to_string(),
                usage_example: None,
                is_experimental: false,
                is_visible: true,
                available_during_task: true,
                category: CommandCategory::System,
                tags: None,
                linked_files: None,
                version: None,
                compatibility: None,
            },
        );
    }

    if let Some(apply_patch_tool_type) = &config.apply_patch_tool_type {
        match apply_patch_tool_type {
            ApplyPatchToolType::Freeform => {
                push_tool_spec(
                    &mut builder,
                    create_apply_patch_freeform_tool(),
                    /*supports_parallel_tool_calls*/ false,
                    config.code_mode_enabled,
                );
            }
            ApplyPatchToolType::Function => {
                push_tool_spec(
                    &mut builder,
                    create_apply_patch_json_tool(),
                    /*supports_parallel_tool_calls*/ false,
                    config.code_mode_enabled,
                );
            }
        }
        builder.register_handler(
            "apply_patch",
            apply_patch_handler,
            CommandMeta {
                name: "apply_patch".to_string(),
                help_text: "Apply a patch to a file.".to_string(),
                usage_example: None,
                is_experimental: false,
                is_visible: true,
                available_during_task: true,
                category: CommandCategory::FileOps,
                tags: None,
                linked_files: None,
                version: None,
                compatibility: None,
            },
        );
    }

    if config
        .experimental_supported_tools
        .iter()
        .any(|tool| tool == "list_dir")
    {
        let list_dir_handler = Arc::new(ListDirHandler);
        push_tool_spec(
            &mut builder,
            create_list_dir_tool(),
            /*supports_parallel_tool_calls*/ true,
            config.code_mode_enabled,
        );
        builder.register_handler(
            "list_dir",
            list_dir_handler,
            CommandMeta {
                name: "list_dir".to_string(),
                help_text: "List contents of a directory.".to_string(),
                usage_example: Some("src/".to_string()),
                is_experimental: false,
                is_visible: true,
                available_during_task: true,
                category: CommandCategory::FileOps,
                tags: None,
                linked_files: None,
                version: None,
                compatibility: None,
            },
        );
    }

    if config
        .experimental_supported_tools
        .iter()
        .any(|tool| tool == "lsp")
    {
        use codex_tools::create_lsp_tool;
        push_tool_spec(
            &mut builder,
            create_lsp_tool(),
            /*supports_parallel_tool_calls*/ true,
            config.code_mode_enabled,
        );
        builder.register_handler(
            crate::tools::handlers::LSP_TOOL_NAME,
            lsp_handler,
            CommandMeta {
                name: crate::tools::handlers::LSP_TOOL_NAME.to_string(),
                help_text: "LSP language services.".to_string(),
                usage_example: None,
                is_experimental: false,
                is_visible: true,
                available_during_task: true,
                category: CommandCategory::FileOps,
                tags: None,
                linked_files: None,
                version: None,
                compatibility: None,
            },
        );
    }

    if config
        .experimental_supported_tools
        .contains(&"test_sync_tool".to_string())
    {
        let test_sync_handler = Arc::new(TestSyncHandler);
        push_tool_spec(
            &mut builder,
            create_test_sync_tool(),
            /*supports_parallel_tool_calls*/ true,
            config.code_mode_enabled,
        );
        builder.register_handler(
            "test_sync_tool",
            test_sync_handler,
            CommandMeta {
                name: "test_sync_tool".to_string(),
                help_text: "Sync test tool.".to_string(),
                usage_example: None,
                is_experimental: true,
                is_visible: false,
                available_during_task: true,
                category: CommandCategory::Experimental,
                tags: None,
                linked_files: None,
                version: None,
                compatibility: None,
            },
        );
    }

    let web_search_tool = codex_tools::create_web_search_tool(codex_tools::WebSearchToolOptions {
        web_search_mode: config.web_search_mode,
        web_search_config: config.web_search_config.as_ref(),
        web_search_tool_type: config.web_search_tool_type,
    });

    if let Some(tool) = web_search_tool {
        push_tool_spec(
            &mut builder,
            tool,
            /*supports_parallel_tool_calls*/ true,
            config.code_mode_enabled,
        );
        builder.register_handler(
            "web_search",
            std::sync::Arc::new(WebSearchHandler::new(config.web_search_config.clone())),
            CommandMeta {
                name: "web_search".to_string(),
                help_text: "Perform a web search using DuckDuckGo or SearXNG.".to_string(),
                usage_example: None,
                is_experimental: false,
                is_visible: true,
                available_during_task: true,
                category: CommandCategory::System,
                tags: None,
                linked_files: None,
                version: None,
                compatibility: None,
            },
        );
    }

    if config.image_gen_tool {
        push_tool_spec(
            &mut builder,
            ToolSpec::ImageGeneration {
                output_format: "png".to_string(),
            },
            /*supports_parallel_tool_calls*/ false,
            config.code_mode_enabled,
        );
    }

    push_tool_spec(
        &mut builder,
        create_view_image_tool(ViewImageToolOptions {
            can_request_original_image_detail: config.can_request_original_image_detail,
        }),
        /*supports_parallel_tool_calls*/ true,
        config.code_mode_enabled,
    );
    builder.register_handler(
        "view_image",
        view_image_handler,
        CommandMeta {
            name: "view_image".to_string(),
            help_text: "View visual content (images/videos).".to_string(),
            usage_example: None,
            is_experimental: false,
            is_visible: true,
            available_during_task: true,
            category: CommandCategory::System,
            tags: None,
            linked_files: None,
            version: None,
            compatibility: None,
        },
    );

    if config.collab_tools {
        if config.multi_agent_v2 {
            push_tool_spec(
                &mut builder,
                create_spawn_agent_tool_v2(SpawnAgentToolOptions {
                    available_models: &config.available_models,
                    agent_type_description: config.agent_type_description.clone(),
                    hide_agent_type_model_reasoning: config.hide_spawn_agent_metadata,
                    include_usage_hint: config.spawn_agent_usage_hint,
                    usage_hint_text: config.spawn_agent_usage_hint_text.clone(),
                }),
                /*supports_parallel_tool_calls*/ false,
                config.code_mode_enabled,
            );
            push_tool_spec(
                &mut builder,
                create_send_message_tool(),
                /*supports_parallel_tool_calls*/ false,
                config.code_mode_enabled,
            );
            push_tool_spec(
                &mut builder,
                create_followup_task_tool(),
                /*supports_parallel_tool_calls*/ false,
                config.code_mode_enabled,
            );
            push_tool_spec(
                &mut builder,
                create_wait_agent_tool_v2(WaitAgentTimeoutOptions {
                    default_timeout_ms: DEFAULT_WAIT_TIMEOUT_MS,
                    min_timeout_ms: MIN_WAIT_TIMEOUT_MS,
                    max_timeout_ms: MAX_WAIT_TIMEOUT_MS,
                }),
                /*supports_parallel_tool_calls*/ false,
                config.code_mode_enabled,
            );
            push_tool_spec(
                &mut builder,
                create_close_agent_tool_v2(),
                /*supports_parallel_tool_calls*/ false,
                config.code_mode_enabled,
            );
            push_tool_spec(
                &mut builder,
                create_list_agents_tool(),
                /*supports_parallel_tool_calls*/ false,
                config.code_mode_enabled,
            );
            builder.register_handler(
                "spawn_agent",
                Arc::new(SpawnAgentHandlerV2),
                CommandMeta {
                    name: "spawn_agent".to_string(),
                    help_text: "Spawn a new specialized agent.".to_string(),
                    usage_example: None,
                    is_experimental: false,
                    is_visible: true,
                    available_during_task: true,
                    category: CommandCategory::System,
                    tags: None,
                    linked_files: None,
                    version: None,
                    compatibility: None,
                },
            );
            builder.register_handler(
                "send_message",
                Arc::new(SendMessageHandlerV2),
                CommandMeta {
                    name: "send_message".to_string(),
                    help_text: "Send a message to an agent.".to_string(),
                    usage_example: None,
                    is_experimental: false,
                    is_visible: true,
                    available_during_task: true,
                    category: CommandCategory::System,
                    tags: None,
                    linked_files: None,
                    version: None,
                    compatibility: None,
                },
            );
            builder.register_handler(
                "assign_task",
                Arc::new(FollowupTaskHandlerV2),
                CommandMeta {
                    name: "assign_task".to_string(),
                    help_text: "Assign a task to an agent.".to_string(),
                    usage_example: None,
                    is_experimental: false,
                    is_visible: true,
                    available_during_task: true,
                    category: CommandCategory::System,
                    tags: None,
                    linked_files: None,
                    version: None,
                    compatibility: None,
                },
            );
            builder.register_handler(
                "wait_agent",
                Arc::new(WaitAgentHandlerV2),
                CommandMeta {
                    name: "wait_agent".to_string(),
                    help_text: "Wait for an agent to finish its task.".to_string(),
                    usage_example: None,
                    is_experimental: false,
                    is_visible: true,
                    available_during_task: true,
                    category: CommandCategory::System,
                    tags: None,
                    linked_files: None,
                    version: None,
                    compatibility: None,
                },
            );
            builder.register_handler(
                "close_agent",
                Arc::new(CloseAgentHandlerV2),
                CommandMeta {
                    name: "close_agent".to_string(),
                    help_text: "Close an agent session.".to_string(),
                    usage_example: None,
                    is_experimental: false,
                    is_visible: true,
                    available_during_task: true,
                    category: CommandCategory::System,
                    tags: None,
                    linked_files: None,
                    version: None,
                    compatibility: None,
                },
            );
            builder.register_handler(
                "list_agents",
                Arc::new(ListAgentsHandlerV2),
                CommandMeta {
                    name: "list_agents".to_string(),
                    help_text: "List all active agents.".to_string(),
                    usage_example: None,
                    is_experimental: false,
                    is_visible: true,
                    available_during_task: true,
                    category: CommandCategory::System,
                    tags: None,
                    linked_files: None,
                    version: None,
                    compatibility: None,
                },
            );
        } else {
            push_tool_spec(
                &mut builder,
                create_spawn_agent_tool_v1(SpawnAgentToolOptions {
                    available_models: &config.available_models,
                    agent_type_description: config.agent_type_description.clone(),
                    hide_agent_type_model_reasoning: config.hide_spawn_agent_metadata,
                    include_usage_hint: config.spawn_agent_usage_hint,
                    usage_hint_text: config.spawn_agent_usage_hint_text.clone(),
                }),
                /*supports_parallel_tool_calls*/ false,
                config.code_mode_enabled,
            );
            push_tool_spec(
                &mut builder,
                create_send_input_tool_v1(),
                /*supports_parallel_tool_calls*/ false,
                config.code_mode_enabled,
            );
            push_tool_spec(
                &mut builder,
                create_resume_agent_tool(),
                /*supports_parallel_tool_calls*/ false,
                config.code_mode_enabled,
            );
            builder.register_handler(
                "resume_agent",
                Arc::new(ResumeAgentHandler),
                CommandMeta {
                    name: "resume_agent".to_string(),
                    help_text: "Resume a paused agent.".to_string(),
                    usage_example: None,
                    is_experimental: false,
                    is_visible: true,
                    available_during_task: true,
                    category: CommandCategory::System,
                    tags: None,
                    linked_files: None,
                    version: None,
                    compatibility: None,
                },
            );
            push_tool_spec(
                &mut builder,
                create_wait_agent_tool_v1(WaitAgentTimeoutOptions {
                    default_timeout_ms: DEFAULT_WAIT_TIMEOUT_MS,
                    min_timeout_ms: MIN_WAIT_TIMEOUT_MS,
                    max_timeout_ms: MAX_WAIT_TIMEOUT_MS,
                }),
                /*supports_parallel_tool_calls*/ false,
                config.code_mode_enabled,
            );
            push_tool_spec(
                &mut builder,
                create_close_agent_tool_v1(),
                /*supports_parallel_tool_calls*/ false,
                config.code_mode_enabled,
            );
            builder.register_handler(
                "spawn_agent",
                Arc::new(SpawnAgentHandler),
                CommandMeta {
                    name: "spawn_agent".to_string(),
                    help_text: "Spawn a new agent (V1).".to_string(),
                    usage_example: None,
                    is_experimental: false,
                    is_visible: true,
                    available_during_task: true,
                    category: CommandCategory::System,
                    tags: None,
                    linked_files: None,
                    version: None,
                    compatibility: None,
                },
            );
            builder.register_handler(
                "send_input",
                Arc::new(SendInputHandler),
                CommandMeta {
                    name: "send_input".to_string(),
                    help_text: "Send input to an agent (V1).".to_string(),
                    usage_example: None,
                    is_experimental: false,
                    is_visible: true,
                    available_during_task: true,
                    category: CommandCategory::System,
                    tags: None,
                    linked_files: None,
                    version: None,
                    compatibility: None,
                },
            );
            builder.register_handler(
                "wait_agent",
                Arc::new(WaitAgentHandler),
                CommandMeta {
                    name: "wait_agent".to_string(),
                    help_text: "Wait for an agent (V1).".to_string(),
                    usage_example: None,
                    is_experimental: false,
                    is_visible: true,
                    available_during_task: true,
                    category: CommandCategory::System,
                    tags: None,
                    linked_files: None,
                    version: None,
                    compatibility: None,
                },
            );
            builder.register_handler(
                "close_agent",
                Arc::new(CloseAgentHandler),
                CommandMeta {
                    name: "close_agent".to_string(),
                    help_text: "Close an agent (V1).".to_string(),
                    usage_example: None,
                    is_experimental: false,
                    is_visible: true,
                    available_during_task: true,
                    category: CommandCategory::System,
                    tags: None,
                    linked_files: None,
                    version: None,
                    compatibility: None,
                },
            );
        }
    }

    if config.agent_jobs_tools {
        let agent_jobs_handler = Arc::new(BatchJobHandler);
        push_tool_spec(
            &mut builder,
            create_spawn_agents_on_csv_tool(),
            /*supports_parallel_tool_calls*/ false,
            config.code_mode_enabled,
        );
        builder.register_handler(
            "spawn_agents_on_csv",
            agent_jobs_handler.clone(),
            CommandMeta {
                name: "spawn_agents_on_csv".to_string(),
                help_text: "Batch spawn agents from a CSV file.".to_string(),
                usage_example: None,
                is_experimental: true,
                is_visible: true,
                available_during_task: false,
                category: CommandCategory::Experimental,
                tags: None,
                linked_files: None,
                version: None,
                compatibility: None,
            },
        );
        if config.agent_jobs_worker_tools {
            push_tool_spec(
                &mut builder,
                create_report_agent_job_result_tool(),
                /*supports_parallel_tool_calls*/ false,
                config.code_mode_enabled,
            );
            builder.register_handler(
                "report_agent_job_result",
                agent_jobs_handler,
                CommandMeta {
                    name: "report_agent_job_result".to_string(),
                    help_text: "Report the results of a batch job.".to_string(),
                    usage_example: None,
                    is_experimental: true,
                    is_visible: true,
                    available_during_task: false,
                    category: CommandCategory::Experimental,
                    tags: None,
                    linked_files: None,
                    version: None,
                    compatibility: None,
                },
            );
        }
    }

    if let Some(mcp_tools) = mcp_tools {
        let mut entries: Vec<(String, rmcp::model::Tool)> = mcp_tools.into_iter().map(|(k, v)| (k, v.tool)).collect();
        entries.sort_by(|a, b| a.0.cmp(&b.0));

        for (name, tool) in entries.into_iter() {
            match mcp_tool_to_responses_api_tool(&ToolName::from(name.as_str()), &tool) {                Ok(converted_tool) => {
                    push_tool_spec(
                        &mut builder,
                        ToolSpec::Function(converted_tool),
                        /*supports_parallel_tool_calls*/ false,
                        config.code_mode_enabled,
                    );
                    builder.register_handler(
                        name.clone(),
                        mcp_handler.clone(),
                        CommandMeta {
                            name,
                            help_text: tool
                                .description
                                .as_ref()
                                .map(|d| d.to_string())
                                .unwrap_or_default(),
                            usage_example: None,
                            is_experimental: false,
                            is_visible: true,
                            available_during_task: true,
                            category: CommandCategory::Mcp,
                            tags: None,
                            linked_files: None,
                            version: None,
                            compatibility: None,
                        },
                    );
                }
                Err(e) => {
                    tracing::error!("Failed to convert {name:?} MCP tool to OpenAI tool: {e:?}");
                }
            }
        }
    }

    if !dynamic_tools.is_empty() {
        for tool in dynamic_tools {
            match dynamic_tool_to_responses_api_tool(tool) {
                Ok(converted_tool) => {
                    push_tool_spec(
                        &mut builder,
                        ToolSpec::Function(converted_tool),
                        /*supports_parallel_tool_calls*/ false,
                        config.code_mode_enabled,
                    );
                    builder.register_handler(
                        tool.name.clone(),
                        dynamic_tool_handler.clone(),
                        CommandMeta {
                            name: tool.name.clone(),
                            help_text: tool.description.clone(),
                            usage_example: None,
                            is_experimental: false,
                            is_visible: true,
                            available_during_task: true,
                            category: CommandCategory::System,
                            tags: tool.tags.clone(),
                            linked_files: tool.linked_files.clone(),
                            version: tool.version.clone(),
                            compatibility: tool.compatibility.clone(),
                        },
                    );
                }
                Err(e) => {
                    tracing::error!(
                        "Failed to convert dynamic tool {:?} to OpenAI tool: {e:?}",
                        tool.name
                    );
                }
            }
        }
    }

    builder
}
#[cfg(test)]
#[path = "spec_tests.rs"]
mod tests;
