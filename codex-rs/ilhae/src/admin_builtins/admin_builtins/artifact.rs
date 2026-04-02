#[macro_export]
macro_rules! register_admin_artifact_handlers {
    ($builder:expr, $state:expr) => {{
        let s = $state.clone();
        $builder
            // ═══ Workflow Artifact List ═══
            .on_receive_request_from(
                sacp::Client,
                {
                    let ilhae_dir = s.infra.ilhae_dir.clone();
                    async move |req: ListWorkflowArtifactsRequest,
                                responder: Responder<ListWorkflowArtifactsResponse>,
                                _cx: ConnectionTo<Conductor>| {
                        info!(
                            "ilhae/list_workflow_artifacts RPC project_path={:?}",
                            req.project_path
                        );
                        let vault_dir = ilhae_dir.join("vault").join("workflow");
                        let mut artifacts = Vec::new();

                        if let Ok(entries) = std::fs::read_dir(&vault_dir) {
                            for entry in entries.flatten() {
                                let path = entry.path();
                                if path.is_file()
                                    && path.extension().map_or(false, |ext| ext == "md")
                                {
                                    if let Some(filename) =
                                        path.file_name().and_then(|n| n.to_str())
                                    {
                                        if !filename.starts_with("DESIGN_")
                                            && !filename.starts_with("PLAN_")
                                            && !filename.starts_with("VERIFICATION_")
                                            && !filename.starts_with("TEST_")
                                        {
                                            continue;
                                        }

                                        let mut artifact_type = "UNKNOWN".to_string();
                                        if filename.starts_with("DESIGN_") {
                                            artifact_type = "DESIGN".to_string();
                                        } else if filename.starts_with("PLAN_") {
                                            artifact_type = "PLAN".to_string();
                                        } else if filename.starts_with("VERIFICATION_") {
                                            artifact_type = "VERIFICATION".to_string();
                                        } else if filename.starts_with("TEST_") {
                                            artifact_type = "TEST".to_string();
                                        }

                                        let timestamp = entry
                                            .metadata()
                                            .and_then(|m| m.modified())
                                            .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
                                            .duration_since(std::time::SystemTime::UNIX_EPOCH)
                                            .unwrap_or_default()
                                            .as_millis()
                                            as i64;

                                        // Parse YAML frontmatter
                                        let content =
                                            std::fs::read_to_string(&path).unwrap_or_default();
                                        let mut doc_project_path = None;
                                        let mut doc_date = None;

                                        if content.starts_with("---\n") {
                                            if let Some(end_idx) = content[4..].find("\n---\n") {
                                                let frontmatter = &content[4..end_idx + 4];
                                                for line in frontmatter.lines() {
                                                    if let Some(rest) =
                                                        line.strip_prefix("project_path: ")
                                                    {
                                                        doc_project_path = Some(
                                                            rest.trim_matches('"')
                                                                .trim()
                                                                .to_string(),
                                                        );
                                                    } else if let Some(rest) =
                                                        line.strip_prefix("date: ")
                                                    {
                                                        doc_date = Some(
                                                            rest.trim_matches('"')
                                                                .trim()
                                                                .to_string(),
                                                        );
                                                    }
                                                }
                                            }
                                        }

                                        // Filter by project_path if requested
                                        if let Some(target_path) = &req.project_path {
                                            if doc_project_path.as_ref() != Some(target_path) {
                                                continue;
                                            }
                                        }

                                        artifacts.push(WorkflowArtifactDto {
                                            id: filename.to_string(),
                                            artifact_type,
                                            project_path: doc_project_path,
                                            date: doc_date,
                                            timestamp,
                                        });
                                    }
                                }
                            }
                        }
                        artifacts.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
                        responder.respond(ListWorkflowArtifactsResponse { artifacts })
                    }
                },
                sacp::on_receive_request!(),
            )
            // ═══ Workflow Artifact Read ═══
            .on_receive_request_from(
                sacp::Client,
                {
                    let ilhae_dir = s.infra.ilhae_dir.clone();
                    async move |req: ReadWorkflowArtifactRequest,
                                responder: Responder<ReadWorkflowArtifactResponse>,
                                _cx: ConnectionTo<Conductor>| {
                        info!("ilhae/read_workflow_artifact RPC id={:?}", req.id);
                        let vault_dir = ilhae_dir.join("vault").join("workflow");
                        let file_path = vault_dir.join(&req.id);

                        let content = std::fs::read_to_string(file_path).unwrap_or_default();
                        responder.respond(ReadWorkflowArtifactResponse { content })
                    }
                },
                sacp::on_receive_request!(),
            )
            // ═══ Artifact Versioning — list session artifacts ═══
            .on_receive_request_from(
                sacp::Client,
                {
                    let artifact_store = s.infra.brain.artifacts().clone();
                    async move |req: ListSessionArtifactsRequest,
                                responder: Responder<ListSessionArtifactsResponse>,
                                _cx: ConnectionTo<Conductor>| {
                        info!(
                            "ilhae/list_session_artifacts RPC session={}",
                            req.session_id
                        );
                        let artifacts = artifact_store
                            .list_session_artifacts(&req.session_id)
                            .unwrap_or_else(|e| {
                                warn!("DB error listing artifacts: {}", e);
                                vec![]
                            });
                        responder.respond(ListSessionArtifactsResponse { artifacts })
                    }
                },
                sacp::on_receive_request!(),
            )
            // ═══ Artifact Versioning — list versions of a file ═══
            .on_receive_request_from(
                sacp::Client,
                {
                    let artifact_store = s.infra.brain.artifacts().clone();
                    async move |req: ListArtifactVersionsRequest,
                                responder: Responder<ListArtifactVersionsResponse>,
                                _cx: ConnectionTo<Conductor>| {
                        info!(
                            "ilhae/list_artifact_versions RPC session={} filename={}",
                            req.session_id, req.filename
                        );
                        let versions = artifact_store
                            .list_artifact_versions(&req.session_id, &req.filename)
                            .unwrap_or_else(|e| {
                                warn!("DB error listing versions: {}", e);
                                vec![]
                            });
                        responder.respond(ListArtifactVersionsResponse { versions })
                    }
                },
                sacp::on_receive_request!(),
            )
            // ═══ Artifact Versioning — get specific version ═══
            .on_receive_request_from(
                sacp::Client,
                {
                    let artifact_store = s.infra.brain.artifacts().clone();
                    async move |req: GetArtifactVersionRequest,
                                responder: Responder<GetArtifactVersionResponse>,
                                _cx: ConnectionTo<Conductor>| {
                        info!(
                            "ilhae/get_artifact_version RPC session={} filename={} version={}",
                            req.session_id, req.filename, req.version
                        );
                        let artifact = artifact_store
                            .get_artifact_version(&req.session_id, &req.filename, req.version)
                            .unwrap_or_else(|e| {
                                warn!("DB error getting version: {}", e);
                                None
                            });
                        responder.respond(GetArtifactVersionResponse { artifact })
                    }
                },
                sacp::on_receive_request!(),
            )
    }};
}
