//! Config option builders and dynamic instruction generators.
//!
//! Extracted from `helpers.rs` — contains all ACP config‐option synthesis
//! (Codex/Gemini), codex profile management, and dynamic instructions
//! (memory system, team mode prompts).

use agent_client_protocol_schema::NewSessionResponse;
use serde_json::json;
use tracing::info;

use crate::helpers::ILHAE_DIR_NAME;
use crate::settings_store;

// ─── Dynamic instructions ────────────────────────────────────────────────

/// Build dynamic instructions based on current plugin settings.
pub fn build_dynamic_instructions(settings: &settings_store::Settings) -> String {
    let mut parts: Vec<&str> = Vec::new();
    let mut owned_parts: Vec<String> = Vec::new();

    // ── Tier 1: Core identity auto-injection ──
    // Uses shared memory_provider to read ~/ilhae/brain/memory/global/*.md
    // These are always injected (~4.5KB, ~1200 tokens) so the LLM knows who it is.
    for (section_name, content) in crate::memory_provider::read_all_global() {
        owned_parts.push(format!(
            "<agent_memory section=\"{}\">\n{}\n</agent_memory>",
            section_name, content
        ));
    }

    // ── Tier 2: Resource index ──
    // Tell the LLM what MCP resources are available for on-demand access via tools.
    owned_parts.push(
        "<available_resources>\n\
         The following data is available on-demand via tools (not pre-loaded to save context):\n\
         - memory_read(section: \"daily\") — today's daily log\n\
         - memory_read(section: \"project\") — project-specific context\n\
         - skills_list / skill_view — inspect available skills on demand\n\
         - task_list — current schedules and missions\n\
         - session_list — past session history\n\
         - session_search(query, limit) — search past sessions by title/message text\n\
         Use these tools when you need specific data rather than guessing.\n\
         </available_resources>"
            .to_string(),
    );

    // ── Memory system: always active ──
    parts.push(concat!(
        "<dynamic_instructions>\n",
        "## 장기 기억 시스템 (Memory)\n",
        "이 에이전트에는 SQLite 기반 장기 기억 시스템이 내장되어 있습니다.\n",
        "사용자가 \"기억해\", \"remember\", \"저장해\" 등을 요청하면 반드시 아래 내장 도구를 사용하세요:\n",
        "- memory_store: 새 기억 저장 (text, source)\n",
        "- memory_search: BM25 검색으로 관련 기억 조회\n",
        "- memory_list: 저장된 기억 목록 조회\n",
        "- memory_forget: 기억 삭제\n",
        "- memory_stats: 기억 통계 확인\n\n",
        "⚠️ 중요: GEMINI.md 파일이나 다른 파일을 직접 수정하여 기억을 저장하지 마세요.\n",
        "반드시 memory_store 도구를 통해 저장하세요. 이 도구는 자동 승인되므로 권한 요청 없이 바로 실행됩니다.\n",
        "사용자가 이전 대화 내용이나 선호사항을 물어보면 memory_search로 먼저 조회하세요.\n",
        "과거 대화/작업 이력을 찾아야 할 때는 session_search를 사용하세요.\n",
        "스킬이 필요할 때는 skills_list로 목록을 보고, skill_view로 필요한 스킬만 로드하세요.\n",
        "</dynamic_instructions>",
    ));

    parts.push(concat!(
        "<dynamic_instructions>\n",
        "## Brain / Knowledge 라우팅 규칙\n",
        "사용자가 knowledge workspace, raw/wiki/output/index, health report, broken link, compile, lint, ingest, query 같은 표현을 쓰면 shell 탐색보다 `kb_*` 도구를 우선하세요.\n",
        "권장 수리 루프: `kb_workspace_list` → `kb_lint` → 필요 시 관련 markdown만 확인 → `kb_file_back` 또는 `kb_compile` → `kb_lint` 재실행으로 issue count를 확인합니다.\n",
        "`brain_knowledge_ops`는 cross-session knowledge graph 질의용 보조 도구입니다. 유효한 action은 `search`, `unified_search`, `diff`, `batch_delete`, `link_add`, `link_remove`, `link_list` 뿐입니다. `action = list`는 잘못된 호출이므로 절대 사용하지 마세요.\n",
        "`list_mcp_resources`와 `read_mcp_resource`는 MCP 리소스를 이미 알고 있을 때만 씁니다. repository 파일 탐색이나 워크스페이스 위치 찾기에 쓰지 마세요.\n",
        "`list_mcp_resources`는 `cursor`를 사용할 때 반드시 같은 `server`를 함께 지정해야 합니다.\n",
        "`read_mcp_resource`는 이전 `list_mcp_resources` 결과에서 받은 정확한 `server`와 `uri`에만 사용하세요. `server = default` 같은 추측값을 쓰지 마세요.\n",
        "knowledge workspace 수리 중에는 `find /workspace`, `ls -R /workspace` 같은 광범위한 스캔을 피하고, 먼저 전용 도구로 대상 workspace를 좁히세요.\n",
        "</dynamic_instructions>",
    ));

    // ── Team mode: instruct leader to delegate via tool_calls ──
    if settings.agent.team_mode {
        // Load team config to get actual agent role names
        let ilhae_dir = dirs::home_dir()
            .map(|h| h.join(ILHAE_DIR_NAME))
            .unwrap_or_default();
        if let Some(team_cfg) = crate::context_proxy::load_team_runtime_config(&ilhae_dir) {
            let tool_names: Vec<String> = team_cfg
                .agents
                .iter()
                .filter(|a| !a.is_main)
                .map(|a| a.role.to_lowercase())
                .collect();
            if !tool_names.is_empty() {
                let tool_list = tool_names.join(", ");
                let team_prompt = if team_cfg.team_prompt.trim().is_empty() {
                    String::new()
                } else {
                    format!("\n\nTeam context:\n{}", team_cfg.team_prompt.trim())
                };
                parts.push(&"<dynamic_instructions>");
                // We need to use a String here, so push it separately
                let team_instr = format!(
                    concat!(
                        "## Team Leader Mode\n",
                        "You are the team leader. Your team members are registered as function-calling tools.\n\n",
                        "Available team tools: [{tools}]\n",
                        "Each tool accepts a `query` parameter (string) describing the task to delegate.\n\n",
                        "### Mandatory Behavior\n",
                        "1. You MUST respond with tool_call function calls. NEVER respond with plain text that describes actions.\n",
                        "2. Analyze the user request → decide which tool(s) to call → emit tool_call(s) immediately.\n",
                        "3. If the task can be parallelized, call multiple tools simultaneously.\n",
                        "4. After receiving tool results, synthesize them into a final answer for the user.\n\n",
                        "### Forbidden\n",
                        "- Writing \"I will delegate to...\" instead of actually calling tools.\n",
                        "- Any response without at least one tool_call when work needs to be done.\n",
                        "{team_prompt}\n",
                        "</dynamic_instructions>",
                    ),
                    tools = tool_list,
                    team_prompt = team_prompt,
                );
                let mut all = owned_parts.iter().map(|s| s.as_str()).collect::<Vec<_>>();
                all.extend_from_slice(&parts);
                return [all.join("\n"), team_instr].join("\n");
            }
        }
    }

    let mut all = owned_parts.iter().map(|s| s.as_str()).collect::<Vec<_>>();
    all.extend_from_slice(&parts);
    all.join("\n")
}

