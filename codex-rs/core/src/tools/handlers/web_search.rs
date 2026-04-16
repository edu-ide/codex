use crate::function_tool::FunctionCallError;
use crate::tools::context::FunctionToolOutput;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::handlers::parse_arguments;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;
use serde::Deserialize;
use web_search_rs::WebSearchClient;

use codex_protocol::config_types::WebSearchConfig;

pub struct WebSearchHandler {
    config: Option<WebSearchConfig>,
}

impl WebSearchHandler {
    pub fn new(config: Option<WebSearchConfig>) -> Self {
        Self { config }
    }
}

#[derive(Deserialize)]
struct WebSearchArgs {
    query: String,
}

impl ToolHandler for WebSearchHandler {
    type Output = FunctionToolOutput;

    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn handle(&self, invocation: ToolInvocation) -> Result<Self::Output, FunctionCallError> {
        let ToolInvocation { payload, .. } = invocation;
        let arguments = match payload {
            ToolPayload::Function { arguments } => arguments,
            _ => return Err(FunctionCallError::RespondToModel("unsupported payload".to_string())),
        };

        let args: WebSearchArgs = parse_arguments(&arguments)?;
        if args.query.trim().is_empty() {
            return Err(FunctionCallError::RespondToModel("Query cannot be empty".to_string()));
        }

        let (searxng_url, fallback, engine) = match &self.config {
            Some(cfg) => {
                let engine = match cfg.engine {
                    Some(codex_protocol::config_types::WebSearchEngine::Searxng) => web_search_rs::WebSearchEngine::Searxng,
                    Some(codex_protocol::config_types::WebSearchEngine::Duckduckgo) => web_search_rs::WebSearchEngine::Duckduckgo,
                    _ => web_search_rs::WebSearchEngine::Auto,
                };
                (cfg.searxng_url.clone(), cfg.use_duckduckgo_fallback, engine)
            },
            None => (None, None, web_search_rs::WebSearchEngine::Auto),
        };
        let client = WebSearchClient::new(searxng_url, fallback.unwrap_or(true), engine);
        match client.perform_full_search(&args.query, 3).await {
            Ok(content) => Ok(FunctionToolOutput::from_text(content, Some(true))),
            Err(e) => Err(FunctionCallError::RespondToModel(format!("Search failed: {}", e))),
        }
    }
}
