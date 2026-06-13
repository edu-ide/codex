use crate::function_tool::FunctionCallError;
use crate::tools::context::{FunctionToolOutput, ToolInvocation, ToolPayload, boxed_tool_output};
use crate::tools::handlers::brain_artifact_ops_spec::{BRAIN_ARTIFACT_OPS_TOOL_NAME, create_brain_artifact_ops_tool};
use crate::tools::handlers::brain_service::shared_brain_service;
use crate::tools::handlers::parse_arguments;
use crate::tools::registry::{CoreToolRuntime, ToolExecutor};
use brain_rs::{ArtifactOpsRequest, execute_artifact_ops};
use codex_tools::{ToolName, ToolSpec};

pub struct BrainArtifactOpsHandler;

impl ToolExecutor<ToolInvocation> for BrainArtifactOpsHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain(BRAIN_ARTIFACT_OPS_TOOL_NAME)
    }
    fn spec(&self) -> ToolSpec {
        create_brain_artifact_ops_tool()
    }
    fn handle(&self, invocation: ToolInvocation) -> codex_tools::ToolExecutorFuture<'_> {
        Box::pin(self.handle_call(invocation))
    }
}

impl BrainArtifactOpsHandler {
    async fn handle_call(
        &self,
        invocation: ToolInvocation,
    ) -> Result<Box<dyn crate::tools::context::ToolOutput>, FunctionCallError> {
        let arguments = match invocation.payload {
            ToolPayload::Function { arguments } => arguments,
            _ => return Err(FunctionCallError::RespondToModel("unsupported payload".into())),
        };
        let req: ArtifactOpsRequest = parse_arguments(&arguments)?;
        let service = shared_brain_service()?;
        let output = tokio::task::spawn_blocking(move || execute_artifact_ops(service.as_ref(), req))
            .await
            .map_err(|e| FunctionCallError::RespondToModel(e.to_string()))?
            .map_err(|e| FunctionCallError::RespondToModel(e.to_string()))?;
        let text = serde_json::to_string_pretty(&output)
            .map_err(|e| FunctionCallError::RespondToModel(e.to_string()))?;
        Ok(boxed_tool_output(FunctionToolOutput::from_text(text, Some(true))))
    }
}
impl CoreToolRuntime for BrainArtifactOpsHandler {}