#[cfg(test)]
mod tests {
    use super::build_dynamic_instructions;

    #[test]
    fn build_dynamic_instructions_includes_kb_routing_guardrails() {
        let settings = crate::settings_store::Settings::default();
        let instructions = build_dynamic_instructions(&settings);

        assert!(instructions.contains("Brain / Knowledge 라우팅 규칙"));
        assert!(instructions.contains("`kb_workspace_list`"));
        assert!(instructions.contains("`kb_lint`"));
        assert!(instructions.contains("`kb_file_back`"));
        assert!(instructions.contains("`action = list`"));
        assert!(instructions.contains("`server = default`"));
    }
}

// ─── Codex capability injection ──────────────────────────────────────────

/// Read `~/.codex/config.toml` and synthesize ACP `config_options` JSON.
pub fn build_codex_config_options() -> Vec<serde_json::Value> {
    let config_path = dirs::home_dir()
        .map(|h| h.join(".codex/config.toml"))
        .unwrap_or_default();

    let config_str = match std::fs::read_to_string(&config_path) {
        Ok(s) => s,
        Err(e) => {
            info!("Could not read Codex config.toml: {}", e);
            return vec![];
        }
    };

    let config: toml::Value = match config_str.parse() {
        Ok(v) => v,
        Err(e) => {
            info!("Failed to parse Codex config.toml: {}", e);
            return vec![];
        }
    };

    let mut options = Vec::new();

    // ── sandbox_mode config option ──
    let current_sandbox = config
        .get("sandbox_mode")
        .and_then(|v| v.as_str())
        .unwrap_or("auto-edit");
    options.push(json!({
        "id": "sandbox_mode",
        "name": "Sandbox Mode",
        "description": "Controls how Codex handles file writes and command execution",
        "type": "select",
        "category": "safety",
        "currentValue": current_sandbox,
        "options": [
            { "value": "suggest", "name": "Suggest", "description": "Read-only — suggests changes without applying" },
            { "value": "auto-edit", "name": "Auto Edit", "description": "Applies file edits; asks before commands" },
            { "value": "danger-full-access", "name": "Full Access", "description": "Applies all edits and runs commands automatically" },
        ]
    }));

    // ── approval_policy config option ──
    let current_approval = config
        .get("approval_policy")
        .and_then(|v| v.as_str())
        .unwrap_or("on-failure");
    options.push(json!({
        "id": "approval_policy",
        "name": "Approval Policy",
        "description": "When to ask for approval before running tools",
        "type": "select",
        "category": "safety",
        "currentValue": current_approval,
        "options": [
            { "value": "on-failure", "name": "On Failure", "description": "Auto-approve unless previous attempt failed" },
            { "value": "unless-allow-listed", "name": "Unless Allow-listed", "description": "Ask unless tool is in the allow list" },
            { "value": "never", "name": "Never", "description": "Never ask for approval (YOLO)" },
        ]
    }));

    // ── model selection from profiles ──
    let current_model = config
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("gpt-5");

    let mut model_values: Vec<serde_json::Value> = vec![];
    if let Some(profiles) = config.get("profiles").and_then(|p| p.as_table()) {
        for (name, profile) in profiles {
            let model = profile
                .get("model")
                .and_then(|v| v.as_str())
                .unwrap_or(name);
            model_values.push(json!({
                "value": name,
                "name": model,
                "description": null,
            }));
        }
    }
    if model_values.is_empty() {
        model_values.push(json!({
            "value": current_model,
            "name": current_model,
            "description": null,
        }));
    }

    let current_profile = config
        .get("profile")
        .and_then(|v| v.as_str())
        .unwrap_or(current_model);
    options.push(json!({
        "id": "model",
        "name": "Model",
        "description": "Active model profile for Codex",
        "type": "select",
        "category": "model",
        "currentValue": current_profile,
        "options": model_values,
    }));

    options
}

