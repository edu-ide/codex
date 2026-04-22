use async_trait::async_trait;
use rmcp::schemars::JsonSchema;
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
#[schemars(crate = "rmcp::schemars")]
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
#[schemars(crate = "rmcp::schemars")]
#[serde(rename_all = "camelCase")]
pub struct LspToolArgs {
    pub operation: LspOperation,
    pub file_path: String,
    #[serde(default)]
    pub line: u32,
    #[serde(default)]
    pub character: u32,
    #[serde(default)]
    pub query: Option<String>,
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
        let args: LspToolArgs = parse_arguments_with_base_path(arguments, &cwd)?;

        let ext = Path::new(&args.file_path)
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("");

        // Ensure LSP server is running for this file type
        let server: std::sync::Arc<crate::tools::handlers::lsp_manager::LspServerInstance> =
            match self
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
        let mut file_content: Option<String> = None;
        if let Ok(content) = tokio::fs::read_to_string(&args.file_path).await {
            file_content = Some(content.clone());
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
            LspOperation::DocumentSymbol => {
                let params = json!({
                    "textDocument": { "uri": format!("file://{}", args.file_path) }
                });
                server
                    .send_request("textDocument/documentSymbol", params)
                    .await
            }
            LspOperation::WorkspaceSymbol => {
                let params = json!({
                    "query": args.query.clone().unwrap_or_default()
                });
                server.send_request("workspace/symbol", params).await
            }
            _ => Err(anyhow::anyhow!(
                "Operation not yet supported in this iteration."
            )),
        };

        let result_str = match lsp_res {
            Ok(val) => {
                if matches!(
                    args.operation,
                    LspOperation::DocumentSymbol | LspOperation::WorkspaceSymbol
                ) {
                    format_symbols(&val, 0, file_content.as_deref())
                } else {
                    serde_json::to_string_pretty(&val).unwrap_or_else(|_| "[]".to_string())
                }
            }
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

fn format_symbols(
    symbols: &serde_json::Value,
    indent: usize,
    file_content: Option<&str>,
) -> String {
    let mut out = String::new();
    if let Some(arr) = symbols.as_array() {
        for sym in arr {
            let name = sym.get("name").and_then(|n| n.as_str()).unwrap_or("?");
            let kind = sym.get("kind").and_then(|k| k.as_u64()).unwrap_or(0);

            let location = sym.get("location");
            let range = sym
                .get("range")
                .or_else(|| location.and_then(|l| l.get("range")));
            let uri = location.and_then(|l| l.get("uri")).and_then(|u| u.as_str());

            let start_line = range
                .and_then(|r| r.get("start"))
                .and_then(|s| s.get("line"))
                .and_then(|l| l.as_u64())
                .unwrap_or(0);
            let end_line = range
                .and_then(|r| r.get("end"))
                .and_then(|s| s.get("line"))
                .and_then(|l| l.as_u64())
                .unwrap_or(0);

            let kind_str = match kind {
                1 => "File",
                2 => "Module",
                3 => "Namespace",
                4 => "Package",
                5 => "Class",
                6 => "Method",
                7 => "Property",
                8 => "Field",
                9 => "Constructor",
                10 => "Enum",
                11 => "Interface",
                12 => "Function",
                13 => "Variable",
                14 => "Constant",
                15 => "String",
                16 => "Number",
                17 => "Boolean",
                18 => "Array",
                19 => "Object",
                20 => "Key",
                21 => "Null",
                22 => "EnumMember",
                23 => "Struct",
                24 => "Event",
                25 => "Operator",
                26 => "TypeParameter",
                _ => "Symbol",
            };

            let prefix = "  ".repeat(indent);

            let mut extra = String::new();
            if let Some(u) = uri {
                let clean_uri = u.strip_prefix("file://").unwrap_or(u);
                extra = format!(" ({})", clean_uri);
            } else if let Some(content) = file_content {
                if let Some(line) = content.lines().nth(start_line as usize) {
                    let trimmed = line.trim();
                    if !trimmed.is_empty() {
                        extra = format!(" // {}", trimmed);
                    }
                }
            }

            out.push_str(&format!(
                "{}[Line {}-{}] [{}] {}{}\n",
                prefix,
                start_line + 1,
                end_line + 1,
                kind_str,
                name,
                extra
            ));

            if let Some(children) = sym.get("children") {
                out.push_str(&format_symbols(children, indent + 1, file_content));
            }
        }
    }
    out
}
