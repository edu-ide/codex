#[macro_export]
macro_rules! register_task_tools {
    ($builder:expr, $brain_service:expr, $bt_settings:expr) => {{
        use $crate::{
            EmptyInput, IdInput, TaskAddHistoryInput, TaskCreateInput, TaskUpdateInput,
        };

        $builder
            // ─── Task ─────
            .tool_fn(
                "task_list",
                "List all daily schedules.",
                {
                    let brain = $brain_service.clone();
                    let bts = $bt_settings.clone();
                    async move |_input: EmptyInput, _cx| {
                        $crate::check_tool_enabled!(bts, "task_list");
                        let list = brain.schedule_list();
                        let text = serde_json::to_string_pretty(&list).unwrap_or("[]".to_string());
                        Ok::<String, sacp::Error>(text)
                    }
                },
                sacp::tool_fn!(),
            )
            .tool_fn(
                "task_create",
                "Create a new task (todo, scheduled, cron, or automation mission). Use 'cron_expr' (e.g. \"30m\", \"1h\") for recurring, 'prompt' for agent command, 'target_url' for web automation.",
                {
                    let brain = $brain_service.clone();
                    let bts = $bt_settings.clone();
                    async move |input: TaskCreateInput, _cx| {
                        $crate::check_tool_enabled!(bts, "task_create");
                        match brain.schedule_create(
                            &input.title,
                            input.description.as_deref(),
                            input.schedule.as_deref(),
                            input.category.as_deref(),
                            input.days.unwrap_or_default(),
                            input.prompt.as_deref(),
                            input.cron_expr.as_deref(),
                            input.target_url.as_deref(),
                            input.instructions.as_deref(),
                            input.enabled,
                        ) {
                            Ok(t) => {
                                let days_str = if t.days.is_empty() {
                                    "매일".to_string()
                                } else {
                                    let names = ["일","월","화","수","목","금","토"];
                                    t.days.iter().map(|d| *names.get(*d as usize).unwrap_or(&"?")).collect::<Vec<_>>().join(",")
                                };
                                let schedule_str = t.schedule.as_deref().unwrap_or("미지정");
                                let cron_str = t.cron_expr.as_deref().unwrap_or("없음");
                                Ok::<String, sacp::Error>(format!("✅ Created task '{}' (id: {}, schedule: {}, days: {}, cron: {})", t.title, t.id, schedule_str, days_str, cron_str))
                            }
                            Err(e) => Err(sacp::Error::internal_error().data(e.to_string())),
                        }
                    }
                },
                sacp::tool_fn!(),
            )
            .tool_fn(
                "task_update",
                "Update any task property: title, description, schedule, category, days, prompt, cron_expr, target_url, instructions, enabled, done, status.",
                {
                    let brain = $brain_service.clone();
                    let bts = $bt_settings.clone();
                    async move |input: TaskUpdateInput, _cx| {
                        $crate::check_tool_enabled!(bts, "task_update");
                        match brain.schedule_update(
                            &input.id,
                            input.title.as_deref(),
                            input.description.as_deref(),
                            input.done,
                            input.status.as_deref(),
                            input.schedule.as_deref(),
                            input.category.as_deref(),
                            input.days,
                            input.prompt.as_deref(),
                            input.cron_expr.as_deref(),
                            input.target_url.as_deref(),
                            input.instructions.as_deref(),
                            input.enabled,
                        ) {
                            Ok(t) => {
                                let done_str = if t.done { "✅" } else { "⬜" };
                                Ok::<String, sacp::Error>(format!("{} Task '{}' updated (status: {})", done_str, t.title, t.last_run_status.as_deref().unwrap_or("none")))
                            }
                            Err(e) => Err(sacp::Error::internal_error().data(e.to_string())),
                        }
                    }
                },
                sacp::tool_fn!(),
            )
            .tool_fn(
                "task_delete",
                "Delete a task by ID.",
                {
                    let brain = $brain_service.clone();
                    let bts = $bt_settings.clone();
                    async move |input: IdInput, _cx| {
                        $crate::check_tool_enabled!(bts, "task_delete");
                        match brain.schedule_delete(&input.id) {
                            Ok(()) => Ok::<String, sacp::Error>(format!("✅ Deleted task {}", input.id)),
                            Err(e) => Err(sacp::Error::internal_error().data(e.to_string())),
                        }
                    }
                },
                sacp::tool_fn!(),
            )
            .tool_fn(
                "task_add_history",
                "Add a history entry to a task. Use to record execution results, agent actions, cron runs, etc.",
                {
                    let brain = $brain_service.clone();
                    let bts = $bt_settings.clone();
                    async move |input: TaskAddHistoryInput, _cx| {
                        $crate::check_tool_enabled!(bts, "task_add_history");
                        match brain.schedule_add_history(
                            &input.id,
                            &input.action,
                            input.detail.as_deref(),
                            input.session_id.as_deref(),
                        ) {
                            Ok(t) => Ok::<String, sacp::Error>(format!(
                                "✅ Added '{}' history to task '{}' (total: {} entries)",
                                input.action, t.title, t.history.len()
                            )),
                            Err(e) => Err(sacp::Error::internal_error().data(e.to_string())),
                        }
                    }
                },
                sacp::tool_fn!(),
            )
            .tool_fn(
                "task_run",
                "Immediately trigger all due cron-style schedules. Returns triggered task prompts.",
                {
                    let brain = $brain_service.clone();
                    let bts = $bt_settings.clone();
                    async move |_input: EmptyInput, _cx| {
                        $crate::check_tool_enabled!(bts, "task_run");
                        let triggered = brain.schedule_run();
                        if triggered.is_empty() {
                            Ok::<String, sacp::Error>("No schedules are currently due.".to_string())
                        } else {
                            let lines: Vec<String> = triggered.iter().map(|t| format!("- [{}] {}", t.schedule_id, t.title)).collect();
                            Ok::<String, sacp::Error>(format!("Triggered {} schedules:\n{}", triggered.len(), lines.join("\n")))
                        }
                    }
                },
                sacp::tool_fn!(),
            )
            // ─── Project aliases ─────
            .tool_fn(
                "project_list",
                "List all project schedules.",
                {
                    let brain = $brain_service.clone();
                    let bts = $bt_settings.clone();
                    async move |_input: EmptyInput, _cx| {
                        $crate::check_tool_enabled!(bts, "task_list");
                        let list = brain.schedule_list_projects();
                        let text = serde_json::to_string_pretty(&list).unwrap_or("[]".to_string());
                        Ok::<String, sacp::Error>(text)
                    }
                },
                sacp::tool_fn!(),
            )
            .tool_fn(
                "project_create",
                "Create a new project task.",
                {
                    let brain = $brain_service.clone();
                    let bts = $bt_settings.clone();
                    async move |input: TaskCreateInput, _cx| {
                        $crate::check_tool_enabled!(bts, "task_create");
                        match brain.schedule_create_project(
                            &input.title,
                            input.description.as_deref(),
                            input.schedule.as_deref(),
                            input.category.as_deref(),
                            input.days.unwrap_or_default(),
                            input.prompt.as_deref(),
                            input.cron_expr.as_deref(),
                            input.target_url.as_deref(),
                            input.instructions.as_deref(),
                            input.enabled,
                        ) {
                            Ok(t) => {
                                let days_str = if t.days.is_empty() {
                                    "매일".to_string()
                                } else {
                                    let names = ["일", "월", "화", "수", "목", "금", "토"];
                                    t.days.iter().map(|d| *names.get(*d as usize).unwrap_or(&"?")).collect::<Vec<_>>().join(",")
                                };
                                let schedule_str = t.schedule.as_deref().unwrap_or("미지정");
                                let cron_str = t.cron_expr.as_deref().unwrap_or("없음");
                                Ok::<String, sacp::Error>(format!("✅ Created project '{}' (id: {}, schedule: {}, days: {}, cron: {})", t.title, t.id, schedule_str, days_str, cron_str))
                            }
                            Err(e) => Err(sacp::Error::internal_error().data(e.to_string())),
                        }
                    }
                },
                sacp::tool_fn!(),
            )
            .tool_fn(
                "project_update",
                "Alias of task_update. Update project task properties.",
                {
                    let brain = $brain_service.clone();
                    let bts = $bt_settings.clone();
                    async move |input: TaskUpdateInput, _cx| {
                        $crate::check_tool_enabled!(bts, "task_update");
                        match brain.schedule_update(
                            &input.id,
                            input.title.as_deref(),
                            input.description.as_deref(),
                            input.done,
                            input.status.as_deref(),
                            input.schedule.as_deref(),
                            input.category.as_deref(),
                            input.days,
                            input.prompt.as_deref(),
                            input.cron_expr.as_deref(),
                            input.target_url.as_deref(),
                            input.instructions.as_deref(),
                            input.enabled,
                        ) {
                            Ok(t) => {
                                let done_str = if t.done { "✅" } else { "⬜" };
                                Ok::<String, sacp::Error>(format!("{} Project '{}' updated (status: {})", done_str, t.title, t.last_run_status.as_deref().unwrap_or("none")))
                            }
                            Err(e) => Err(sacp::Error::internal_error().data(e.to_string())),
                        }
                    }
                },
                sacp::tool_fn!(),
            )
            .tool_fn(
                "project_delete",
                "Alias of task_delete. Delete a project task by ID.",
                {
                    let brain = $brain_service.clone();
                    let bts = $bt_settings.clone();
                    async move |input: IdInput, _cx| {
                        $crate::check_tool_enabled!(bts, "task_delete");
                        match brain.schedule_delete(&input.id) {
                            Ok(()) => Ok::<String, sacp::Error>(format!("✅ Deleted project {}", input.id)),
                            Err(e) => Err(sacp::Error::internal_error().data(e.to_string())),
                        }
                    }
                },
                sacp::tool_fn!(),
            )
            .tool_fn(
                "project_add_history",
                "Alias of task_add_history. Add history entry to a project task.",
                {
                    let brain = $brain_service.clone();
                    let bts = $bt_settings.clone();
                    async move |input: TaskAddHistoryInput, _cx| {
                        $crate::check_tool_enabled!(bts, "task_add_history");
                        match brain.schedule_add_history(
                            &input.id,
                            &input.action,
                            input.detail.as_deref(),
                            input.session_id.as_deref(),
                        ) {
                            Ok(t) => Ok::<String, sacp::Error>(format!(
                                "✅ Added '{}' history to project '{}' (total: {} entries)",
                                input.action, t.title, t.history.len()
                            )),
                            Err(e) => Err(sacp::Error::internal_error().data(e.to_string())),
                        }
                    }
                },
                sacp::tool_fn!(),
            )
            .tool_fn(
                "project_run",
                "Immediately trigger due project automation schedules.",
                {
                    let brain = $brain_service.clone();
                    let bts = $bt_settings.clone();
                    async move |_input: EmptyInput, _cx| {
                        $crate::check_tool_enabled!(bts, "task_run");
                        let triggered = brain.schedule_run_projects();
                        if triggered.is_empty() {
                            Ok::<String, sacp::Error>("No project schedules are currently due.".to_string())
                        } else {
                            let lines: Vec<String> = triggered.iter().map(|t| format!("- [{}] {}", t.schedule_id, t.title)).collect();
                            Ok::<String, sacp::Error>(format!("Triggered {} project schedules:\n{}", triggered.len(), lines.join("\n")))
                        }
                    }
                },
                sacp::tool_fn!(),
            )
    }};
}
