use crate::function_tool::FunctionCallError;
use crate::tools::context::FunctionToolOutput;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::context::boxed_tool_output;
use crate::tools::handlers::local_web_search_spec::LOCAL_WEB_SEARCH_TOOL_NAME;
use crate::tools::handlers::local_web_search_spec::create_local_web_search_tool;
use crate::tools::handlers::parse_arguments;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolExecutor;
use codex_protocol::models::WebSearchAction;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::WebSearchBeginEvent;
use codex_protocol::protocol::WebSearchEndEvent;
use codex_tools::ToolName;
use codex_tools::ToolSpec;
use serde::Deserialize;
use std::path::Path;
use std::time::Duration;
use url::Url;
use url::form_urlencoded;

const DEFAULT_LIMIT: usize = 5;
const MAX_LIMIT: usize = 10;
const SEARCH_TIMEOUT_SECS: u64 = 15;
const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120 Safari/537.36";

pub struct LocalWebSearchHandler;

#[derive(Debug, Deserialize)]
struct LocalWebSearchArgs {
    query: String,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SearchResult {
    title: String,
    url: String,
    snippet: String,
}

#[derive(Debug, Deserialize)]
struct SearxngResponse {
    #[serde(default)]
    results: Vec<SearxngResult>,
}

#[derive(Debug, Deserialize)]
struct SearxngResult {
    title: Option<String>,
    url: Option<String>,
    content: Option<String>,
}

impl ToolExecutor<ToolInvocation> for LocalWebSearchHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain(LOCAL_WEB_SEARCH_TOOL_NAME)
    }

    fn spec(&self) -> ToolSpec {
        create_local_web_search_tool()
    }

    fn handle(&self, invocation: ToolInvocation) -> codex_tools::ToolExecutorFuture<'_> {
        Box::pin(self.handle_call(invocation))
    }
}

impl LocalWebSearchHandler {
    async fn handle_call(
        &self,
        invocation: ToolInvocation,
    ) -> Result<Box<dyn crate::tools::context::ToolOutput>, FunctionCallError> {
        let ToolInvocation {
            session,
            turn,
            call_id,
            payload,
            ..
        } = invocation;
        let arguments = match payload {
            ToolPayload::Function { arguments } => arguments,
            _ => {
                return Err(FunctionCallError::RespondToModel(
                    "web_search handler received unsupported payload".to_string(),
                ));
            }
        };

        let args: LocalWebSearchArgs = parse_arguments(&arguments)?;
        let query = args.query.trim();
        if query.is_empty() {
            return Err(FunctionCallError::RespondToModel(
                "web_search query must not be empty".to_string(),
            ));
        }
        let limit = args.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
        let action = WebSearchAction::Search {
            query: Some(query.to_string()),
            queries: None,
        };
        session
            .send_event(
                &turn,
                EventMsg::WebSearchBegin(WebSearchBeginEvent {
                    call_id: call_id.clone(),
                }),
            )
            .await;

        let results = match web_search(query, limit).await {
            Ok(results) => results,
            Err(err) => {
                session
                    .send_event(
                        &turn,
                        EventMsg::WebSearchEnd(WebSearchEndEvent {
                            call_id,
                            query: query.to_string(),
                            action,
                        }),
                    )
                    .await;
                return Err(err);
            }
        };
        session
            .send_event(
                &turn,
                EventMsg::WebSearchEnd(WebSearchEndEvent {
                    call_id,
                    query: query.to_string(),
                    action,
                }),
            )
            .await;
        Ok(boxed_tool_output(FunctionToolOutput::from_text(
            render_results(query, &results),
            Some(!results.is_empty()),
        )))
    }
}

impl CoreToolRuntime for LocalWebSearchHandler {}

async fn web_search(query: &str, limit: usize) -> Result<Vec<SearchResult>, FunctionCallError> {
    let mut searxng_error = None;
    if let Some(searxng_url) = configured_searxng_url() {
        match searxng_search(&searxng_url, query, limit).await {
            Ok(results) if !results.is_empty() => return Ok(results),
            Ok(_) => {
                searxng_error = Some(format!("SearXNG at {searxng_url} returned no results"));
            }
            Err(err) => {
                searxng_error = Some(format!("SearXNG at {searxng_url} failed: {err}"));
            }
        }
    }

    match duckduckgo_search(query, limit).await {
        Ok(results) if !results.is_empty() => Ok(results),
        Ok(results) => {
            if let Some(searxng_error) = searxng_error {
                return Err(FunctionCallError::RespondToModel(format!(
                    "web_search failed: {searxng_error}; DuckDuckGo returned no results."
                )));
            }
            Ok(results)
        }
        Err(err) => {
            if let Some(searxng_error) = searxng_error {
                return Err(FunctionCallError::RespondToModel(format!(
                    "web_search failed: {searxng_error}; {err}"
                )));
            }
            Err(err)
        }
    }
}

