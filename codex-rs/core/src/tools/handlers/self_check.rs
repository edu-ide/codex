use async_trait::async_trait;
use serde::Deserialize;

use crate::function_tool::FunctionCallError;
use crate::tools::context::{FunctionToolOutput, ToolInvocation, ToolPayload};
use crate::tools::registry::{ToolHandler, ToolKind};

pub struct SelfCheckHandler;

#[derive(Deserialize)]
struct SelfCheckArgs {
    status: String,
    errors_encountered: Option<Vec<String>>,
    mitigation_plan: Option<String>,
}

#[async_trait]
impl ToolHandler for SelfCheckHandler {
    type Output = FunctionToolOutput;

    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn handle(&self, invocation: ToolInvocation) -> Result<Self::Output, FunctionCallError> {
        let args: SelfCheckArgs =
            crate::tools::handlers::parse_arguments(&match invocation.payload {
                ToolPayload::Function { arguments } => arguments,
                _ => {
                    return Err(FunctionCallError::RespondToModel(
                        "self_check received unsupported payload".to_string(),
                    ));
                }
            })?;

        let mut output = format!("Self-check recorded. Status: {}.", args.status);
        if let Some(errs) = args.errors_encountered {
            output.push_str(&format!("\nTracking {} errors for mitigation.", errs.len()));
        }
        if let Some(plan) = args.mitigation_plan {
            output.push_str(&format!("\nMitigation plan logged: {}", plan));
        }

        let mut tool_output = FunctionToolOutput::from_text(output, Some(true));

        if args.status.to_lowercase() == "failed" || args.status.to_lowercase() == "blocked" {
            tool_output.hint = Some("Self-check indicates a blockage. Consider reviewing the problem from a different angle, breaking it down into smaller steps, or consulting the user for clarification.".to_string());
        } else {
            tool_output.hint = Some(
                "Self-check resolved. Proceed with the next steps of your execution plan."
                    .to_string(),
            );
        }

        Ok(tool_output)
    }
}
