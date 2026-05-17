use codex_protocol::config_types::WebSearchConfig;
use codex_protocol::config_types::WebSearchEngine;
use codex_protocol::config_types::WebSearchMode;
use codex_protocol::openai_models::WebSearchToolType;
use codex_tools::AdditionalProperties;
use codex_tools::ResponsesApiTool;
use codex_tools::ToolSpec;
use std::collections::BTreeMap;

const WEB_SEARCH_TEXT_AND_IMAGE_CONTENT_TYPES: [&str; 2] = ["text", "image"];

pub struct WebSearchToolOptions<'a> {
    pub web_search_mode: Option<WebSearchMode>,
    pub web_search_config: Option<&'a WebSearchConfig>,
    pub web_search_tool_type: WebSearchToolType,
}

pub fn create_image_generation_tool(output_format: &str) -> ToolSpec {
    ToolSpec::ImageGeneration {
        output_format: output_format.to_string(),
    }
}

pub fn create_web_search_tool(options: WebSearchToolOptions<'_>) -> Option<ToolSpec> {
    let external_web_access = match options.web_search_mode {
        Some(WebSearchMode::Cached) => Some(false),
        Some(WebSearchMode::Live) => Some(true),
        Some(WebSearchMode::Disabled) | None => None,
    }?;

    if should_use_local_web_search(options.web_search_config) {
        return Some(create_local_web_search_tool());
    }

    let search_content_types = match options.web_search_tool_type {
        WebSearchToolType::Text => None,
        WebSearchToolType::TextAndImage => Some(
            WEB_SEARCH_TEXT_AND_IMAGE_CONTENT_TYPES
                .into_iter()
                .map(str::to_string)
                .collect(),
        ),
    };

    Some(ToolSpec::WebSearch {
        external_web_access: Some(external_web_access),
        filters: options
            .web_search_config
            .and_then(|config| config.filters.clone().map(Into::into)),
        user_location: options
            .web_search_config
            .and_then(|config| config.user_location.clone().map(Into::into)),
        search_context_size: options
            .web_search_config
            .and_then(|config| config.search_context_size),
        search_content_types,
    })
}

fn should_use_local_web_search(config: Option<&WebSearchConfig>) -> bool {
    let Some(config) = config else {
        return false;
    };

    matches!(
        config.engine,
        Some(WebSearchEngine::Duckduckgo | WebSearchEngine::Searxng)
    ) || config
        .searxng_url
        .as_deref()
        .is_some_and(|url| !url.trim().is_empty())
        || config.use_duckduckgo_fallback.is_some()
}

fn create_local_web_search_tool() -> ToolSpec {
    let mut properties = BTreeMap::new();
    properties.insert(
        "query".to_string(),
        codex_tools::JsonSchema::string(Some(
            "Search query. Extract dates, times, scores, names, numbers, prices, and facts. Be specific, direct, and detailed.".to_string(),
        )),
    );

    ToolSpec::Function(ResponsesApiTool {
        name: "web_search".to_string(),
        description:
            "Perform a web search using DuckDuckGo or SearXNG and read the content using Jina.ai."
                .to_string(),
        strict: false,
        defer_loading: None,
        parameters: codex_tools::JsonSchema::object(
            properties,
            Some(vec!["query".to_string()]),
            Some(AdditionalProperties::Boolean(false)),
        ),
        output_schema: None,
    })
}

#[cfg(test)]
#[path = "hosted_spec_tests.rs"]
mod tests;
