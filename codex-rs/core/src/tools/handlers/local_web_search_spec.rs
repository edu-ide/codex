use codex_tools::JsonSchema;
use codex_tools::ResponsesApiTool;
use codex_tools::ToolSpec;
use std::collections::BTreeMap;

pub const LOCAL_WEB_SEARCH_TOOL_NAME: &str = "web_search";

pub(crate) fn create_local_web_search_tool() -> ToolSpec {
    let properties = BTreeMap::from([
        (
            "query".to_string(),
            JsonSchema::string(Some("Search query.".to_string())),
        ),
        (
            "limit".to_string(),
            JsonSchema::number(Some(
                "Maximum number of search results to return. Defaults to 5.".to_string(),
            )),
        ),
    ]);

    ToolSpec::Function(ResponsesApiTool {
        name: LOCAL_WEB_SEARCH_TOOL_NAME.to_string(),
        description: "Search the web using the local Ilhae web-search adapter. Use this when current external information is needed, including web research loop discovery."
            .to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(
            properties,
            Some(vec!["query".to_string()]),
            Some(false.into()),
        ),
        output_schema: None,
    })
}
