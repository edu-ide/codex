// Imports used inside macro expansion — suppress unused warnings.
#[allow(unused_imports)]
use sacp::{Client, Conductor, ConnectionTo, Responder};
#[allow(unused_imports)]
use serde_json::json;
#[allow(unused_imports)]
use std::sync::Arc;
#[allow(unused_imports)]
use tracing::{info, warn};

#[allow(unused_imports)]
use crate::admin_proxy::default_team_config;
#[allow(unused_imports)]
use crate::{
    ClaimSharedTaskRequest, ClaimSharedTaskResponse, CreateSharedTaskRequest,
    CreateSharedTaskResponse, CreateTaskRequest, CreateTaskResponse, DeleteTaskRequest,
    DeleteTaskResponse, ListProjectsRequest, ListProjectsResponse, ListSharedTasksRequest,
    ListSharedTasksResponse, ListTasksRequest, ListTasksResponse, SharedState, SharedTaskDto,
    UpdateTaskRequest, UpdateTaskResponse,
};

#[macro_export]
macro_rules! register_admin_task_handlers {
    ($builder:expr, $state:expr) => {{
        fn append_preferred_roles_hint(
            instructions: Option<&str>,
            preferred_roles: Option<&Vec<String>>,
        ) -> Option<String> {
            let preferred = preferred_roles
                .map(|roles| {
                    roles.iter()
                        .map(|role| role.trim().to_ascii_lowercase())
                        .filter(|role| !role.is_empty())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            if preferred.is_empty() {
                return instructions.map(|value| value.to_string());
            }
            let marker = format!("[ilhae:preferred_roles={}]", preferred.join(","));
            let mut merged = instructions.unwrap_or("").trim().to_string();
            if !merged.is_empty() {
                merged.push_str("\n\n");
            }
            merged.push_str(&marker);
            Some(merged)
        }

        fn first_preferred_role(preferred_roles: Option<&Vec<String>>) -> Option<String> {
            preferred_roles.and_then(|roles| {
                roles.iter()
                    .map(|role| role.trim().to_ascii_lowercase())
                    .find(|role| !role.is_empty())
            })
        }

        let s = $state.clone();
        $builder
            .on_receive_request_from(Client, {
                let brain = s.infra.brain.clone();
                async move |_req: crate::IlhaeAppTaskListRequest, responder: Responder<crate::IlhaeAppTaskListResponse>, _cx: ConnectionTo<Conductor>| {
                    info!("ilhae/app/task/list RPC");
                    let tasks = brain
                        .schedule_list()
                        .into_iter()
                        .filter_map(|task| serde_json::to_value(task).ok())
                        .collect();
                    responder.respond(crate::IlhaeAppTaskListResponse { tasks })
                }
            }, sacp::on_receive_request!())
            .on_receive_request_from(Client, {
                let brain = s.infra.brain.clone();
                async move |req: crate::IlhaeAppTaskGetRequest, responder: Responder<crate::IlhaeAppTaskGetResponse>, _cx: ConnectionTo<Conductor>| {
                    info!("ilhae/app/task/get RPC id={}", req.task_id);
                    let task = brain
                        .schedule_list()
                        .into_iter()
                        .find(|task| task.id == req.task_id)
                        .and_then(|task| serde_json::to_value(task).ok());
                    responder.respond(crate::IlhaeAppTaskGetResponse { task })
                }
            }, sacp::on_receive_request!())
            .on_receive_request_from(Client, {
                let brain = s.infra.brain.clone();
                async move |req: crate::IlhaeAppTaskCreateRequest, responder: Responder<crate::IlhaeAppTaskCreateResponse>, _cx: ConnectionTo<Conductor>| {
                    info!("ilhae/app/task/create RPC title={}", req.task.title);
                    let task = req.task;
                    let preferred_agent = first_preferred_role(task.preferred_roles.as_ref());
                    let instructions =
                        append_preferred_roles_hint(task.instructions.as_deref(), task.preferred_roles.as_ref());
                    match brain.schedule_create(
                        &task.title,
                        task.description.as_deref(),
                        task.schedule.as_deref(),
                        task.category.as_deref(),
                        task.days.unwrap_or_default(),
                        task.prompt.as_deref(),
                        task.cron_expr.as_deref(),
                        task.target_url.as_deref(),
                        instructions.as_deref(),
                        task.enabled,
                    ) {
                        Ok(task) => {
                            let task = if preferred_agent.is_some() || instructions.is_some() {
                                brain.schedule_update_full(
                                    &task.id,
                                    None,
                                    None,
                                    None,
                                    None,
                                    None,
                                    None,
                                    None,
                                    None,
                                    None,
                                    None,
                                    instructions.as_deref(),
                                    None,
                                    preferred_agent.as_deref(),
                                    None,
                                    None,
                                    None,
                                ).unwrap_or(task)
                            } else {
                                task
                            };
                            responder.respond(crate::IlhaeAppTaskCreateResponse {
                                task: serde_json::to_value(task).ok(),
                            })
                        }
                        Err(e) => responder.respond_with_error(sacp::util::internal_error(e)),
                    }
                }
            }, sacp::on_receive_request!())
            .on_receive_request_from(Client, {
                let brain = s.infra.brain.clone();
                async move |req: crate::IlhaeAppTaskUpdateRequest, responder: Responder<crate::IlhaeAppTaskUpdateResponse>, _cx: ConnectionTo<Conductor>| {
                    info!("ilhae/app/task/update RPC id={}", req.task.id);
                    let task = req.task;
                    let preferred_agent = first_preferred_role(task.preferred_roles.as_ref());
                    let instructions =
                        append_preferred_roles_hint(task.instructions.as_deref(), task.preferred_roles.as_ref());
                    match brain.schedule_update_full(
                        &task.id,
                        task.title.as_deref(),
                        task.description.as_deref(),
                        task.done,
                        task.status.as_deref(),
                        task.schedule.as_deref(),
                        task.category.as_deref(),
                        task.days,
                        task.prompt.as_deref(),
                        task.cron_expr.as_deref(),
                        task.target_url.as_deref(),
                        instructions.as_deref(),
                        task.enabled,
                        preferred_agent.as_deref(),
                        None,
                        None,
                        None,
                    ) {
                        Ok(task) => responder.respond(crate::IlhaeAppTaskUpdateResponse {
                            task: serde_json::to_value(task).ok(),
                        }),
                        Err(e) => responder.respond_with_error(sacp::util::internal_error(e)),
                    }
                }
            }, sacp::on_receive_request!())
            .on_receive_request_from(Client, {
                let brain = s.infra.brain.clone();
                async move |req: crate::IlhaeAppTaskDeleteRequest, responder: Responder<crate::IlhaeAppTaskDeleteResponse>, _cx: ConnectionTo<Conductor>| {
                    info!("ilhae/app/task/delete RPC id={}", req.task_id);
                    match brain.schedule_delete(&req.task_id) {
                        Ok(()) => responder.respond(crate::IlhaeAppTaskDeleteResponse { ok: true }),
                        Err(e) => responder.respond_with_error(sacp::util::internal_error(e)),
                    }
                }
            }, sacp::on_receive_request!())
            .on_receive_request_from(Client, {
                let brain = s.infra.brain.clone();
                let settings = s.infra.settings_store.clone();
                async move |req: crate::IlhaeAppTaskRunRequest, responder: Responder<crate::IlhaeAppTaskRunResponse>, _cx: ConnectionTo<Conductor>| {
                    info!("ilhae/app/task/run RPC");
                    if !settings.get().agent.kairos_enabled {
                        return responder.respond_with_error(sacp::Error::new(
                            -32602,
                            "kairos is disabled in the active ilhae profile".to_string(),
                        ));
                    }
                    let triggered = brain
                        .schedule_run_with_scope(None, req.task_id.as_deref())
                        .into_iter()
                        .filter_map(|task| serde_json::to_value(task).ok())
                        .collect();
                    responder.respond(crate::IlhaeAppTaskRunResponse { triggered })
                }
            }, sacp::on_receive_request!())
            .on_receive_request_from(Client, {
                let brain = s.infra.brain.clone();
                async move |_req: ListTasksRequest, responder: Responder<ListTasksResponse>, _cx: ConnectionTo<Conductor>| {
                    info!("ilhae/list_schedules RPC");
                    let mut schedules = brain.schedule_list();

                    // On-the-fly scan of active vault schedules for markdown checkboxes
                    let vault_dir = brain.vault_dir();
                    let schedules_dir = vault_dir.join("schedules");
                    let _ = std::fs::create_dir_all(&schedules_dir);
                    if schedules_dir.exists() {
                        for entry in walkdir::WalkDir::new(&schedules_dir)
                            .into_iter()
                            .filter_map(Result::ok)
                            .filter(|e| e.file_type().is_file())
                        {
                            let path = entry.path();
                            if path.extension().and_then(|s| s.to_str()) != Some("md") {
                                continue;
                            }
                            // Skip the legacy Tasks.md if it happens to be moved here,
                            // though ScheduleStore uses vault/Tasks.md
                            if let Ok(content) = std::fs::read_to_string(path) {
                                let filename = path.file_stem().and_then(|s| s.to_str()).unwrap_or("Unnamed").to_string();
                                let rel_path = path.strip_prefix(&schedules_dir).unwrap_or(path).to_string_lossy().to_string();

                                // ── Parse YAML Frontmatter ──
                                let mut fm_schedule: Option<String> = None;
                                let mut fm_category: Option<String> = None;
                                let mut fm_days: Vec<u8> = vec![];
                                let mut fm_prompt: Option<String> = None;
                                let mut fm_cron_expr: Option<String> = None;
                                let mut fm_target_url: Option<String> = None;
                                let mut fm_instructions: Option<String> = None;
                                let mut fm_schedule_type: String = "task".to_string();
                                let mut fm_created_at: Option<String> = None;
                                let mut fm_due: Option<String> = None;
                                let mut fm_priority: Option<String> = None;
                                let mut fm_description: Option<String> = None;

                                let mut body_start = 0usize;
                                let lines_vec: Vec<&str> = content.lines().collect();

                                if lines_vec.first().map(|l| l.trim()) == Some("---") {
                                    let mut closing = None;
                                    for (i, line) in lines_vec.iter().enumerate().skip(1) {
                                        if line.trim() == "---" { closing = Some(i); break; }
                                    }
                                    if let Some(close_idx) = closing {
                                        body_start = close_idx + 1;
                                        for fmline in &lines_vec[1..close_idx] {
                                            if let Some((key, val)) = fmline.split_once(':') {
                                                let k = key.trim().to_lowercase();
                                                let v = val.trim().trim_matches('"').trim_matches('\'').to_string();
                                                match k.as_str() {
                                                    "schedule" | "time" => fm_schedule = Some(v),
                                                    "category" | "tag" | "tags" => fm_category = Some(v),
                                                    "days" => {
                                                        let cleaned = v.trim_start_matches('[').trim_end_matches(']');
                                                        fm_days = cleaned.split(',').filter_map(|s| s.trim().parse::<u8>().ok()).collect();
                                                    }
                                                    "prompt" => fm_prompt = Some(v),
                                                    "cron" | "cron_expr" | "interval" => fm_cron_expr = Some(v),
                                                    "url" | "target_url" => fm_target_url = Some(v),
                                                    "instructions" => fm_instructions = Some(v),
                                                    "type" | "task_type" => fm_schedule_type = v,
                                                    "created" | "created_at" | "date" => fm_created_at = Some(v),
                                                    "due" | "due_date" => fm_due = Some(v),
                                                    "priority" => fm_priority = Some(v),
                                                    "description" | "desc" => fm_description = Some(v),
                                                    _ => {}
                                                }
                                            }
                                        }
                                    }
                                }

                                let created_at = fm_created_at.unwrap_or_else(|| chrono::Utc::now().to_rfc3339());
                                let mut desc_parts: Vec<String> = vec![];
                                if let Some(ref d) = fm_description { desc_parts.push(d.clone()); }
                                if let Some(ref due) = fm_due { desc_parts.push(format!("📅 {}", due)); }
                                if let Some(ref pri) = fm_priority { desc_parts.push(format!("⚡ {}", pri)); }
                                let description = if desc_parts.is_empty() { None } else { Some(desc_parts.join(" | ")) };

                                for (line_idx, line) in lines_vec.iter().enumerate().skip(body_start) {
                                    let trimmed = line.trim();
                                    if trimmed.starts_with("- [") && trimmed.len() > 6 {
                                        let status_char = trimmed.chars().nth(3).unwrap_or(' ');
                                        if status_char == 'x' || status_char == 'X' || status_char == ' ' {
                                            let done = status_char != ' ';
                                            let title = trimmed[5..].split("<!--").next().unwrap_or(&trimmed[5..]).trim();
                                            if !title.is_empty() {
                                                let id = format!("mdtask:{}:{}", rel_path, line_idx);
                                                schedules.push(brain_rs::schedule::Schedule {
                                                    id,
                                                    title: title.to_string(),
                                                    description: description.clone(),
                                                    done,
                                                    schedule: fm_schedule.clone(),
                                                    category: fm_category.clone().or(Some(filename.clone())),
                                                    days: fm_days.clone(),
                                                    prompt: fm_prompt.clone(),
                                                    cron_expr: fm_cron_expr.clone(),
                                                    enabled: true,
                                                    plugins: std::collections::HashMap::new(),
                                                    target_url: fm_target_url.clone(),
                                                    instructions: fm_instructions.clone(),
                                                    history: vec![],
                                                    last_run_status: None,
                                                    last_run: None,
                                                    result_summary: None,
                                                    created_at: created_at.clone(),
                                                    completed_at: if done { Some(created_at.clone()) } else { None },
                                                    schedule_type: fm_schedule_type.clone(),
                                                    status: "pending".to_string(),
                                                    assigned_agent: None,
                                                    priority: brain_rs::schedule::default_priority(),
                                                    retry_count: 0,
                                                    max_retries: brain_rs::schedule::default_max_retries(),
                                                });
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    let schedules_val = serde_json::to_value(&schedules).unwrap_or(json!([]));
                    responder.respond(ListTasksResponse { schedules: schedules_val })
                }
            }, sacp::on_receive_request!())
            // ═══ Task Create ═══
            .on_receive_request_from(Client, {
                let brain = s.infra.brain.clone();
                async move |req: CreateTaskRequest, responder: Responder<CreateTaskResponse>, _cx: ConnectionTo<Conductor>| {
                    info!("ilhae/create_task RPC title={}", req.title);

                    let schedules_dir = brain.vault_dir().join("schedules");
                    let _ = std::fs::create_dir_all(&schedules_dir);
                    let inbox_path = schedules_dir.join("inbox.md");

                    // Creates inbox.md if it doesn't exist, and append
                    use std::fs::OpenOptions;
                    use std::io::Write;

                    let mut file = match OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&inbox_path) {
                            Ok(f) => f,
                            Err(e) => return responder.respond(CreateTaskResponse { task: None, error: Some(e.to_string()) }),
                        };

                    let line = format!("- [ ] {}\n", req.title);
                    if let Err(e) = file.write_all(line.as_bytes()) {
                        return responder.respond(CreateTaskResponse { task: None, error: Some(e.to_string()) });
                    }

                    // We can just return a true ok, wait, we need to return a Task.
                    // But we don't know the exact index unless we read the file.
                    // Actually, the frontend will refetch `list_schedules` anyway, so we just return a dummy proxy task or None.
                    // Let's create a minimal task representation.
                    let task = brain_rs::schedule::Schedule {
                        id: req.id.clone().unwrap_or_else(|| "mdtask:inbox.md:new".to_string()),
                        title: req.title.clone(),
                        description: req.description,
                        done: false,
                        schedule: None,
                        category: Some("inbox.md".to_string()),
                        days: vec![],
                        prompt: None,
                        cron_expr: None,
                        enabled: true,
                        plugins: std::collections::HashMap::new(),
                        target_url: None,
                        instructions: None,
                        history: vec![],
                        last_run_status: None,
                        last_run: None,
                        result_summary: None,
                        created_at: chrono::Utc::now().to_rfc3339(),
                        completed_at: None,
                        schedule_type: "task".to_string(),
                        status: "pending".to_string(),
                        assigned_agent: None,
                        priority: brain_rs::schedule::default_priority(),
                        retry_count: 0,
                        max_retries: brain_rs::schedule::default_max_retries(),
                    };
                    responder.respond(CreateTaskResponse { task: Some(serde_json::to_value(task).unwrap()), error: None })
                }
            }, sacp::on_receive_request!())
            // ═══ Task Update ═══
            .on_receive_request_from(Client, {
                let brain = s.infra.brain.clone();
                async move |req: UpdateTaskRequest, responder: Responder<UpdateTaskResponse>, _cx: ConnectionTo<Conductor>| {
                    info!("ilhae/update_task RPC id={}", req.id);

                    if let Some(rest) = req.id.strip_prefix("mdtask:") {
                        let mut parts = rest.rsplitn(2, ':');
                        if let (Some(line_idx_str), Some(rel_path)) = (parts.next(), parts.next()) {
                            if let Ok(line_idx) = line_idx_str.parse::<usize>() {
                                let schedules_dir = brain.vault_dir().join("schedules");
                                let file_path = schedules_dir.join(rel_path);
                                if let Ok(content) = std::fs::read_to_string(&file_path) {
                                    let mut lines: Vec<String> = content.lines().map(String::from).collect();
                                    if line_idx < lines.len() {
                                        let mut line = lines[line_idx].clone();
                                        if let Some(done) = req.done {
                                            let new_char = if done { 'x' } else { ' ' };
                                            if line.trim().starts_with("- [") && line.trim().len() >= 6 {
                                                if let Some(bracket_idx) = line.find("- [") {
                                                    line.replace_range(bracket_idx+3..bracket_idx+4, &new_char.to_string());
                                                }
                                            }
                                        }
                                        if let Some(title) = &req.title {
                                            if line.trim().starts_with("- [") && line.trim().len() >= 6 {
                                                if let Some(bracket_idx) = line.find("- [") {
                                                    let prefix = &line[0..bracket_idx+6];
                                                    let fname = format!("{}.md", title);
                                                    // This `id` variable is not used, but it was in the user's provided snippet.
                                                    // Keeping it for faithful reproduction of the user's requested change.
                                                    let _id = Some(fname.clone());
                                                    let _file_path = schedules_dir.join(&fname);
                                                    line = format!("{} {}", prefix, title);
                                                }
                                            }
                                        }
                                        lines[line_idx] = line;
                                        let _ = std::fs::write(&file_path, lines.join("\n"));

                                        let task = brain_rs::schedule::Schedule {
                                            id: req.id.clone(),
                                            title: req.title.unwrap_or_default(),
                                            description: None,
                                            done: req.done.unwrap_or(false),
                                            schedule: None,
                                            category: Some(file_path.file_stem().and_then(|s| s.to_str()).unwrap_or("Unnamed").to_string()),
                                            days: vec![],
                                            prompt: None,
                                            cron_expr: None,
                                            enabled: true,
                                            plugins: std::collections::HashMap::new(),
                                            target_url: None,
                                            instructions: None,
                                            history: vec![],
                                            last_run_status: None,
                                            last_run: None,
                                            result_summary: None,
                                            created_at: chrono::Utc::now().to_rfc3339(),
                                            completed_at: None,
                                            schedule_type: "task".to_string(),
                                            status: "pending".to_string(),
                                            assigned_agent: None,
                                            priority: brain_rs::schedule::default_priority(),
                                            retry_count: 0,
                                            max_retries: brain_rs::schedule::default_max_retries(),
                                        };
                                        return responder.respond(UpdateTaskResponse { task: Some(serde_json::to_value(task).unwrap()), error: None });
                                    }
                                }
                            }
                        }
                    }

                    match brain.schedule_update_full(
                        &req.id,
                        req.title.as_deref(),
                        req.description.as_deref(),
                        req.done,
                        req.status.as_deref(),
                        None, // schedule
                        None, // category
                        None, // days
                        None, // prompt
                        None, // cron_expr
                        None, // target_url
                        None, // instructions
                        None, // enabled
                        None, // assigned_agent
                        None, // priority
                        None, // retry_count
                        None, // max_retries
                    ) {
                        Ok(task) => {
                            let task_val = serde_json::to_value(&task).ok();
                            responder.respond(UpdateTaskResponse { task: task_val, error: None })
                        }
                        Err(e) => {
                            warn!("update_task error: {}", e);
                            responder.respond(UpdateTaskResponse { task: None, error: Some(e) })
                        }
                    }
                }
            }, sacp::on_receive_request!())
            // ═══ Task Delete ═══
            .on_receive_request_from(Client, {
                let brain = s.infra.brain.clone();
                async move |req: DeleteTaskRequest, responder: Responder<DeleteTaskResponse>, _cx: ConnectionTo<Conductor>| {
                    info!("ilhae/delete_task RPC id={}", req.id);

                    if let Some(rest) = req.id.strip_prefix("mdtask:") {
                        let mut parts = rest.rsplitn(2, ':');
                        if let (Some(line_idx_str), Some(rel_path)) = (parts.next(), parts.next()) {
                            if let Ok(line_idx) = line_idx_str.parse::<usize>() {
                                let schedules_dir = brain.vault_dir().join("schedules");
                                let file_path = schedules_dir.join(rel_path);
                                if let Ok(content) = std::fs::read_to_string(&file_path) {
                                    let mut lines: Vec<String> = content.lines().map(String::from).collect();
                                    if line_idx < lines.len() {
                                        lines.remove(line_idx);
                                        let _ = std::fs::write(&file_path, lines.join("\n"));
                                        return responder.respond(DeleteTaskResponse { ok: true, error: None });
                                    }
                                }
                            }
                        }
                    }

                    match brain.schedule_delete(&req.id) {
                        Ok(()) => responder.respond(DeleteTaskResponse { ok: true, error: None }),
                        Err(e) => {
                            warn!("delete_task error: {}", e);
                            responder.respond(DeleteTaskResponse { ok: false, error: Some(e) })
                        }
                    }
                }
            }, sacp::on_receive_request!())
            // ═══ Project List ═══
            .on_receive_request_from(Client, {
                let brain = s.infra.brain.clone();
                async move |_req: ListProjectsRequest, responder: Responder<ListProjectsResponse>, _cx: ConnectionTo<Conductor>| {
                    info!("ilhae/list_projects RPC");
                    let projects = brain.schedule_list_projects();
                    let projects_val = serde_json::to_value(&projects).unwrap_or(json!([]));
                    responder.respond(ListProjectsResponse { projects: projects_val })
                }
            }, sacp::on_receive_request!())
            .on_receive_request_from(Client, {
                let pool = s.infra.shared_task_pool.clone();
                async move |req: CreateSharedTaskRequest, responder: Responder<CreateSharedTaskResponse>, _cx: ConnectionTo<Conductor>| {
                    info!("ilhae/create_shared_task RPC desc={}", req.description);
                    let task = SharedTaskDto {
                        id: uuid::Uuid::new_v4().to_string(),
                        description: req.description,
                        created_at: chrono::Utc::now().to_rfc3339(),
                        claimed_by: None,
                        state: "pending".to_string(),
                    };
                    pool.write().await.push(task.clone());
                    responder.respond(CreateSharedTaskResponse { task })
                }
            }, sacp::on_receive_request!())
            // ═══ Shared Task Pool — list ═══
            .on_receive_request_from(Client, {
                let pool = s.infra.shared_task_pool.clone();
                async move |_req: ListSharedTasksRequest, responder: Responder<ListSharedTasksResponse>, _cx: ConnectionTo<Conductor>| {
                    let schedules = pool.read().await.clone();
                    responder.respond(ListSharedTasksResponse { schedules })
                }
            }, sacp::on_receive_request!())
            // ═══ Shared Task Pool — claim (forwards to agent via A2A message/send) ═══
            .on_receive_request_from(Client, {
                let pool = s.infra.shared_task_pool.clone();
                let ilhae_dir = s.infra.ilhae_dir.clone();
                async move |req: ClaimSharedTaskRequest, responder: Responder<ClaimSharedTaskResponse>, cx: ConnectionTo<Conductor>| {
                    info!("ilhae/claim_shared_task RPC schedule_id={} agent_role={}", req.schedule_id, req.agent_role);
                    let pool = pool.clone();
                    let ilhae_dir = ilhae_dir.clone();
                    cx.spawn(async move {
                        let mut schedules = pool.write().await;
                        let idx = schedules.iter().position(|t| t.id == req.schedule_id);
                        let Some(idx) = idx else {
                            return responder.respond(ClaimSharedTaskResponse {
                                success: false,
                                task: None,
                                error: Some("Task not found".to_string()),
                            });
                        };
                        if schedules[idx].claimed_by.is_some() {
                            return responder.respond(ClaimSharedTaskResponse {
                                success: false,
                                task: Some(schedules[idx].clone()),
                                error: Some(format!("Already claimed by {}", schedules[idx].claimed_by.as_deref().unwrap_or("?"))),
                            });
                        }

                        // Find agent endpoint
                        let team_path = ilhae_dir.join("brain").join("settings").join("team_config.json");
                        let config: serde_json::Value = match std::fs::read_to_string(&team_path) {
                            Ok(s) => serde_json::from_str(&s).unwrap_or(default_team_config()),
                            Err(_) => default_team_config(),
                        };
                        let agents = config.get("agents").and_then(|v| v.as_array()).cloned().unwrap_or_default();
                        let endpoint = agents.iter().find_map(|a| {
                            let role = a.get("role").and_then(|v| v.as_str()).unwrap_or("");
                            if role.eq_ignore_ascii_case(&req.agent_role) {
                                a.get("endpoint").and_then(|v| v.as_str()).map(|s| s.to_string())
                            } else {
                                None
                            }
                        });

                        let Some(endpoint) = endpoint else {
                            return responder.respond(ClaimSharedTaskResponse {
                                success: false,
                                task: None,
                                error: Some(format!("Agent '{}' not found in team config", req.agent_role)),
                            });
                        };

                        // Forward to agent via A2A message/send (v0.3.0 spec)
                        let msg_id = uuid::Uuid::new_v4().to_string();
                        let body = serde_json::json!({
                            "jsonrpc": "2.0",
                            "id": 1,
                            "method": "message/send",
                            "params": {
                                "message": {
                                    "role": "user",
                                    "messageId": msg_id,
                                    "parts": [{ "kind": "text", "text": schedules[idx].description.clone() }]
                                }
                            }
                        });

                        let client = reqwest::Client::builder()
                            .timeout(std::time::Duration::from_secs(30))
                            .build()
                            .unwrap_or_default();

                        match client.post(&endpoint).json(&body).send().await {
                            Ok(res) if res.status().is_success() => {
                                schedules[idx].claimed_by = Some(req.agent_role);
                                schedules[idx].state = "working".to_string();
                                let task = schedules[idx].clone();
                                responder.respond(ClaimSharedTaskResponse {
                                    success: true,
                                    task: Some(task),
                                    error: None,
                                })
                            }
                            Ok(res) => {
                                let status = res.status().to_string();
                                responder.respond(ClaimSharedTaskResponse {
                                    success: false,
                                    task: Some(schedules[idx].clone()),
                                    error: Some(format!("A2A message/send failed: {}", status)),
                                })
                            }
                            Err(e) => {
                                responder.respond(ClaimSharedTaskResponse {
                                    success: false,
                                    task: Some(schedules[idx].clone()),
                                    error: Some(format!("A2A message/send error: {}", e)),
                                })
                            }
                        }
                    })
                }
            }, sacp::on_receive_request!())
    }};
}
