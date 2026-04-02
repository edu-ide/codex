use crate::function_tool::{FunctionCallError, ToolHandler};
use crate::tools::registry::ToolRegistryBuilder;
use async_trait::async_trait;
use codex_protocol::models::MessageContent;
use serde_json::Value;

pub struct NativeBrowserHandler;

#[async_trait]
impl ToolHandler for NativeBrowserHandler {
    fn name(&self) -> &'static str {
        "browser"
    }

    async fn execute(&self, arguments: &str) -> Result<Vec<MessageContent>, FunctionCallError> {
        let args: Value = serde_json::from_str(arguments).map_err(|e| {
            FunctionCallError::RespondToModel(format!("failed to parse browser args: {e}"))
        })?;
        let res = action_browser_rs::execute_browser_action(args).await.map_err(|e| {
            FunctionCallError::RespondToModel(format!("Browser error: {e}"))
        })?;
        let json_str = serde_json::to_string(&res).unwrap_or_default();
        Ok(vec![MessageContent::Text { text: json_str }])
    }
}

pub struct NativeComputerHandler;

#[async_trait]
impl ToolHandler for NativeComputerHandler {
    fn name(&self) -> &'static str {
        "computer"
    }

    async fn execute(&self, arguments: &str) -> Result<Vec<MessageContent>, FunctionCallError> {
        let args: Value = serde_json::from_str(arguments).map_err(|e| {
            FunctionCallError::RespondToModel(format!("failed to parse computer args: {e}"))
        })?;
        let res = action_computer_rs::execute_computer_action(args).map_err(|e| {
            FunctionCallError::RespondToModel(format!("Computer error: {e}"))
        })?;
        let json_str = serde_json::to_string(&res).unwrap_or_default();
        Ok(vec![MessageContent::Text { text: json_str }])
    }
}

pub struct NativeBrainHandler;

#[async_trait]
impl ToolHandler for NativeBrainHandler {
    fn name(&self) -> &'static str {
        "brain"
    }

    async fn execute(&self, arguments: &str) -> Result<Vec<MessageContent>, FunctionCallError> {
        let args: Value = serde_json::from_str(arguments).map_err(|e| {
            FunctionCallError::RespondToModel(format!("failed to parse brain args: {e}"))
        })?;
        // Directly call Brain memory operations natively here
        let res = brain_rs::execute_brain_action(args).await.map_err(|e| {
            FunctionCallError::RespondToModel(format!("Brain error: {e}"))
        })?;
        let json_str = serde_json::to_string(&res).unwrap_or_default();
        Ok(vec![MessageContent::Text { text: json_str }])
    }
}
