#[macro_export]
macro_rules! register_misc_tools {
    ($builder:expr, $brain_service:expr, $bt_settings:expr, $notify_relay_tx:expr, $notif_store:expr, $state:expr) => {{
        use $crate::{EmptyInput, SkillViewInput, UiNotifyInput};
        use crate::relay_server::{RelayEvent, broadcast_event};

        $builder
            .tool_fn(
                "ui_notify",
                "Send a real-time notification to the desktop UI. Use this to alert the user about task completion, errors, or important status updates.",
                {
                    let bts = $bt_settings.clone();
                    let relay = $notify_relay_tx.clone();
                    let notif_store = $notif_store.clone();
                    async move |input: UiNotifyInput, _cx| {
                        $crate::check_tool_enabled!(bts, "ui_notify");
                        eprintln!("[ui_notify:{}] {}", input.level, input.message);
                        if let Err(e) = notif_store.add(&input.message, &input.level, "agent") {
                            tracing::warn!("Failed to persist notification: {}", e);
                        }
                        broadcast_event(&relay, RelayEvent::UiNotification {
                            message: input.message.clone(),
                            level: input.level.clone(),
                            source: Some("agent".to_string()),
                        });
                        Ok::<String, sacp::Error>(format!("✅ Notification sent: [{}] {}", input.level, input.message))
                    }
                },
                sacp::tool_fn!(),
            )
            .tool_fn(
                "skills_list",
                "List available skills from brain/skills with metadata only. Use skill_view to load full content.",
                {
                    let bts = $bt_settings.clone();
                    async move |_input: EmptyInput, _cx| {
                        $crate::check_tool_enabled!(bts, "skills_list");
                        $crate::builtins::misc::skills_list_json()
                    }
                },
                sacp::tool_fn!(),
            )
            .tool_fn(
                "skill_view",
                "Load a skill's SKILL.md or a supporting file from brain/skills.",
                {
                    let bts = $bt_settings.clone();
                    async move |input: SkillViewInput, _cx| {
                        $crate::check_tool_enabled!(bts, "skill_view");
                        $crate::builtins::misc::skill_view_json(&input.name, input.file_path.as_deref())
                    }
                },
                sacp::tool_fn!(),
            )
            .tool_fn(
                "mcp_app_demo",
                "MCP Apps UI demo: returns an interactive HTML widget rendered inline in the chat. Use this to test MCP Apps rendering.",
                {
                    async move |_input: EmptyInput, _cx| {
                        Ok::<String, sacp::Error>(serde_json::json!({
                            "mcp_app": true,
                            "title": "MCP App Demo",
                            "html": r#"<!DOCTYPE html>
<html><head>
<meta charset="UTF-8">
<meta name="color-scheme" content="light dark">
<style>
*{box-sizing:border-box;margin:0;padding:0}
body{font-family:system-ui,-apple-system,sans-serif;padding:20px;background:light-dark(#f8fafc,#1a1a2e);color:light-dark(#1e293b,#e2e8f0)}
.card{background:light-dark(#fff,#16213e);border-radius:12px;padding:20px;box-shadow:0 4px 12px rgba(0,0,0,0.15);max-width:480px}
h2{font-size:18px;margin-bottom:8px;background:linear-gradient(135deg,#6366f1,#8b5cf6);-webkit-background-clip:text;-webkit-text-fill-color:transparent}
.badge{display:inline-block;padding:2px 8px;border-radius:99px;font-size:11px;font-weight:600;background:#6366f1;color:#fff;margin-bottom:8px}
</style>
</head><body>
<div class="card">
  <span class="badge">MCP APPS SDK</span>
  <h2>Interactive Widget Demo</h2>
  <p>MCP Apps JSON-RPC over postMessage</p>
</div>
</body></html>"#
                        }).to_string())
                    }
                },
                sacp::tool_fn!(),
            )
            .resource_fn(
                "ilhae://memory/all",
                "All Memory",
                "Complete agent context from vault: global memory (SYSTEM/IDENTITY/SOUL/USER) + daily logs + project",
                "text/markdown",
                {
                    move |_uri: String| {
                        async move {
                            $crate::memory_provider::read_section("all")
                                .map_err(|e| sacp::Error::internal_error().data(e))
                        }
                    }
                },
            )
            .resource_fn(
                "ilhae://schedules",
                "All Tasks",
                "Complete task list including todos, scheduled, cron, and missions",
                "application/json",
                {
                    let brain = $brain_service.clone();
                    move |_uri: String| {
                        let brain = brain.clone();
                        async move {
                            let list = brain.schedule_list();
                            let text = serde_json::to_string_pretty(&list)
                                .unwrap_or("[]".to_string());
                            Ok::<String, sacp::Error>(text)
                        }
                    }
                },
            )
            .resource_fn(
                "ilhae://sessions",
                "All Sessions",
                "List of all chat sessions with titles and timestamps",
                "application/json",
                {
                    let brain = $brain_service.clone();
                    move |_uri: String| {
                        let brain = brain.clone();
                        async move {
                            match brain.session_list(None) {
                                Ok(sessions) => {
                                    let text = serde_json::to_string_pretty(&sessions)
                                        .unwrap_or("[]".to_string());
                                    Ok::<String, sacp::Error>(text)
                                }
                                Err(e) => Err(sacp::Error::internal_error().data(e.to_string())),
                            }
                        }
                    }
                },
            )
            .resource_template_fn(
                "ilhae://memory/{section}",
                "Memory Section",
                "Read a specific memory section from vault: system, identity, soul, user, daily, or project",
                "text/markdown",
                {
                    move |uri: String| {
                        async move {
                            let section = uri.rsplit('/').next().unwrap_or("all");
                            $crate::memory_provider::read_section(section)
                                .map_err(|e| sacp::Error::invalid_request().data(e))
                        }
                    }
                },
            )
            .resource_template_fn(
                "ilhae://session/{id}",
                "Session Messages",
                "Load all messages for a specific session by ID",
                "application/json",
                {
                    let brain = $brain_service.clone();
                    move |uri: String| {
                        let brain = brain.clone();
                        async move {
                            let id = uri.rsplit('/').next().unwrap_or("");
                            match brain.session_load(id) {
                                Ok(messages) => {
                                    let text = serde_json::to_string_pretty(&messages)
                                        .unwrap_or("[]".to_string());
                                    Ok::<String, sacp::Error>(text)
                                }
                                Err(e) => Err(sacp::Error::internal_error().data(e.to_string())),
                            }
                        }
                    }
                },
            )
            .resource_template_fn(
                "ilhae://task/{id}",
                "Task Detail",
                "Get details for a specific task by ID",
                "application/json",
                {
                    let brain = $brain_service.clone();
                    move |uri: String| {
                        let brain = brain.clone();
                        async move {
                            let id = uri.rsplit('/').next().unwrap_or("");
                            let all = brain.schedule_list();
                            if let Some(task) = all.iter().find(|t| t.id == id) {
                                let text = serde_json::to_string_pretty(task)
                                    .unwrap_or("{}".to_string());
                                Ok::<String, sacp::Error>(text)
                            } else {
                                Err(sacp::Error::invalid_request().data(format!("Task '{}' not found", id)))
                            }
                        }
                    }
                },
            )
            .prompt_fn(
                "daily-report",
                "Generate a daily report summarizing today's schedules, sessions, and memory updates",
                &[],
                {
                    move |_args: Option<serde_json::Map<String, serde_json::Value>>| {
                        async move {
                            Ok::<String, sacp::Error>(
                                "Generate a daily report for today. Include:\n\
                                 1. Summary of schedules completed and in-progress\n\
                                 2. Key decisions made\n\
                                 3. Notable memory updates\n\
                                 4. Tomorrow's priorities\n\
                                 \n\
                                 Use the task_list and memory_read tools to gather current data.".to_string()
                            )
                        }
                    }
                },
            )
            .prompt_fn(
                "code-review",
                "Request a thorough code review for specified files or changes",
                &[("target", "File path, PR number, or description of changes to review", true)],
                {
                    move |args: Option<serde_json::Map<String, serde_json::Value>>| {
                        async move {
                            let target = args
                                .and_then(|a| a.get("target").and_then(|v| v.as_str().map(String::from)))
                                .unwrap_or_else(|| "the recent changes".to_string());
                            Ok::<String, sacp::Error>(format!(
                                "Please perform a thorough code review of: {}\n\
                                 \n\
                                 Focus on:\n\
                                 1. Correctness and potential bugs\n\
                                 2. Performance implications\n\
                                 3. Security concerns\n\
                                 4. Code style and readability\n\
                                 5. Missing error handling\n\
                                 6. Test coverage gaps",
                                target
                            ))
                        }
                    }
                },
            )
            .prompt_fn(
                "brainstorm",
                "Enter brainstorming mode for a topic — generates creative, exploratory prompts",
                &[("topic", "The topic or problem to brainstorm about", true)],
                {
                    move |args: Option<serde_json::Map<String, serde_json::Value>>| {
                        async move {
                            let topic = args
                                .and_then(|a| a.get("topic").and_then(|v| v.as_str().map(String::from)))
                                .unwrap_or_else(|| "the given topic".to_string());
                            Ok::<String, sacp::Error>(format!(
                                "Let's brainstorm about: {}\n\
                                 \n\
                                 Rules for this brainstorming session:\n\
                                 - Generate at least 5 diverse approaches\n\
                                 - Include unconventional or creative solutions\n\
                                 - Consider trade-offs for each approach\n\
                                 - Think about both short-term and long-term implications\n\
                                 - Reference relevant memory/context if available via memory_read",
                                topic
                            ))
                        }
                    }
                },
            )
    }};
}

fn skills_root() -> std::path::PathBuf {
    crate::config::get_active_vault_dir().join("skills")
}

pub fn advisor_context_summary_from_value(
    value: &serde_json::Value,
    max_messages: usize,
) -> String {
    let Some(items) = value.as_array() else {
        return String::new();
    };

    let mut remaining = max_messages;
    let mut skip_latest_assistant = true;
    let mut lines = Vec::new();
    for item in items.iter().rev() {
        let role = item
            .get("role")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .trim();
        if role.is_empty() || role.eq_ignore_ascii_case("system") {
            continue;
        }
        if skip_latest_assistant && role.eq_ignore_ascii_case("assistant") {
            skip_latest_assistant = false;
            continue;
        }
        skip_latest_assistant = false;

        let content = item
            .get("content")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .or_else(|| {
                item.pointer("/message/content")
                    .and_then(serde_json::Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
            });

        let Some(content) = content else {
            continue;
        };

        lines.push(format!("{}: {}", role.to_ascii_lowercase(), content));
        remaining = remaining.saturating_sub(1);
        if remaining == 0 {
            break;
        }
    }

    lines.reverse();
    lines.join("\n")
}

fn parse_skill_frontmatter(content: &str) -> (Option<String>, Option<String>) {
    if !content.starts_with("---\n") {
        return (None, None);
    }
    let Some(end_idx) = content[4..].find("\n---\n") else {
        return (None, None);
    };
    let yaml_str = &content[4..4 + end_idx];
    let mut name = None;
    let mut description = None;
    for line in yaml_str.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some(value) = trimmed.strip_prefix("name:") {
            let value = value.trim().trim_matches('"').trim_matches('\'');
            if !value.is_empty() {
                name = Some(value.to_string());
            }
        }
        if let Some(value) = trimmed.strip_prefix("description:") {
            let value = value.trim().trim_matches('"').trim_matches('\'');
            if !value.is_empty() {
                description = Some(value.to_string());
            }
        }
    }
    (name, description)
}

fn resolve_skill_dir(name: &str) -> Result<std::path::PathBuf, sacp::Error> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(sacp::Error::invalid_request().data("skill name is empty"));
    }
    let rel = std::path::Path::new(trimmed);
    if rel.is_absolute()
        || rel
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(sacp::Error::invalid_request().data("invalid skill path"));
    }
    let dir = skills_root().join(rel);
    if !dir.is_dir() {
        return Err(sacp::Error::invalid_request().data(format!("skill '{}' not found", trimmed)));
    }
    Ok(dir)
}

pub fn skills_list_json() -> Result<String, sacp::Error> {
    let root = skills_root();
    let _ = std::fs::create_dir_all(&root);

    let mut skills = Vec::<serde_json::Value>::new();
    for entry in walkdir::WalkDir::new(&root)
        .min_depth(1)
        .max_depth(4)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_dir())
    {
        let skill_dir = entry.path();
        let skill_file = skill_dir.join("SKILL.md");
        if !skill_file.is_file() {
            continue;
        }

        let rel = skill_dir
            .strip_prefix(&root)
            .unwrap_or(skill_dir)
            .to_string_lossy()
            .to_string();
        let content = match std::fs::read_to_string(&skill_file) {
            Ok(content) => content,
            Err(_) => continue,
        };
        let (frontmatter_name, description) = parse_skill_frontmatter(&content);
        let display_name = frontmatter_name.unwrap_or_else(|| rel.clone());
        let source = if rel.starts_with("3rdparty/") {
            "3rdparty"
        } else {
            "brain"
        };
        skills.push(serde_json::json!({
            "name": rel,
            "display_name": display_name,
            "description": description.unwrap_or_else(|| "Brain skill".to_string()),
            "source": source,
        }));
    }

    skills.sort_by(|a, b| {
        a.get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .cmp(b.get("name").and_then(|v| v.as_str()).unwrap_or(""))
    });

    Ok(serde_json::to_string_pretty(&serde_json::json!({
        "skills": skills,
        "count": skills.len(),
        "hint": "Use skill_view(name) to load full SKILL.md content or skill_view(name, file_path) for supporting files."
    }))
    .unwrap_or_else(|_| "{\"skills\":[]}".to_string()))
}

pub fn skill_view_json(name: &str, file_path: Option<&str>) -> Result<String, sacp::Error> {
    let skill_dir = resolve_skill_dir(name)?;
    let target_rel = file_path.unwrap_or("SKILL.md").trim();
    let rel_path = std::path::Path::new(target_rel);
    if rel_path.is_absolute()
        || rel_path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(sacp::Error::invalid_request().data("invalid skill file path"));
    }

    let target = skill_dir.join(rel_path);
    if !target.is_file() {
        return Err(
            sacp::Error::invalid_request().data(format!("skill file '{}' not found", target_rel))
        );
    }

    let content = std::fs::read_to_string(&target)
        .map_err(|e| sacp::Error::internal_error().data(e.to_string()))?;

    let mut linked_files = Vec::<String>::new();
    for entry in walkdir::WalkDir::new(&skill_dir)
        .min_depth(1)
        .max_depth(4)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
    {
        let path = entry.path();
        if path == target || path.file_name().and_then(|n| n.to_str()) == Some("SKILL.md") {
            continue;
        }
        if let Ok(rel) = path.strip_prefix(&skill_dir) {
            linked_files.push(rel.to_string_lossy().to_string());
        }
    }
    linked_files.sort();

    Ok(serde_json::to_string_pretty(&serde_json::json!({
        "name": name,
        "file_path": target_rel,
        "content": content,
        "linked_files": linked_files,
    }))
    .unwrap_or_else(|_| "{}".to_string()))
}

#[cfg(test)]
mod tests {
    #[test]
    fn advisor_context_summary_keeps_recent_relevant_messages() {
        let messages = serde_json::json!([
            { "role": "system", "content": "system prompt" },
            { "role": "user", "content": "first user ask" },
            { "role": "assistant", "content": "first reply" },
            { "role": "user", "content": "second user ask" },
            { "role": "assistant", "content": "second reply" }
        ]);

        let summary = super::advisor_context_summary_from_value(&messages, 3);

        assert!(summary.contains("user: first user ask"));
        assert!(summary.contains("assistant: first reply"));
        assert!(summary.contains("user: second user ask"));
        assert!(!summary.contains("system prompt"));
        assert!(!summary.contains("assistant: second reply"));
    }
}