async fn searxng_search(
    base_url: &str,
    query: &str,
    limit: usize,
) -> Result<Vec<SearchResult>, FunctionCallError> {
    let url = format!("{}/search", base_url.trim_end_matches('/'));
    let response: SearxngResponse = reqwest::Client::builder()
        .timeout(Duration::from_secs(SEARCH_TIMEOUT_SECS))
        .user_agent(USER_AGENT)
        .build()
        .map_err(|err| {
            FunctionCallError::RespondToModel(format!("web_search SearXNG init failed: {err}"))
        })?
        .get(url)
        .query(&[("q", query), ("format", "json")])
        .send()
        .await
        .map_err(|err| {
            FunctionCallError::RespondToModel(format!("web_search SearXNG request failed: {err}"))
        })?
        .error_for_status()
        .map_err(|err| {
            FunctionCallError::RespondToModel(format!("web_search SearXNG HTTP error: {err}"))
        })?
        .json()
        .await
        .map_err(|err| {
            FunctionCallError::RespondToModel(format!("web_search SearXNG JSON error: {err}"))
        })?;

    Ok(response
        .results
        .into_iter()
        .filter_map(|result| {
            let title = result.title?.trim().to_string();
            let url = result.url?.trim().to_string();
            if title.is_empty() || url.is_empty() {
                return None;
            }
            Some(SearchResult {
                title,
                url,
                snippet: result.content.unwrap_or_default(),
            })
        })
        .take(limit)
        .collect())
}

async fn duckduckgo_search(
    query: &str,
    limit: usize,
) -> Result<Vec<SearchResult>, FunctionCallError> {
    let encoded_query = form_urlencoded::Serializer::new(String::new())
        .append_pair("q", query)
        .finish();
    let url = format!("https://html.duckduckgo.com/html/?{encoded_query}");
    let html = reqwest::Client::builder()
        .timeout(Duration::from_secs(SEARCH_TIMEOUT_SECS))
        .user_agent(USER_AGENT)
        .build()
        .map_err(|err| FunctionCallError::RespondToModel(format!("web_search init failed: {err}")))?
        .get(url)
        .send()
        .await
        .map_err(|err| {
            FunctionCallError::RespondToModel(format!("web_search request failed: {err}"))
        })?
        .error_for_status()
        .map_err(|err| FunctionCallError::RespondToModel(format!("web_search HTTP error: {err}")))?
        .text()
        .await
        .map_err(|err| {
            FunctionCallError::RespondToModel(format!("web_search response read failed: {err}"))
        })?;

    if is_duckduckgo_bot_challenge(&html) {
        return Err(FunctionCallError::RespondToModel(
            "web_search DuckDuckGo fallback failed: DuckDuckGo returned a bot challenge page"
                .to_string(),
        ));
    }

    Ok(parse_duckduckgo_html(&html, limit))
}

fn configured_searxng_url() -> Option<String> {
    std::env::var("ILHAE_WEB_SEARCH_SEARXNG_URL")
        .ok()
        .or_else(|| std::env::var("SEARXNG_URL").ok())
        .and_then(non_empty_trimmed)
        .or_else(|| {
            std::env::var("CODEX_HOME").ok().and_then(|codex_home| {
                searxng_url_from_config(Path::new(&codex_home).join("config.toml"))
            })
        })
        .or_else(|| {
            std::env::var("HOME").ok().and_then(|home| {
                searxng_url_from_config(Path::new(&home).join(".ilhae/config.toml"))
            })
        })
}

fn searxng_url_from_config(path: impl AsRef<Path>) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    let config = toml::from_str::<toml::Table>(&content).ok()?;
    config
        .get("web_search_config")
        .and_then(toml::Value::as_table)
        .and_then(|table| table.get("searxng_url"))
        .and_then(toml::Value::as_str)
        .and_then(non_empty_trimmed)
}