/// Build configOptions for Gemini CLI by reading `~/.gemini/settings.json`
/// for the current model and providing the full VALID_GEMINI_MODELS list.
/// Mirrors the pattern of `build_codex_config_options`.
pub fn build_gemini_config_options() -> Vec<serde_json::Value> {
    // Read current model from ~/.gemini/settings.json
    let current_model = dirs::home_dir()
        .map(|h| h.join(".gemini/settings.json"))
        .and_then(|p| std::fs::read_to_string(&p).ok())
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v.get("model")?.get("name")?.as_str().map(|s| s.to_string()))
        .unwrap_or_else(|| "gemini-2.5-flash".to_string());

    // Full model list matching Gemini CLI's VALID_GEMINI_MODELS
    let models = [
        ("gemini-3.1-pro-preview", "Gemini 3.1 Pro Preview"),
        ("gemini-3-pro-preview", "Gemini 3 Pro Preview"),
        ("gemini-3-flash-preview", "Gemini 3 Flash Preview"),
        ("gemini-2.5-pro", "Gemini 2.5 Pro"),
        ("gemini-2.5-flash", "Gemini 2.5 Flash"),
        ("gemini-2.5-flash-lite", "Gemini 2.5 Flash Lite"),
    ];
    let model_values: Vec<serde_json::Value> = models
        .iter()
        .map(|(id, name)| {
            json!({
                "value": id,
                "name": name,
                "description": null,
            })
        })
        .collect();
    vec![json!({
        "id": "model",
        "name": "Model",
        "description": "Active model for Gemini CLI",
        "type": "select",
        "category": "model",
        "currentValue": current_model,
        "options": model_values,
    })]
}

