use codex_tools::JsonSchema;
use codex_tools::ResponsesApiTool;
use codex_tools::ToolSpec;
use std::collections::BTreeMap;

pub const BRAIN_MEMORY_OPS_TOOL_NAME: &str = "brain_memory_ops";

pub(crate) fn create_brain_memory_ops_tool() -> ToolSpec {
    let properties = BTreeMap::from([
        ("action".to_string(), JsonSchema::string(Some("search, index_start, index_status, index_stop, index_resume, log, save_item.".to_string()))),
        ("query".to_string(), JsonSchema::string(Some("Search query.".to_string()))),
        ("path".to_string(), JsonSchema::string(Some("Folder for index_start.".to_string()))),
        ("project".to_string(), JsonSchema::string(Some("Project namespace.".to_string()))),
        ("content".to_string(), JsonSchema::string(Some("Content for log/write.".to_string()))),
        ("id".to_string(), JsonSchema::string(Some("Item id.".to_string()))),
        ("title".to_string(), JsonSchema::string(Some("Title.".to_string()))),
        ("summary".to_string(), JsonSchema::string(Some("Summary.".to_string()))),
        ("limit".to_string(), JsonSchema::number(Some("Search limit.".to_string()))),
        ("confirm".to_string(), JsonSchema::boolean(Some("Force index_stop.".to_string()))),
    ]);
    ToolSpec::Function(ResponsesApiTool {
        name: BRAIN_MEMORY_OPS_TOOL_NAME.to_string(),
        description: "Local Brain memory and document RAG (index + search).".to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(properties, Some(vec!["action".to_string()]), Some(false.into())),
        output_schema: None,
    })
}