fn non_empty_trimmed(value: impl AsRef<str>) -> Option<String> {
    let value = value.as_ref().trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn is_duckduckgo_bot_challenge(html: &str) -> bool {
    html.contains("anomaly-modal") || html.contains("Unfortunately, bots use DuckDuckGo too")
}

fn render_results(query: &str, results: &[SearchResult]) -> String {
    if results.is_empty() {
        return format!("No web_search results found for `{query}`.");
    }

    let mut out = format!("web_search results for `{query}`:\n");
    for (index, result) in results.iter().enumerate() {
        let number = index + 1;
        out.push_str(&format!(
            "\n{number}. {}\n   URL: {}\n   Snippet: {}\n",
            result.title, result.url, result.snippet
        ));
    }
    out
}

fn parse_duckduckgo_html(html: &str, limit: usize) -> Vec<SearchResult> {
    let mut results = Vec::new();
    let mut cursor = html;
    while results.len() < limit {
        let Some(link_pos) = cursor.find("class=\"result__a\"") else {
            break;
        };
        cursor = &cursor[link_pos..];
        let Some(href_start) = cursor.find("href=\"").map(|idx| idx + "href=\"".len()) else {
            break;
        };
        let href_rest = &cursor[href_start..];
        let Some(href_end) = href_rest.find('"') else {
            break;
        };
        let raw_url = &href_rest[..href_end];
        let Some(title_start) = href_rest[href_end..]
            .find('>')
            .map(|idx| href_end + idx + 1)
        else {
            break;
        };
        let title_rest = &href_rest[title_start..];
        let Some(title_end) = title_rest.find("</a>") else {
            break;
        };
        let title = clean_html_text(&title_rest[..title_end]);
        cursor = &title_rest[title_end..];
        let snippet = extract_following_snippet(cursor);
        let url = normalize_duckduckgo_url(raw_url);
        if !title.is_empty() && !url.is_empty() {
            results.push(SearchResult {
                title,
                url,
                snippet,
            });
        }
    }
    results
}

fn extract_following_snippet(fragment: &str) -> String {
    let Some(snippet_pos) = fragment.find("class=\"result__snippet\"") else {
        return String::new();
    };
    let snippet_fragment = &fragment[snippet_pos..];
    let Some(start) = snippet_fragment.find('>').map(|idx| idx + 1) else {
        return String::new();
    };
    let Some(end) = snippet_fragment[start..].find("</a>") else {
        return String::new();
    };
    clean_html_text(&snippet_fragment[start..start + end])
}

fn normalize_duckduckgo_url(raw_url: &str) -> String {
    let decoded = decode_html_entities(raw_url);
    let parse_target = if decoded.starts_with("//") {
        format!("https:{decoded}")
    } else {
        decoded.clone()
    };
    if let Ok(parsed) = Url::parse(&parse_target)
        && let Some(uddg) = parsed
            .query_pairs()
            .find_map(|(key, value)| (key == "uddg").then_some(value.into_owned()))
    {
        return uddg;
    }
    decoded
}

fn clean_html_text(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut in_tag = false;
    for ch in input.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    decode_html_entities(&out)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn decode_html_entities(input: &str) -> String {
    input
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_duckduckgo_html_results() {
        let html = r#"
            <a rel="nofollow" class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fchat%3Fa%3D1&amp;rut=abc">Example <b>Chat</b> UI</a>
            <a class="result__snippet" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com">A &amp; B static chat demo</a>
        "#;

        assert_eq!(
            parse_duckduckgo_html(html, 5),
            vec![SearchResult {
                title: "Example Chat UI".to_string(),
                url: "https://example.com/chat?a=1".to_string(),
                snippet: "A & B static chat demo".to_string(),
            }]
        );
    }

    #[test]
    fn detects_duckduckgo_bot_challenge() {
        assert!(is_duckduckgo_bot_challenge(
            r#"<div class="anomaly-modal__title">Unfortunately, bots use DuckDuckGo too.</div>"#
        ));
    }

    #[test]
    fn reads_searxng_url_from_config() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let config_path = temp_dir.path().join("config.toml");
        std::fs::write(
            &config_path,
            r#"
[web_search_config]
searxng_url = "http://localhost:18080"
"#,
        )
        .expect("write config");

        assert_eq!(
            searxng_url_from_config(&config_path),
            Some("http://localhost:18080".to_string())
        );
    }
}
