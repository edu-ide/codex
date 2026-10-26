use codex_tools::JsonSchema;
use codex_tools::ResponsesApiTool;
use codex_tools::ToolSpec;
use std::collections::BTreeMap;

pub const BRAIN_MEMORY_OPS_TOOL_NAME: &str = "brain_memory_ops";

pub(crate) fn create_brain_memory_ops_tool() -> ToolSpec {
    let properties = BTreeMap::from([
        (
            "action".to_string(),
            JsonSchema::string(Some(
                "search, doc_answer, index_start, index_status, index_stop, index_resume, log, save_item. doc_answer requires query and returns an answer with original-source citations."
                    .to_string(),
            )),
        ),
        (
            "query".to_string(),
            JsonSchema::string(Some("Query for search or doc_answer.".to_string())),
        ),
        (
            "path".to_string(),
            JsonSchema::string(Some("Folder for index_start.".to_string())),
        ),
        (
            "project".to_string(),
            JsonSchema::string(Some("Project namespace; limits search and doc_answer to this project.".to_string())),
        ),
        (
            "content".to_string(),
            JsonSchema::string(Some("Content for log/write.".to_string())),
        ),
        (
            "id".to_string(),
            JsonSchema::string(Some("Item id.".to_string())),
        ),
        (
            "title".to_string(),
            JsonSchema::string(Some("Title.".to_string())),
        ),
        (
            "summary".to_string(),
            JsonSchema::string(Some("Summary.".to_string())),
        ),
        (
            "limit".to_string(),
            JsonSchema::integer(Some("Result limit for search or source count for doc_answer (default 5).".to_string())),
        ),
        (
            "search_mode".to_string(),
            JsonSchema::string_enum(
                vec![
                    "hybrid".into(),
                    "graph".into(),
                    "local".into(),
                    "global".into(),
                    "exact".into(),
                    "deep".into(),
                ],
                Some("Retrieval mode for search or doc_answer: hybrid (default) combines keyword/vector retrieval with the Brain graph; graph/local follows relations, global uses graph communities, exact searches keywords, deep uses vector retrieval and reranking.".to_string()),
            ),
        ),
        (
            "confirm".to_string(),
            JsonSchema::boolean(Some("Force index_stop.".to_string())),
        ),
    ]);
    ToolSpec::Function(ResponsesApiTool {
        name: BRAIN_MEMORY_OPS_TOOL_NAME.to_string(),
        description: "Local Brain memory and document GraphRAG: index, search, or doc_answer with original-source citations. Use project to scope retrieval and search_mode to select hybrid, graph/local, global, exact, or deep.".to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(
            properties,
            Some(vec!["action".to_string()]),
            Some(false.into()),
        ),
        output_schema: None,
    })
}
