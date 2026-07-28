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
            .on_receive_request_from(
                sacp::Client,
                {
                    let ilhae_dir = s.infra.ilhae_dir.clone();
                    async move |req: IlhaeAppWorkflowListRequest,
                                responder: Responder<IlhaeAppWorkflowListResponse>,
                                _cx: ConnectionTo<Conductor>| {
                        info!(
                            "ilhae/app/workflow/list RPC project_path={:?}",
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
                        responder.respond(IlhaeAppWorkflowListResponse { artifacts })
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
            .on_receive_request_from(
                sacp::Client,
                {
                    let ilhae_dir = s.infra.ilhae_dir.clone();
                    async move |req: IlhaeAppWorkflowGetRequest,
                                responder: Responder<IlhaeAppWorkflowGetResponse>,
                                _cx: ConnectionTo<Conductor>| {
                        info!("ilhae/app/workflow/get RPC id={:?}", req.id);
                        let vault_dir = ilhae_dir.join("vault").join("workflow");
                        let file_path = vault_dir.join(&req.id);

                        let content = std::fs::read_to_string(file_path).unwrap_or_default();
                        responder.respond(IlhaeAppWorkflowGetResponse { content })
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
            .on_receive_request_from(
                sacp::Client,
                {
                    let artifact_store = s.infra.brain.artifacts().clone();
                    async move |req: IlhaeAppArtifactListRequest,
                                responder: Responder<IlhaeAppArtifactListResponse>,
                                _cx: ConnectionTo<Conductor>| {
                        info!("ilhae/app/artifact/list RPC session={}", req.session_id);
                        let artifacts = artifact_store
                            .list_session_artifacts(&req.session_id)
                            .unwrap_or_else(|e| {
                                warn!("DB error listing artifacts: {}", e);
                                vec![]
                            });
                        responder.respond(IlhaeAppArtifactListResponse { artifacts })
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
            .on_receive_request_from(
                sacp::Client,
                {
                    let artifact_store = s.infra.brain.artifacts().clone();
                    async move |req: IlhaeAppArtifactVersionsRequest,
                                responder: Responder<IlhaeAppArtifactVersionsResponse>,
                                _cx: ConnectionTo<Conductor>| {
                        info!(
                            "ilhae/app/artifact/versions RPC session={} filename={}",
                            req.session_id, req.filename
                        );
                        let versions = artifact_store
                            .list_artifact_versions(&req.session_id, &req.filename)
                            .unwrap_or_else(|e| {
                                warn!("DB error listing versions: {}", e);
                                vec![]
                            });
                        responder.respond(IlhaeAppArtifactVersionsResponse { versions })
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
            .on_receive_request_from(
                sacp::Client,
                {
                    let artifact_store = s.infra.brain.artifacts().clone();
                    async move |req: IlhaeAppArtifactGetRequest,
                                responder: Responder<IlhaeAppArtifactGetResponse>,
                                _cx: ConnectionTo<Conductor>| {
                        info!(
                            "ilhae/app/artifact/get RPC session={} filename={} version={}",
                            req.session_id, req.filename, req.version
                        );
                        let artifact = artifact_store
                            .get_artifact_version(&req.session_id, &req.filename, req.version)
                            .unwrap_or_else(|e| {
                                warn!("DB error getting version: {}", e);
                                None
                            });
                        responder.respond(IlhaeAppArtifactGetResponse { artifact })
                    }
                },
                sacp::on_receive_request!(),
            )
            // ═══ Workflow Spec — list (seeds a sample on first run) ═══
            .on_receive_request_from(
                sacp::Client,
                {
                    let ilhae_dir = s.infra.ilhae_dir.clone();
                    async move |req: WorkflowSpecListRequest,
                                responder: Responder<WorkflowSpecListResponse>,
                                _cx: ConnectionTo<Conductor>| {
                        info!(
                            "ilhae/app/workflow/spec/list RPC industry={:?}",
                            req.industry
                        );
                        let vault_dir = ilhae_dir.join("vault").join("workflow");
                        let _ = std::fs::create_dir_all(&vault_dir);

                        // Seed a sample WORKFLOW spec if none exists yet, so the
                        // canvas is never empty on a fresh install.
                        let has_spec = std::fs::read_dir(&vault_dir)
                            .map(|entries| {
                                entries.flatten().any(|entry| {
                                    entry.file_name().to_str().is_some_and(|name| {
                                        name.starts_with("WF_") && name.ends_with(".json")
                                    })
                                })
                            })
                            .unwrap_or(false);
                        if !has_spec {
                            let seed = $crate::admin_builtins::artifact::WORKFLOW_SEED_JSON;
                            let _ = std::fs::write(vault_dir.join("WF_tax_monthly.json"), seed);
                        }

                        let mut specs = Vec::new();
                        if let Ok(entries) = std::fs::read_dir(&vault_dir) {
                            for entry in entries.flatten() {
                                let path = entry.path();
                                let name = match path.file_name().and_then(|name| name.to_str()) {
                                    Some(name)
                                        if name.starts_with("WF_") && name.ends_with(".json") =>
                                    {
                                        name.to_string()
                                    }
                                    _ => continue,
                                };
                                let content = std::fs::read_to_string(&path).unwrap_or_default();
                                let spec: WorkflowSpec = match serde_json::from_str(&content) {
                                    Ok(spec) => spec,
                                    Err(err) => {
                                        warn!("skip malformed workflow spec {name}: {err}");
                                        continue;
                                    }
                                };
                                if let Some(filter) = &req.industry
                                    && spec.industry.as_ref() != Some(filter)
                                {
                                    continue;
                                }
                                let timestamp = entry
                                    .metadata()
                                    .and_then(|metadata| metadata.modified())
                                    .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
                                    .duration_since(std::time::SystemTime::UNIX_EPOCH)
                                    .unwrap_or_default()
                                    .as_millis()
                                    as i64;
                                specs.push(WorkflowSpecSummary {
                                    id: name,
                                    title: spec.title,
                                    industry: spec.industry,
                                    coverage: spec.coverage,
                                    timestamp,
                                });
                            }
                        }
                        specs.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
                        responder.respond(WorkflowSpecListResponse { specs })
                    }
                },
                sacp::on_receive_request!(),
            )
            // ═══ Workflow Spec — get full graph ═══
            .on_receive_request_from(
                sacp::Client,
                {
                    let ilhae_dir = s.infra.ilhae_dir.clone();
                    async move |req: WorkflowSpecGetRequest,
                                responder: Responder<WorkflowSpecGetResponse>,
                                _cx: ConnectionTo<Conductor>| {
                        info!("ilhae/app/workflow/spec/get RPC id={:?}", req.id);
                        let vault_dir = ilhae_dir.join("vault").join("workflow");
                        // Reject path traversal — id must be a bare filename.
                        let spec = if req.id.contains('/') || req.id.contains('\\') {
                            warn!("rejecting workflow spec id with separators: {}", req.id);
                            None
                        } else {
                            std::fs::read_to_string(vault_dir.join(&req.id))
                                .ok()
                                .and_then(|content| {
                                    serde_json::from_str::<WorkflowSpec>(&content).ok()
                                })
                        };
                        responder.respond(WorkflowSpecGetResponse { spec })
                    }
                },
                sacp::on_receive_request!(),
            )
            // ═══ Workflow Spec — save (create or overwrite) ═══
            .on_receive_request_from(
                sacp::Client,
                {
                    let ilhae_dir = s.infra.ilhae_dir.clone();
                    async move |req: WorkflowSpecSaveRequest,
                                responder: Responder<WorkflowSpecSaveResponse>,
                                _cx: ConnectionTo<Conductor>| {
                        let mut id = req.spec.id.clone();
                        if !id.starts_with("WF_") {
                            id = format!("WF_{id}");
                        }
                        if !id.ends_with(".json") {
                            id.push_str(".json");
                        }
                        info!("ilhae/app/workflow/spec/save RPC id={id}");
                        let saved = if id.contains('/') || id.contains('\\') {
                            warn!("rejecting workflow spec id with separators: {id}");
                            false
                        } else {
                            let vault_dir = ilhae_dir.join("vault").join("workflow");
                            let _ = std::fs::create_dir_all(&vault_dir);
                            let mut spec = req.spec;
                            spec.id = id.clone();
                            spec.artifact_type = "WORKFLOW".to_string();
                            match serde_json::to_string_pretty(&spec) {
                                Ok(json) => std::fs::write(vault_dir.join(&id), json).is_ok(),
                                Err(err) => {
                                    warn!("serialize workflow spec failed: {err}");
                                    false
                                }
                            }
                        };
                        responder.respond(WorkflowSpecSaveResponse { id, saved })
                    }
                },
                sacp::on_receive_request!(),
            )
    }};
}

/// Seed workflow spec written on first list when the vault is empty.
/// Mirrors the 세무 월기장 6-step sample the WorkflowPage prototype shipped with.
pub const WORKFLOW_SEED_JSON: &str = r#"{
  "artifact_type": "WORKFLOW",
  "id": "WF_tax_monthly.json",
  "title": "월 기장 → 부가세·원천 신고 → 고객 안내",
  "industry": "세무",
  "coverage": { "total_steps": 6, "automatable": 5, "tools_ready": 4, "tools_to_build": 2 },
  "nodes": [
    { "id": "n1", "step": "STEP 1", "label": "증빙 수집·분류", "mcp": "doc-mcp/classify_documents", "kind": "auto", "status": "ready", "x": 300, "y": 128 },
    { "id": "n2", "step": "STEP 2", "label": "계정과목 매핑", "mcp": "tax-mcp/map_accounts", "kind": "auto", "status": "to_build", "x": 300, "y": 262 },
    { "id": "n3", "step": "STEP 3", "label": "세무조정 계산", "mcp": "tax-mcp/compute_adjustment", "kind": "auto", "status": "to_build", "x": 300, "y": 396 },
    { "id": "n4", "step": "STEP 4", "label": "신고서 양식 작성", "mcp": "office-mcp/fill_form", "kind": "auto", "status": "prebuilt", "x": 300, "y": 530 },
    { "id": "n5", "step": "STEP 5", "label": "최종 검토·신고 결정", "mcp": null, "kind": "approval", "status": "human", "note": "면허·책임 직군: 자동화 대상이 아니라 사람이 승인하는 게이트로 설계됨.", "x": 300, "y": 664 },
    { "id": "n6", "step": "STEP 6 · 병렬", "label": "고객 안내문 작성", "mcp": "doc-mcp/draft_notice", "kind": "auto", "status": "ready", "x": 654, "y": 498 }
  ],
  "edges": [
    { "from": "n1", "to": "n2" },
    { "from": "n2", "to": "n3" },
    { "from": "n3", "to": "n4" },
    { "from": "n4", "to": "n5", "kind": "approval" },
    { "from": "n4", "to": "n6" }
  ]
}"#;

#[cfg(test)]
mod tests {
    use super::WORKFLOW_SEED_JSON;
    use crate::WorkflowSpec;

    #[test]
    fn seed_json_roundtrips_into_workflow_spec() {
        // The list handler writes this seed then immediately parses every
        // WF_*.json back into WorkflowSpec. A schema mismatch would yield an
        // empty canvas.
        let spec: WorkflowSpec =
            serde_json::from_str(WORKFLOW_SEED_JSON).expect("seed must parse into WorkflowSpec");
        assert_eq!(spec.artifact_type, "WORKFLOW");
        assert_eq!(spec.nodes.len(), 6, "6 nodes");
        assert_eq!(spec.edges.len(), 5, "5 edges");
        assert_eq!(spec.coverage.total_steps, 6);

        let gate = spec
            .nodes
            .iter()
            .find(|node| node.kind == "approval")
            .expect("seed includes approval node");
        assert!(gate.mcp.is_none());

        let json = serde_json::to_string_pretty(&spec).expect("seed serializes");
        let back: WorkflowSpec = serde_json::from_str(&json).expect("seed re-parses");
        assert_eq!(back.nodes.len(), 6);
        assert_eq!(back.edges.len(), 5);
    }

    #[test]
    fn workflow_spec_preserves_intake_during_json_roundtrip() {
        let input = serde_json::json!({
            "artifact_type": "WORKFLOW",
            "id": "WF_captured_invoice.json",
            "title": "거래처 청구서 작성",
            "coverage": {
                "total_steps": 1,
                "automatable": 1,
                "tools_ready": 1,
                "tools_to_build": 0
            },
            "nodes": [{
                "id": "n1",
                "label": "청구서 작성",
                "mcp": "doc-mcp/create_invoice",
                "kind": "auto",
                "status": "ready",
                "x": 300,
                "y": 128
            }],
            "edges": [],
            "intake": {
                "origin": "captured_turn",
                "materials": ["/work/orders.xlsx"],
                "deliverable": "7월 청구서.docx",
                "deadline": "2026-07-31",
                "approvers": ["김회계"],
                "amount_unit": "원",
                "completion_criteria": ["합계가 원장과 일치한다"],
                "variable_candidates": [{
                    "kind": "counterparty",
                    "value": "한빛상사",
                    "confirmed": true,
                    "source": "대화"
                }]
            }
        });

        let spec: WorkflowSpec =
            serde_json::from_value(input.clone()).expect("intake가 포함된 spec을 읽어야 한다");
        let serialized = serde_json::to_value(spec).expect("workflow spec을 다시 직렬화해야 한다");

        assert_eq!(serialized.get("intake"), input.get("intake"));
    }

    #[test]
    fn workflow_spec_preserves_version_revision_during_json_roundtrip() {
        let input = serde_json::json!({
            "artifact_type": "WORKFLOW",
            "id": "WF_invoice.json",
            "title": "월 청구서",
            "version": 2,
            "activated_at": 2000,
            "coverage": {
                "total_steps": 1,
                "automatable": 1,
                "tools_ready": 1,
                "tools_to_build": 0
            },
            "nodes": [{
                "id": "total",
                "label": "부가세 포함 합계",
                "mcp": "office/formula",
                "kind": "auto",
                "status": "ready",
                "x": 0,
                "y": 0
            }],
            "edges": [],
            "version_history": [{
                "version": 1,
                "activated_at": 0,
                "reason": "최초 절차",
                "nodes": [{
                    "id": "total",
                    "label": "합계 계산",
                    "mcp": "office/formula",
                    "kind": "auto",
                    "status": "ready",
                    "x": 0,
                    "y": 0
                }],
                "edges": [],
                "coverage": {
                    "total_steps": 1,
                    "automatable": 1,
                    "tools_ready": 1,
                    "tools_to_build": 0
                }
            }],
            "pending_revision": {
                "revision_id": "revision-2",
                "workflow_id": "WF_invoice.json",
                "kind": "rollback",
                "base_version": 2,
                "target_version": 3,
                "reason": "v1 내용으로 되돌리기",
                "review_quotes": [],
                "proposed_nodes": [{
                    "id": "total",
                    "label": "합계 계산",
                    "mcp": "office/formula",
                    "kind": "auto",
                    "status": "ready",
                    "x": 0,
                    "y": 0
                }],
                "proposed_edges": [],
                "proposed_coverage": {
                    "total_steps": 1,
                    "automatable": 1,
                    "tools_ready": 1,
                    "tools_to_build": 0
                },
                "created_at": 3000,
                "approval_item_id": "approval-2",
                "approval_content_sha256": "abc123"
            }
        });

        let spec: WorkflowSpec =
            serde_json::from_value(input.clone()).expect("버전이 포함된 spec을 읽어야 한다");
        let serialized = serde_json::to_value(spec).expect("버전 spec을 다시 직렬화해야 한다");

        assert_eq!(serialized.get("version"), input.get("version"));
        assert_eq!(serialized.get("activated_at"), input.get("activated_at"));
        assert_eq!(serialized["version_history"][0]["version"], 1);
        assert_eq!(
            serialized["version_history"][0]["nodes"][0]["label"],
            "합계 계산"
        );
        assert_eq!(serialized["pending_revision"]["revision_id"], "revision-2");
        assert_eq!(
            serialized["pending_revision"]["workflow_id"],
            "WF_invoice.json"
        );
        assert_eq!(serialized["pending_revision"]["kind"], "rollback");
        assert_eq!(
            serialized["pending_revision"]["approval_item_id"],
            "approval-2"
        );
        assert_eq!(
            serialized["pending_revision"]["approval_content_sha256"],
            "abc123"
        );
    }
}