pub fn read_codex_runtime_options() -> (String, String) {
    let config_path = dirs::home_dir()
        .map(|h| h.join(".codex/config.toml"))
        .unwrap_or_default();

    let config_str = match std::fs::read_to_string(&config_path) {
        Ok(s) => s,
        Err(_) => {
            return ("auto-edit".to_string(), "on-failure".to_string());
        }
    };

    let config = match config_str.parse::<toml::Value>() {
        Ok(v) => v,
        Err(_) => {
            return ("auto-edit".to_string(), "on-failure".to_string());
        }
    };

    let sandbox_mode = config
        .get("sandbox_mode")
        .and_then(|v| v.as_str())
        .unwrap_or("auto-edit")
        .to_string();

    let approval_policy = config
        .get("approval_policy")
        .and_then(|v| v.as_str())
        .unwrap_or("on-failure")
        .to_string();

    (sandbox_mode, approval_policy)
}

pub fn write_codex_runtime_option(key: &str, value: &str) -> Result<(), String> {
    let config_path = dirs::home_dir()
        .map(|h| h.join(".codex/config.toml"))
        .unwrap_or_default();
    let config_str = std::fs::read_to_string(&config_path).map_err(|e| e.to_string())?;
    let mut doc = config_str
        .parse::<toml_edit::DocumentMut>()
        .map_err(|e| e.to_string())?;
    doc[key] = toml_edit::value(value);
    std::fs::write(&config_path, doc.to_string()).map_err(|e| e.to_string())
}

