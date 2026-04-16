use crate::JsonSchema;
use crate::ResponsesApiTool;
use crate::ToolSpec;
use std::collections::BTreeMap;

pub fn create_lsp_tool() -> ToolSpec {
    let position_properties = BTreeMap::from([
        (
            "line".to_string(),
            JsonSchema::number(Some("Line number (1-based, matching editor UI).".to_string())),
        ),
        (
            "column".to_string(),
            JsonSchema::number(Some("Column number (1-based, matching editor UI).".to_string())),
        ),
    ]);

    let properties = BTreeMap::from([
        (
            "operation".to_string(),
            JsonSchema::string(Some(
                "The LSP operation to perform: 'GoToDefinition', 'FindReferences', 'GetDiagnostics', 'Hover', 'Rename', 'CodeAction', 'DocumentSymbols', 'WorkspaceSymbols', 'CodeLens'.".to_string(),
            )),
        ),
        (
            "file_path".to_string(),
            JsonSchema::string(Some("The absolute path to the file to perform the operation on.".to_string())),
        ),
        (
            "content".to_string(),
            JsonSchema::string(Some("The content of the file. Required for most operations if the file has been modified.".to_string())),
        ),
        (
            "position".to_string(),
            JsonSchema::object(
                position_properties,
                Some(vec!["line".to_string(), "column".to_string()]),
                Some(false.into()),
            ),
        ),
        (
            "new_name".to_string(),
            JsonSchema::string(Some("The new name to use for a Rename operation.".to_string())),
        ),
        (
            "query".to_string(),
            JsonSchema::string(Some("The query to use for a WorkspaceSymbols operation.".to_string())),
        ),
    ]);

    ToolSpec::Function(ResponsesApiTool {
        name: "lsp".to_string(),
        description:
            "Language Server Protocol (LSP) tool for semantic code navigation and analysis."
                .to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(
            properties,
            Some(vec!["operation".to_string()]),
            Some(false.into()),
        ),
        output_schema: None,
    })
}
