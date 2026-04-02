use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::Path;
use std::sync::Arc;

use crate::function_tool::FunctionCallError;
use crate::tools::context::{FunctionToolOutput, ToolInvocation, ToolPayload};
use crate::tools::registry::{ToolHandler, ToolKind};

use super::lsp_manager::LspServerManager;
use super::parse_arguments_with_base_path;

pub const LSP_TOOL_NAME: &str = "lsp";

#[derive(Serialize, Deserialize, JsonSchema, Debug)]
#[serde(rename_all = "camelCase")]
pub enum LspOperation {
    GoToDefinition,
    FindReferences,
    Hover,
    DocumentSymbol,
    WorkspaceSymbol,
    GoToImplementation,
    PrepareCallHierarchy,
    IncomingCalls,
    OutgoingCalls,
}

#[derive(Serialize, Deserialize, JsonSchema, Debug)]
#[serde(rename_all = "camelCase")]
pub struct LspToolArgs {
    pub operation: LspOperation,
    pub file_path: String,
    #[serde(default)]
    pub line: u32,
    #[serde(default)]
    pub character: u32,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct LspToolOutput {
    pub operation: LspOperation,
    pub result: String,
    pub file_path: String,
    pub result_count: Option<u32>,
    pub file_count: Option<u32>,
}

pub struct LspToolHandler {
    manager: Arc<LspServerManager>,
}

impl LspToolHandler {
    pub fn new(manager: Arc<LspServerManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl ToolHandler for LspToolHandler {
    type Output = FunctionToolOutput;

    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn handle(&self, invocation: ToolInvocation) -> Result<Self::Output, FunctionCallError> {
        let arguments = match &invocation.payload {
            ToolPayload::Function { arguments } => arguments,
            _ => {
                return Err(FunctionCallError::RespondToModel(
                    "unsupported payload".to_string(),
                ));
            }
        };
        let cwd = invocation.turn.cwd.clone();
        let args: LspToolArgs = parse_arguments_with_base_path(arguments, cwd.as_path())?;

        let ext = Path::new(&args.file_path)
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("");

        // Ensure LSP server is running for this file type
        let server = match self
            .manager
            .get_or_start_server(ext, &format!("file://{}", cwd.display()))
            .await
        {
            Ok(s) => s,
            Err(e) => {
                let output = LspToolOutput {
                    operation: args.operation,
                    result: format!("Error starting LSP server: {}", e),
                    file_path: args.file_path,
                    result_count: Some(0),
                    file_count: Some(0),
                };
                return Ok(FunctionToolOutput {
                    body: vec![
                        codex_protocol::models::FunctionCallOutputContentItem::InputText {
                            text: serde_json::to_string(&output).unwrap_or_default(),
                        },
                    ],
                    success: Some(false),
                    post_tool_use_response: None,
                    hint: None,
                });
            }
        };

        // Try reading file content and open it in LSP
        if let Ok(content) = tokio::fs::read_to_string(&args.file_path).await {
            let _ = self
                .manager
                .ensure_file_open(&server, &args.file_path, &content)
                .await;
        }

        let lsp_res = match args.operation {
            LspOperation::GoToDefinition => {
                let params = json!({
                    "textDocument": { "uri": format!("file://{}", args.file_path) },
                    "position": { "line": args.line, "character": args.character }
                });
                server.send_request("textDocument/definition", params).await
            }
            LspOperation::FindReferences => {
                let params = json!({
                    "textDocument": { "uri": format!("file://{}", args.file_path) },
                    "position": { "line": args.line, "character": args.character },
                    "context": { "includeDeclaration": true }
                });
                server.send_request("textDocument/references", params).await
            }
            LspOperation::Hover => {
                let params = json!({
                    "textDocument": { "uri": format!("file://{}", args.file_path) },
                    "position": { "line": args.line, "character": args.character }
                });
                server.send_request("textDocument/hover", params).await
            }
            _ => Err(anyhow::anyhow!(
                "Operation not yet supported in this iteration."
            )),
        };

        let result_str = match lsp_res {
            Ok(val) => serde_json::to_string_pretty(&val).unwrap_or_else(|_| "[]".to_string()),
            Err(e) => format!("LSP Request Failed: {}", e),
        };

        let output = LspToolOutput {
            operation: args.operation,
            result: result_str,
            file_path: args.file_path,
            result_count: Some(1),
            file_count: Some(1),
        };

        Ok(FunctionToolOutput {
            body: vec![
                codex_protocol::models::FunctionCallOutputContentItem::InputText {
                    text: serde_json::to_string(&output).unwrap_or_else(|_| "[]".to_string()),
                },
            ],
            success: Some(true),
            post_tool_use_response: None,
            hint: None,
        })
    }
}
