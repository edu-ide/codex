#[macro_export]
macro_rules! register_artifact_tools {
    ($builder:expr, $brain_service:expr, $bt_settings:expr, $active_sid:expr) => {{
        use $crate::{
            ArtifactSaveInput, ArtifactListInput, ArtifactGetInput, ArtifactEditInput,
        };

        $builder
            .tool_fn(
                "artifact_save",
                "Create or update a session artifact with automatic versioning. \
                 artifact_type: 'task' (task checklist), 'plan' (implementation plan), 'walkthrough' (completion summary), or 'other'. \
                 The system automatically determines the file path and version.",
                {
                    let brain = $brain_service.clone();
                    let bts = $bt_settings.clone();
                    let active_sid = $active_sid.clone();
                    async move |input: ArtifactSaveInput, _cx| {
                        $crate::check_tool_enabled!(bts, "artifact_save");

                        let sid = active_sid.read().await.clone();
                        if sid.is_empty() {
                            return Err(sacp::Error::internal_error()
                                .data("No active session. Cannot save artifact.".to_string()));
                        }

                        let summary = input.summary.as_deref().unwrap_or("");
                        match brain.artifact_save(&sid, &input.artifact_type, &input.content, summary, None) {
                            Ok((filename, ver)) => {
                                Ok::<String, sacp::Error>(format!(
                                    "✅ Artifact '{}' saved (v{}, {} chars)",
                                    filename, ver, input.content.len()
                                ))
                            }
                            Err(e) => Err(sacp::Error::internal_error()
                                .data(format!("Save error: {}", e))),
                        }
                    }
                },
                sacp::tool_fn!(),
            )
            .tool_fn(
                "artifact_list",
                "List all artifacts in the current session with version info.",
                {
                    let brain = $brain_service.clone();
                    let bts = $bt_settings.clone();
                    let active_sid = $active_sid.clone();
                    async move |_input: ArtifactListInput, _cx| {
                        $crate::check_tool_enabled!(bts, "artifact_list");

                        let sid = active_sid.read().await.clone();
                        if sid.is_empty() {
                            return Ok::<String, sacp::Error>("No active session.".to_string());
                        }

                        match brain.artifact_list(&sid) {
                            Ok(artifacts) if artifacts.is_empty() => {
                                Ok::<String, sacp::Error>("No artifacts in this session yet.".to_string())
                            }
                            Ok(artifacts) => {
                                let text = serde_json::to_string_pretty(&artifacts)
                                    .unwrap_or("[]".to_string());
                                Ok::<String, sacp::Error>(text)
                            }
                            Err(e) => Err(sacp::Error::internal_error()
                                .data(format!("List error: {}", e))),
                        }
                    }
                },
                sacp::tool_fn!(),
            )
            .tool_fn(
                "artifact_get",
                "Get the content of a specific artifact in the current session. \
                 Returns the latest version by default, or a specific version if specified.",
                {
                    let brain = $brain_service.clone();
                    let bts = $bt_settings.clone();
                    let active_sid = $active_sid.clone();
                    async move |input: ArtifactGetInput, _cx| {
                        $crate::check_tool_enabled!(bts, "artifact_get");

                        let sid = active_sid.read().await.clone();
                        if sid.is_empty() {
                            return Err(sacp::Error::internal_error()
                                .data("No active session.".to_string()));
                        }

                        match brain.artifact_get(&sid, &input.artifact_type, input.version) {
                            Ok(Some(artifact)) => {
                                Ok::<String, sacp::Error>(serde_json::to_string_pretty(&artifact).unwrap_or_default())
                            }
                            Ok(None) => {
                                let filename = brain_rs::BrainService::artifact_filename(&input.artifact_type);
                                Ok::<String, sacp::Error>(format!("No artifact '{}' found in this session.", filename))
                            }
                            Err(e) => Err(sacp::Error::internal_error().data(format!("Get error: {}", e))),
                        }
                    }
                },
                sacp::tool_fn!(),
            )
            .tool_fn(
                "artifact_edit",
                "Edit an existing artifact by providing the full updated content. \
                 Creates a new version automatically. Use this to update task checklists, \
                 refine plans, or add to walkthroughs.",
                {
                    let brain = $brain_service.clone();
                    let bts = $bt_settings.clone();
                    let active_sid = $active_sid.clone();
                    async move |input: ArtifactEditInput, _cx| {
                        $crate::check_tool_enabled!(bts, "artifact_edit");

                        let sid = active_sid.read().await.clone();
                        if sid.is_empty() {
                            return Err(sacp::Error::internal_error()
                                .data("No active session. Cannot edit artifact.".to_string()));
                        }

                        let summary = input.summary.as_deref().unwrap_or("edited");
                        match brain.artifact_save(&sid, &input.artifact_type, &input.content, summary, None) {
                            Ok((filename, ver)) => {
                                Ok::<String, sacp::Error>(format!(
                                    "✅ Artifact '{}' updated (v{}, {} chars)",
                                    filename, ver, input.content.len()
                                ))
                            }
                            Err(e) => Err(sacp::Error::internal_error()
                                .data(format!("Edit error: {}", e))),
                        }
                    }
                },
                sacp::tool_fn!(),
            )
    }};
}
