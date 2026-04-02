use crate::JsonSchema;
use crate::ResponsesApiTool;
use crate::ToolSpec;
use std::collections::BTreeMap;

pub fn create_lsp_tool() -> ToolSpec {
    let position_properties = BTreeMap::from([
        (
            "line".to_string(),
            JsonSchema::Number {
                description: Some("Line number (1-based, matching editor UI).".to_string()),
            },
        ),
        (
            "column".to_string(),
            JsonSchema::Number {
                description: Some("Column number (1-based, matching editor UI).".to_string()),
            },
        ),
    ]);

    let properties = BTreeMap::from([
        (
            "operation".to_string(),
            JsonSchema::String {
                description: Some(
                    "The LSP operation to perform: 'GoToDefinition', 'FindReferences', 'GetDiagnostics', 'Hover', 'Rename', 'CodeAction', 'DocumentSymbols', 'WorkspaceSymbols', 'CodeLens'.".to_string(),
                ),
            },
        ),
        (
            "file_path".to_string(),
            JsonSchema::String {
                description: Some("The absolute path to the file to perform the operation on.".to_string()),
            },
        ),
        (
            "content".to_string(),
            JsonSchema::String {
                description: Some("The content of the file. Required for most operations if the file has been modified.".to_string()),
            },
        ),
        (
            "position".to_string(),
            JsonSchema::Object {
                properties: position_properties,
                required: Some(vec!["line".to_string(), "column".to_string()]),
                additional_properties: Some(false.into()),
            },
        ),
        (
            "new_name".to_string(),
            JsonSchema::String {
                description: Some("The new name to use for a Rename operation.".to_string()),
            },
        ),
        (
            "query".to_string(),
            JsonSchema::String {
                description: Some("The query to use for a WorkspaceSymbols operation.".to_string()),
            },
        ),
    ]);

    ToolSpec::Function(ResponsesApiTool {
        name: "lsp".to_string(),
        description:
            "Language Server Protocol (LSP) tool for semantic code navigation and analysis."
                .to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::Object {
            properties,
            required: Some(vec!["operation".to_string()]),
            additional_properties: Some(false.into()),
        },
        output_schema: None,
    })
}
