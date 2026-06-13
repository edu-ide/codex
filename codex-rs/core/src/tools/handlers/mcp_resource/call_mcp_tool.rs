use std::sync::Arc;
use std::time::Instant;

use crate::function_tool::FunctionCallError;
use crate::mcp_tool_call::handle_mcp_tool_call;
use crate::original_image_detail::can_request_original_image_detail;
use crate::tools::context::McpToolOutput;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::context::boxed_tool_output;
use crate::tools::handlers::mcp_resource_spec::create_call_mcp_tool;
use crate::tools::hook_names::HookToolName;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolExecutor;
use codex_tools::ToolName;
use codex_tools::ToolSpec;

use super::CallMcpToolArgs;
use super::normalize_required_string;
use super::parse_args;
use super::parse_arguments;

pub struct CallMcpToolHandler;

impl ToolExecutor<ToolInvocation> for CallMcpToolHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain("call_mcp_tool")
    }

    fn spec(&self) -> ToolSpec {
        create_call_mcp_tool()
    }

    fn handle(&self, invocation: ToolInvocation) -> codex_tools::ToolExecutorFuture<'_> {
        Box::pin(self.handle_call(invocation))
    }
}

impl CallMcpToolHandler {
    async fn handle_call(
        &self,
        invocation: ToolInvocation,
    ) -> Result<Box<dyn crate::tools::context::ToolOutput>, FunctionCallError> {
        let ToolInvocation {
            session,
            turn,
            call_id,
            payload,
            ..
        } = invocation;

        let payload = match payload {
            ToolPayload::Function { arguments } => arguments,
            _ => {
                return Err(FunctionCallError::RespondToModel(
                    "call_mcp_tool handler received unsupported payload".to_string(),
                ));
            }
        };

        let args: CallMcpToolArgs = parse_args(parse_arguments(payload.as_str())?)?;
        let server = normalize_required_string("server", args.server)?;
        let tool = normalize_required_string("tool", args.tool)?;
        let arguments = serde_json::to_string(&args.arguments).map_err(|err| {
            FunctionCallError::RespondToModel(format!(
                "failed to serialize MCP tool arguments: {err}"
            ))
        })?;

        let started = Instant::now();
        let result = handle_mcp_tool_call(
            Arc::clone(&session),
            &turn,
            call_id,
            server.clone(),
            tool.clone(),
            HookToolName::new(format!("mcp__{server}__{tool}")),
            arguments,
        )
        .await;

        Ok(boxed_tool_output(McpToolOutput {
            result: result.result,
            tool_input: result.tool_input,
            wall_time: started.elapsed(),
            original_image_detail_supported: can_request_original_image_detail(&turn.model_info),
            truncation_policy: turn.truncation_policy,
        }))
    }
}

impl CoreToolRuntime for CallMcpToolHandler {}
