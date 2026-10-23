use codex_tools::ModelTextReplacement;
use codex_tools::ToolOutput;
use codex_tools::ToolPayload;
use serde_json::Value;
use serde_json::json;

use super::context::ToolInvocation;
use super::flat_tool_name;

pub(super) async fn process_output(
    invocation: &ToolInvocation,
    output: Box<dyn ToolOutput>,
) -> Box<dyn ToolOutput> {
    let Some(config) = invocation.turn.config.tool_result_processor.as_ref() else {
        return output;
    };
    let Some(snapshot) = output.raw_text_snapshot() else {
        return output;
    };
    let input = match &invocation.payload {
        ToolPayload::Function { arguments } => {
            serde_json::from_str(arguments).unwrap_or_else(|_| Value::String(arguments.clone()))
        }
        ToolPayload::Custom { input } => Value::String(input.clone()),
        ToolPayload::ToolSearch { arguments } => {
            let Ok(value) = serde_json::to_value(arguments) else {
                return output;
            };
            value
        }
    };
    let request = json!({
        "version": 1,
        "source": {
            "host": "codex",
            "session_uri": format!("codex://threads/{}", invocation.session.thread_id),
            "turn_id": invocation.turn.sub_id,
            "call_id": invocation.call_id,
            "tool_name": flat_tool_name(&invocation.tool_name),
            "cwd": invocation.step_context.environments.primary().map(|env| env.cwd().to_string())
                .unwrap_or_else(|| invocation.turn.config.cwd.display().to_string()),
        },
        "input": input,
        "output": snapshot,
    });
    match codex_hooks::process_tool_result(
        config,
        &request,
        invocation.turn.config.cwd.as_path(),
        invocation.cancellation_token.cancelled(),
    )
    .await
    {
        Some(text) => Box::new(ModelTextReplacement::new(output, text)),
        None => output,
    }
}
