#[macro_export]
macro_rules! register_knowledge_tools {
    ($builder:expr, $ilhae_dir:expr, $bt_settings:expr) => {{
        use $crate::{
            KbCompileInput, KbFileBackInput, KbIngestInput, KbLintInput, KbQueryInput,
            KbWorkspaceListInput,
        };
        use std::path::PathBuf;

        $builder
            .tool_fn(
                "kb_workspace_list",
                "List configured knowledge-base workspaces and their root paths. Use this first instead of shell find/ls when a task mentions a knowledge workspace.",
                {
                    let ilhae_dir = $ilhae_dir.clone();
                    let bts = $bt_settings.clone();
                    async move |_input: KbWorkspaceListInput, _cx| {
                        $crate::check_tool_enabled!(bts, "kb_workspace_list");
                        match $crate::admin_builtins::kb::load_registry(&ilhae_dir) {
                            Ok(registry) => {
                                let response = $crate::IlhaeAppKbWorkspaceListResponse {
                                    workspaces: registry
                                        .workspaces
                                        .iter()
                                        .map(|entry| $crate::admin_builtins::kb::workspace_to_dto(entry, registry.active_workspace.as_deref()))
                                        .collect(),
                                    active_workspace: registry.active_workspace,
                                };
                                Ok::<String, sacp::Error>(serde_json::to_string_pretty(&response).unwrap_or_else(|_| "{}".to_string()))
                            }
                            Err(err) => Err(sacp::Error::internal_error().data(err.to_string())),
                        }
                    }
                },
                sacp::tool_fn!(),
            )
            .tool_fn(
                "kb_ingest",
                "Scan raw/ in the active knowledge workspace and refresh raw_inventory.json. Prefer this over manual raw/ traversal when syncing sources.",
                {
                    let ilhae_dir = $ilhae_dir.clone();
                    let bts = $bt_settings.clone();
                    async move |input: KbIngestInput, _cx| {
                        $crate::check_tool_enabled!(bts, "kb_ingest");
                        match $crate::admin_builtins::kb::resolve_workspace(&ilhae_dir, input.workspace_id.as_deref()) {
                            Ok((registry, workspace)) => {
                                let root = PathBuf::from(&workspace.root_path);
                                $crate::admin_builtins::kb::ensure_workspace_dirs(&root)
                                    .map_err(|err| sacp::Error::internal_error().data(err.to_string()))?;
                                let sources = $crate::admin_builtins::kb::collect_sources(&root)
                                    .map_err(|err| sacp::Error::internal_error().data(err.to_string()))?;
                                let inventory_path = root.join("index").join("raw_inventory.json");
                                let inventory_body = serde_json::to_vec_pretty(&sources)
                                    .map_err(|err| sacp::Error::internal_error().data(err.to_string()))?;
                                std::fs::write(&inventory_path, inventory_body)
                                    .map_err(|err| sacp::Error::internal_error().data(err.to_string()))?;
                                let response = $crate::IlhaeAppKbIngestResponse {
                                    workspace: Some($crate::admin_builtins::kb::workspace_to_dto(&workspace, registry.active_workspace.as_deref())),
                                    sources,
                                    inventory_path: Some(
                                        inventory_path
                                            .strip_prefix(&root)
                                            .unwrap_or(inventory_path.as_path())
                                            .to_string_lossy()
                                            .to_string(),
                                    ),
                                };
                                Ok::<String, sacp::Error>(serde_json::to_string_pretty(&response).unwrap_or_else(|_| "{}".to_string()))
                            }
                            Err(err) => Err(sacp::Error::internal_error().data(err.to_string())),
                        }
                    }
                },
                sacp::tool_fn!(),
            )
            .tool_fn(
                "kb_compile",
                "Compile raw/ sources into wiki/sources, wiki/concepts, and index markdown files. Use after editing source knowledge content when generated indexes must be refreshed.",
                {
                    let ilhae_dir = $ilhae_dir.clone();
                    let bts = $bt_settings.clone();
                    async move |input: KbCompileInput, _cx| {
                        $crate::check_tool_enabled!(bts, "kb_compile");
                        match $crate::admin_builtins::kb::resolve_workspace(&ilhae_dir, input.workspace_id.as_deref()) {
                            Ok((registry, workspace)) => {
                                let root = PathBuf::from(&workspace.root_path);
                                $crate::admin_builtins::kb::ensure_workspace_dirs(&root)
                                    .map_err(|err| sacp::Error::internal_error().data(err.to_string()))?;
                                let (compiled_sources, concept_count, generated_files) =
                                    $crate::admin_builtins::kb::compile_workspace(&root)
                                        .map_err(|err| sacp::Error::internal_error().data(err.to_string()))?;
                                let response = $crate::IlhaeAppKbCompileResponse {
                                    workspace: Some($crate::admin_builtins::kb::workspace_to_dto(&workspace, registry.active_workspace.as_deref())),
                                    compiled_sources,
                                    concept_count,
                                    generated_files,
                                };
                                Ok::<String, sacp::Error>(serde_json::to_string_pretty(&response).unwrap_or_else(|_| "{}".to_string()))
                            }
                            Err(err) => Err(sacp::Error::internal_error().data(err.to_string())),
                        }
                    }
                },
                sacp::tool_fn!(),
            )
            .tool_fn(
                "kb_lint",
                "Run KB health checks: missing summaries, stale raw sources, and broken local links. Prefer this over recursive shell scans when diagnosing knowledge workspace issues.",
                {
                    let ilhae_dir = $ilhae_dir.clone();
                    let bts = $bt_settings.clone();
                    async move |input: KbLintInput, _cx| {
                        $crate::check_tool_enabled!(bts, "kb_lint");
                        match $crate::admin_builtins::kb::resolve_workspace(&ilhae_dir, input.workspace_id.as_deref()) {
                            Ok((registry, workspace)) => {
                                let root = PathBuf::from(&workspace.root_path);
                                let issues = $crate::admin_builtins::kb::lint_workspace(&root)
                                    .map_err(|err| sacp::Error::internal_error().data(err.to_string()))?;
                                let response = $crate::IlhaeAppKbLintResponse {
                                    workspace: Some($crate::admin_builtins::kb::workspace_to_dto(&workspace, registry.active_workspace.as_deref())),
                                    issues,
                                };
                                Ok::<String, sacp::Error>(serde_json::to_string_pretty(&response).unwrap_or_else(|_| "{}".to_string()))
                            }
                            Err(err) => Err(sacp::Error::internal_error().data(err.to_string())),
                        }
                    }
                },
                sacp::tool_fn!(),
            )
            .tool_fn(
                "kb_query",
                "Search wiki/index markdown in the active knowledge workspace and synthesize a markdown report. Use this for workspace-aware lookup instead of broad repository grep.",
                {
                    let ilhae_dir = $ilhae_dir.clone();
                    let bts = $bt_settings.clone();
                    async move |input: KbQueryInput, _cx| {
                        $crate::check_tool_enabled!(bts, "kb_query");
                        match $crate::admin_builtins::kb::resolve_workspace(&ilhae_dir, input.workspace_id.as_deref()) {
                            Ok((registry, workspace)) => {
                                let root = PathBuf::from(&workspace.root_path);
                                let (answer, matched_paths) = $crate::admin_builtins::kb::query_workspace(&root, &input.query)
                                    .map_err(|err| sacp::Error::internal_error().data(err.to_string()))?;
                                let report_path = match input.output_path.as_deref() {
                                    Some(relative) => {
                                        let path = $crate::admin_builtins::kb::resolve_relative_target(&root, "output", relative)
                                            .map_err(|err| sacp::Error::invalid_request().data(err.to_string()))?;
                                        $crate::admin_builtins::kb::write_markdown(&path, &answer)
                                            .map_err(|err| sacp::Error::internal_error().data(err.to_string()))?;
                                        Some(
                                            path.strip_prefix(&root)
                                                .unwrap_or(path.as_path())
                                                .to_string_lossy()
                                                .to_string(),
                                        )
                                    }
                                    None => None,
                                };
                                let response = $crate::IlhaeAppKbQueryResponse {
                                    workspace: Some($crate::admin_builtins::kb::workspace_to_dto(&workspace, registry.active_workspace.as_deref())),
                                    answer,
                                    matched_paths,
                                    report_path,
                                };
                                Ok::<String, sacp::Error>(serde_json::to_string_pretty(&response).unwrap_or_else(|_| "{}".to_string()))
                            }
                            Err(err) => Err(sacp::Error::internal_error().data(err.to_string())),
                        }
                    }
                },
                sacp::tool_fn!(),
            )
            .tool_fn(
                "kb_file_back",
                "Write markdown content back into wiki/, output/, or index/ within the active knowledge workspace. Prefer this over ad-hoc shell writes when fixing workspace markdown files.",
                {
                    let ilhae_dir = $ilhae_dir.clone();
                    let bts = $bt_settings.clone();
                    async move |input: KbFileBackInput, _cx| {
                        $crate::check_tool_enabled!(bts, "kb_file_back");
                        match $crate::admin_builtins::kb::resolve_workspace(&ilhae_dir, input.workspace_id.as_deref()) {
                            Ok((_registry, workspace)) => {
                                let root = PathBuf::from(&workspace.root_path);
                                let path = $crate::admin_builtins::kb::resolve_relative_target(&root, &input.target, &input.relative_path)
                                    .map_err(|err| sacp::Error::invalid_request().data(err.to_string()))?;
                                $crate::admin_builtins::kb::write_markdown(&path, &input.content)
                                    .map_err(|err| sacp::Error::internal_error().data(err.to_string()))?;
                                let response = $crate::IlhaeAppKbFileBackResponse {
                                    ok: true,
                                    path: Some(
                                        path.strip_prefix(&root)
                                            .unwrap_or(path.as_path())
                                            .to_string_lossy()
                                            .to_string(),
                                    ),
                                };
                                Ok::<String, sacp::Error>(serde_json::to_string_pretty(&response).unwrap_or_else(|_| "{}".to_string()))
                            }
                            Err(err) => Err(sacp::Error::internal_error().data(err.to_string())),
                        }
                    }
                },
                sacp::tool_fn!(),
            )
    }};
}