/// Apply a Codex profile's settings to the global config.toml.
pub fn apply_codex_profile_to_config(profile_name: &str) -> Result<(), String> {
    let config_path = dirs::home_dir()
        .map(|h| h.join(".codex/config.toml"))
        .ok_or("Could not determine home directory")?;

    let config_str = std::fs::read_to_string(&config_path)
        .map_err(|e| format!("Could not read config.toml: {}", e))?;

    let mut doc = config_str
        .parse::<toml_edit::DocumentMut>()
        .map_err(|e| format!("Could not parse config.toml: {}", e))?;

    let profile_model = doc
        .get("profiles")
        .and_then(|p| p.get(profile_name))
        .and_then(|prof| prof.get("model"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let profile_reasoning_summary = doc
        .get("profiles")
        .and_then(|p| p.get(profile_name))
        .and_then(|prof| prof.get("model_reasoning_summary"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    doc["profile"] = toml_edit::value(profile_name);
    if let Some(model) = &profile_model {
        doc["model"] = toml_edit::value(model.as_str());
    }

    if let Some(summary) = &profile_reasoning_summary {
        doc["model_reasoning_summary"] = toml_edit::value(summary.as_str());
    } else {
        doc.remove("model_reasoning_summary");
    }

    std::fs::write(&config_path, doc.to_string())
        .map_err(|e| format!("Could not write config.toml: {}", e))?;

    info!(
        "Updated Codex config.toml: profile={}, model={:?}, reasoning_summary={:?}",
        profile_name, profile_model, profile_reasoning_summary
    );

    Ok(())
}

/// Enrich a NewSessionResponse with ACP-standard `configOptions`.
///
/// For **Codex**: reads model/sandbox/approval options from `~/.codex/config.toml`.
/// For **Gemini/others**: converts `models.availableModels` into a configOption
/// (category: "model") so ConfigOptionsPanel can render model selection.
pub fn enrich_response_with_config_options(
    response: NewSessionResponse,
    agent_id: &str,
) -> NewSessionResponse {
    if agent_id == "codex" {
        // Codex: inject from config.toml
        let options = build_codex_config_options();
        if options.is_empty() {
            return response;
        }
        let mut json = match serde_json::to_value(&response) {
            Ok(v) => v,
            Err(_) => return response,
        };
        let has_config = json
            .get("configOptions")
            .or_else(|| json.get("config_options"))
            .map(|v| v.is_array() && !v.as_array().unwrap().is_empty())
            .unwrap_or(false);
        if has_config {
            return response;
        }
        info!(
            "Injecting {} Codex config_options into NewSessionResponse",
            options.len()
        );
        json["configOptions"] = json!(options);
        return serde_json::from_value(json).unwrap_or(response);
    }

    // Non-Codex (Gemini etc.): convert models.availableModels → configOptions
    let mut json = match serde_json::to_value(&response) {
        Ok(v) => v,
        Err(_) => return response,
    };

    // Debug: log what the agent actually returned
    if let Some(obj) = json.as_object() {
        let keys: Vec<&str> = obj.keys().map(|k| k.as_str()).collect();
        info!("[EnrichDebug] agent={} response keys: {:?}", agent_id, keys);
        if let Some(models) = obj.get("models") {
            info!("[EnrichDebug] models field: {}", models);
        } else {
            info!("[EnrichDebug] NO models field in response");
        }
        if let Some(co) = obj
            .get("configOptions")
            .or_else(|| obj.get("config_options"))
        {
            info!(
                "[EnrichDebug] configOptions field present: {} items",
                co.as_array().map(|a| a.len()).unwrap_or(0)
            );
        } else {
            info!("[EnrichDebug] NO configOptions in response");
        }
    }

    // Already has configOptions? Pass through.
    let has_config = json
        .get("configOptions")
        .or_else(|| json.get("config_options"))
        .map(|v| v.is_array() && !v.as_array().unwrap().is_empty())
        .unwrap_or(false);
    if has_config {
        return response;
    }

    // Extract models.availableModels and models.currentModelId
    let models = json.get("models");
    let available = models
        .and_then(|m| m.get("availableModels"))
        .and_then(|a| a.as_array());
    let current_model_id = models
        .and_then(|m| m.get("currentModelId"))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if let Some(model_list) = available {
        if !model_list.is_empty() {
            let model_values: Vec<serde_json::Value> = model_list
                .iter()
                .filter_map(|m| {
                    let model_id = m.get("modelId").and_then(|v| v.as_str())?;
                    let name = m.get("name").and_then(|v| v.as_str()).unwrap_or(model_id);
                    Some(json!({
                        "value": model_id,
                        "name": name,
                        "description": null,
                    }))
                })
                .collect();

            if !model_values.is_empty() {
                let config_option = json!({
                    "id": "model",
                    "name": "Model",
                    "description": "Active model",
                    "type": "select",
                    "category": "model",
                    "currentValue": current_model_id,
                    "options": model_values,
                });
                info!(
                    "Converting {} availableModels → configOption for {}",
                    model_values.len(),
                    agent_id
                );
                json["configOptions"] = json!([config_option]);
                return serde_json::from_value(json).unwrap_or(response);
            }
        }
    }

    response
}
