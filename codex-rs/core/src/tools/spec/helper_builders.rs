use std::collections::BTreeMap;

use codex_model_provider_info::provider_uses_json_function_tools;
use codex_tools::ResponsesApiNamespaceTool;
use codex_tools::ResponsesApiTool;
use codex_tools::ToolSpec;
use codex_tools::create_apply_patch_json_tool;

use super::JsonSchema;

pub fn create_tools_json_for_responses_api_with_provider(
    tools: &[ToolSpec],
    provider_id: &str,
) -> codex_protocol::error::Result<Vec<serde_json::Value>> {
    if provider_uses_json_function_tools(provider_id) {
        return create_tools_json_for_json_function_provider(tools);
    }

    codex_tools::create_tools_json_for_responses_api(tools)
        .map_err(|e| codex_protocol::error::CodexErr::Fatal(e.to_string()))
}

pub(super) fn create_self_check_tool() -> ToolSpec {
    let mut properties = BTreeMap::new();
    properties.insert(
        "status".to_string(),
        codex_tools::JsonSchema::string(Some(
            "The evaluation status ('passed', 'failed', 'blocked').".to_string(),
        )),
    );
    properties.insert(
        "errors_encountered".to_string(),
        codex_tools::JsonSchema::array(
            codex_tools::JsonSchema::string(None),
            Some("Any errors or failures you encountered during this step.".to_string()),
        ),
    );
    properties.insert(
        "mitigation_plan".to_string(),
        codex_tools::JsonSchema::string(Some(
            "Proposed strategy to recover, retry, or ask for user help.".to_string(),
        )),
    );

    ToolSpec::Function(ResponsesApiTool {
        name: "self_check".to_string(),
        description: "Run an autonomous self-check to evaluate success, mitigate errors, and trigger tool-call retries. This harness provides autonomous error recovery feedback.".to_string(),
        strict: false,
        defer_loading: None,
        parameters: codex_tools::JsonSchema::object(
            properties,
            Some(vec!["status".to_string()]),
            Some(codex_tools::AdditionalProperties::Boolean(false)),
        ),
        output_schema: None,
    })
}

pub(super) fn create_advisor_request_tool() -> ToolSpec {
    let mut properties = BTreeMap::new();
    properties.insert(
        "question".to_string(),
        codex_tools::JsonSchema::string(Some(
            "Focused question for the high-reasoning advisor. Ask for strategy, trade-offs, or a plan."
                .to_string(),
        )),
    );
    properties.insert(
        "context".to_string(),
        codex_tools::JsonSchema::string(Some(
            "Optional executor context summarizing the immediate uncertainty or local findings."
                .to_string(),
        )),
    );

    ToolSpec::Function(ResponsesApiTool {
        name: "advisor_request".to_string(),
        description: "On-demand high-reasoning advisor. Use sparingly when the task needs deeper planning, ambiguity resolution, or strategic trade-off analysis before execution.".to_string(),
        strict: false,
        defer_loading: None,
        parameters: codex_tools::JsonSchema::object(
            properties,
            Some(vec!["question".to_string()]),
            Some(codex_tools::AdditionalProperties::Boolean(false)),
        ),
        output_schema: None,
    })
}

fn create_tools_json_for_json_function_provider(
    tools: &[ToolSpec],
) -> codex_protocol::error::Result<Vec<serde_json::Value>> {
    let mut tools_json = Vec::new();

    for tool in tools {
        for tool in json_function_provider_tool_specs(tool) {
            let json = serde_json::to_value(tool)?;
            tools_json.push(json);
        }
    }

    Ok(tools_json)
}

fn json_function_provider_tool_specs(tool: &ToolSpec) -> Vec<ToolSpec> {
    match tool {
        ToolSpec::Function(tool) => vec![ToolSpec::Function(tool.clone())],
        ToolSpec::Namespace(namespace) => namespace
            .tools
            .iter()
            .map(|tool| match tool {
                ResponsesApiNamespaceTool::Function(tool) => {
                    let mut flattened_tool = tool.clone();
                    flattened_tool.name = format!("{}{}", namespace.name, flattened_tool.name);
                    ToolSpec::Function(flattened_tool)
                }
            })
            .collect(),
        ToolSpec::LocalShell {} => vec![create_local_shell_json_tool()],
        ToolSpec::Freeform(tool) if tool.name == "apply_patch" => {
            vec![create_apply_patch_json_tool()]
        }
        ToolSpec::Freeform(tool) if tool.name == "js_repl" => vec![create_js_repl_json_tool()],
        ToolSpec::ToolSearch {
            description,
            parameters,
            ..
        } => vec![create_tool_search_json_tool(
            description.clone(),
            parameters.clone(),
        )],
        ToolSpec::ImageGeneration { .. } | ToolSpec::WebSearch { .. } | ToolSpec::Freeform(_) => {
            Vec::new()
        }
    }
}

fn create_tool_search_json_tool(description: String, parameters: JsonSchema) -> ToolSpec {
    ToolSpec::Function(ResponsesApiTool {
        name: "tool_search".to_string(),
        description,
        strict: false,
        defer_loading: None,
        parameters,
        output_schema: None,
    })
}

fn create_local_shell_json_tool() -> ToolSpec {
    let properties = BTreeMap::from([
        (
            "command".to_string(),
            JsonSchema::array(
                JsonSchema::string(None),
                Some("The command to execute.".to_string()),
            ),
        ),
        (
            "workdir".to_string(),
            JsonSchema::string(Some(
                "The working directory to execute the command in.".to_string(),
            )),
        ),
        (
            "timeout_ms".to_string(),
            JsonSchema::number(Some(
                "The timeout for the command in milliseconds.".to_string(),
            )),
        ),
    ]);

    ToolSpec::Function(ResponsesApiTool {
        name: "local_shell".to_string(),
        description:
            "Runs a local shell command and returns its output. The command is passed directly to execvp(). Always set `workdir` when possible."
                .to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(
            properties,
            Some(vec!["command".to_string()]),
            Some(codex_tools::AdditionalProperties::Boolean(false)),
        ),
        output_schema: None,
    })
}

fn create_js_repl_json_tool() -> ToolSpec {
    let properties = BTreeMap::from([
        (
            "code".to_string(),
            codex_tools::JsonSchema::string(Some(
                "Raw JavaScript source to execute in the persistent Node kernel.".to_string(),
            )),
        ),
        (
            "timeout_ms".to_string(),
            codex_tools::JsonSchema::number(Some(
                "Optional timeout override in milliseconds for this execution.".to_string(),
            )),
        ),
    ]);

    ToolSpec::Function(ResponsesApiTool {
        name: "js_repl".to_string(),
        description:
            "Runs JavaScript in a persistent Node kernel with top-level await. Send JSON with a `code` string and optional `timeout_ms`."
                .to_string(),
        strict: false,
        defer_loading: None,
        parameters: codex_tools::JsonSchema::object(
            properties,
            Some(vec!["code".to_string()]),
            Some(codex_tools::AdditionalProperties::Boolean(false)),
        ),
        output_schema: None,
    })
}
