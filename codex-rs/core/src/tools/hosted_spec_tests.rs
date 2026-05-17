use super::*;
use codex_protocol::config_types::WebSearchContextSize;
use codex_protocol::config_types::WebSearchEngine;
use codex_protocol::config_types::WebSearchFilters;
use codex_protocol::config_types::WebSearchUserLocation;
use codex_protocol::config_types::WebSearchUserLocationType;
use codex_tools::ResponsesApiWebSearchFilters;
use codex_tools::ResponsesApiWebSearchUserLocation;
use pretty_assertions::assert_eq;

#[test]
fn image_generation_tool_matches_expected_spec() {
    assert_eq!(
        create_image_generation_tool("png"),
        ToolSpec::ImageGeneration {
            output_format: "png".to_string(),
        }
    );
}

#[test]
fn web_search_tool_preserves_configured_options() {
    assert_eq!(
        create_web_search_tool(WebSearchToolOptions {
            web_search_mode: Some(WebSearchMode::Live),
            web_search_config: Some(&WebSearchConfig {
                filters: Some(WebSearchFilters {
                    allowed_domains: Some(vec!["example.com".to_string()]),
                }),
                user_location: Some(WebSearchUserLocation {
                    r#type: WebSearchUserLocationType::Approximate,
                    country: Some("US".to_string()),
                    region: None,
                    city: None,
                    timezone: Some("America/Los_Angeles".to_string()),
                }),
                search_context_size: Some(WebSearchContextSize::Low),
                ..Default::default()
            }),
            web_search_tool_type: WebSearchToolType::TextAndImage,
        }),
        Some(ToolSpec::WebSearch {
            external_web_access: Some(true),
            filters: Some(ResponsesApiWebSearchFilters {
                allowed_domains: Some(vec!["example.com".to_string()]),
            }),
            user_location: Some(ResponsesApiWebSearchUserLocation {
                r#type: WebSearchUserLocationType::Approximate,
                country: Some("US".to_string()),
                region: None,
                city: None,
                timezone: Some("America/Los_Angeles".to_string()),
            }),
            search_context_size: Some(WebSearchContextSize::Low),
            search_content_types: Some(vec!["text".to_string(), "image".to_string()]),
        })
    );
}

#[test]
fn web_search_tool_uses_local_function_for_duckduckgo_engine() {
    let tool = create_web_search_tool(WebSearchToolOptions {
        web_search_mode: Some(WebSearchMode::Live),
        web_search_config: Some(&WebSearchConfig {
            engine: Some(WebSearchEngine::Duckduckgo),
            ..Default::default()
        }),
        web_search_tool_type: WebSearchToolType::Text,
    })
    .expect("web search tool");

    let serialized = serde_json::to_value(tool).expect("serialize tool");
    assert_eq!(serialized["type"], "function");
    assert_eq!(serialized["name"], "web_search");
    assert_eq!(
        serialized["parameters"]["required"],
        serde_json::json!(["query"])
    );
}

#[test]
fn web_search_tool_uses_local_function_for_searxng_url() {
    let tool = create_web_search_tool(WebSearchToolOptions {
        web_search_mode: Some(WebSearchMode::Live),
        web_search_config: Some(&WebSearchConfig {
            searxng_url: Some("http://localhost:8888".to_string()),
            ..Default::default()
        }),
        web_search_tool_type: WebSearchToolType::Text,
    })
    .expect("web search tool");

    let ToolSpec::Function(function_tool) = tool else {
        panic!("expected local function web_search tool");
    };
    assert_eq!(function_tool.name, "web_search");
    assert!(function_tool.description.contains("DuckDuckGo"));
}

#[test]
fn web_search_tool_uses_local_function_for_duckduckgo_fallback_config() {
    let tool = create_web_search_tool(WebSearchToolOptions {
        web_search_mode: Some(WebSearchMode::Live),
        web_search_config: Some(&WebSearchConfig {
            use_duckduckgo_fallback: Some(true),
            ..Default::default()
        }),
        web_search_tool_type: WebSearchToolType::Text,
    })
    .expect("web search tool");

    assert!(matches!(tool, ToolSpec::Function(_)));
}

#[test]
fn web_search_tool_is_absent_when_disabled() {
    assert_eq!(
        create_web_search_tool(WebSearchToolOptions {
            web_search_mode: Some(WebSearchMode::Disabled),
            web_search_config: None,
            web_search_tool_type: WebSearchToolType::Text,
        }),
        None
    );
}
