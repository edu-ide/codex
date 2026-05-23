use codex_tools::JsonSchema;
use codex_tools::ResponsesApiTool;
use codex_tools::ToolSpec;
use std::collections::BTreeMap;

pub const BRAIN_VAULT_PATCH_TOOL_NAME: &str = "brain_vault_patch";

pub(crate) fn create_brain_vault_patch_tool() -> ToolSpec {
    let properties = BTreeMap::from([
        (
            "action".to_string(),
            JsonSchema::string(Some(
                "Record type, such as record_decision, record_log, record_wiki, record_research, record_improvement, or record_cleanup."
                    .to_string(),
            )),
        ),
        (
            "scope".to_string(),
            JsonSchema::string(Some(
                "Vault scope: project or global. Defaults to project.".to_string(),
            )),
        ),
        (
            "vault".to_string(),
            JsonSchema::string(Some(
                "Vault target: brain or wiki. Defaults to brain.".to_string(),
            )),
        ),
        (
            "location".to_string(),
            JsonSchema::string(Some(
                "Project wiki location: local for .ilhae/wiki or docs for docs/wiki. Defaults to local."
                    .to_string(),
            )),
        ),
        (
            "loop_phase".to_string(),
            JsonSchema::string(Some(
                "Goal loop phase that produced the record, such as decision, wiki, log, or web_research."
                    .to_string(),
            )),
        ),
        (
            "title".to_string(),
            JsonSchema::string(Some("Short record title.".to_string())),
        ),
        (
            "content".to_string(),
            JsonSchema::string(Some(
                "Markdown body to append to the Brain/Wiki record.".to_string(),
            )),
        ),
        (
            "summary".to_string(),
            JsonSchema::string(Some(
                "Fallback summary when content is not supplied.".to_string(),
            )),
        ),
        (
            "base_hash".to_string(),
            JsonSchema::string(Some(
                "Optional CAS guard. When supplied, the write succeeds only if the current target content hash matches this baseHash."
                    .to_string(),
            )),
        ),
    ]);

    ToolSpec::Function(ResponsesApiTool {
        name: BRAIN_VAULT_PATCH_TOOL_NAME.to_string(),
        description: "Append a compact goal-loop record to the local Ilhae Brain/Wiki vault using a CAS-style write. Emits apply_patch-style file-change events and returns resourceUri, path, baseHash, and newHash. Use this for non-execution loop decisions, wiki notes, logs, research notes, improvements, and cleanup records."
            .to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(properties, None, Some(false.into())),
        output_schema: None,
    })
}
