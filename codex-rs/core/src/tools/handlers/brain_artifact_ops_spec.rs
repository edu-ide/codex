use codex_tools::JsonSchema;
use codex_tools::ResponsesApiTool;
use codex_tools::ToolSpec;
use std::collections::BTreeMap;

pub const BRAIN_ARTIFACT_OPS_TOOL_NAME: &str = "brain_artifact_ops";

pub(crate) fn create_brain_artifact_ops_tool() -> ToolSpec {
    let properties = BTreeMap::from([
        ("action".to_string(), JsonSchema::string(Some("save, edit, list, get.".to_string()))),
        ("session_id".to_string(), JsonSchema::string(Some("Session id.".to_string()))),
        ("project".to_string(), JsonSchema::string(Some("Project.".to_string()))),
        ("artifact_type".to_string(), JsonSchema::string(Some("Artifact name.".to_string()))),
        ("content".to_string(), JsonSchema::string(Some("Body.".to_string()))),
        ("summary".to_string(), JsonSchema::string(Some("Summary.".to_string()))),
        ("version".to_string(), JsonSchema::number(Some("Version.".to_string()))),
    ]);
    ToolSpec::Function(ResponsesApiTool {
        name: BRAIN_ARTIFACT_OPS_TOOL_NAME.to_string(),
        description: "Local Brain artifact/wiki tools.".to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(properties, Some(vec!["action".to_string()]), Some(false.into())),
        output_schema: None,
    })
}