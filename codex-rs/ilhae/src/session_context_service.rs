use std::sync::Arc;

use agent_client_protocol_schema::{ContentBlock, TextContent};
use brain_rs::BrainService;
use brain_session_rs::session_store::SessionInfo;
use moka::sync::Cache;
use tokio::sync::RwLock;
use tracing::info;

use crate::{
    ARTIFACT_INSTRUCTION, infer_agent_id_from_command,
    session_persistence_service::SessionRegistryService, settings_store::SettingsStore,
};

pub struct PreparedSessionPromptContext {
    pub current_agent_id: String,
    pub session_info: Option<SessionInfo>,
    pub prompt_blocks: Vec<ContentBlock>,
}

pub struct SessionPromptContextDeps {
    pub brain: Arc<BrainService>,
    pub settings_store: Arc<SettingsStore>,
    pub context_prefix: String,
    pub reverse_session_map: Option<Arc<Cache<String, String>>>,
    pub active_session_id: Option<Arc<RwLock<String>>>,
}

pub fn extract_user_text(prompt: &[ContentBlock]) -> String {
    prompt
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) if !text.text.starts_with("__MCP_WIDGET_CTX__:") => {
                Some(text.text.clone())
            }
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub async fn prepare_session_prompt_context(
    deps: &SessionPromptContextDeps,
    session_id: &str,
    is_subagent: bool,
) -> anyhow::Result<PreparedSessionPromptContext> {
    let settings = deps.settings_store.clone();
    let current_agent_id = infer_agent_id_from_command(&settings.get().agent.command);

    if let Some(active_session_id) = &deps.active_session_id {
        let db_sid = deps
            .reverse_session_map
            .as_ref()
            .and_then(|reverse_map| reverse_map.get(session_id));
        let effective_sid = db_sid.unwrap_or_else(|| session_id.to_string());
        *active_session_id.write().await = effective_sid;
    }

    let session_info = if is_subagent {
        None
    } else {
        SessionRegistryService::get_session_info(&deps.brain, session_id)?
    };
    let is_new = session_info.is_none();

    if is_new && !is_subagent {
        let ensure_agent_id = if settings.get().agent.team_mode {
            "team".to_string()
        } else {
            current_agent_id.clone()
        };
        info!("New session detected via PromptRequest: {}", session_id);
        let _ = SessionRegistryService::ensure_session(
            &deps.brain,
            session_id,
            &ensure_agent_id,
            &ensure_agent_id,
            "/",
        );
    }

    let should_inject_context = !is_subagent
        && (is_new
            || session_info
                .as_ref()
                .is_some_and(|info| info.message_count == 0));

    let mut prompt_blocks = Vec::new();
    if should_inject_context {
        info!("Injecting context for session: {}", session_id);

        let locale_val = settings.get_value("ui.locale");
        let locale_str = locale_val.as_str().unwrap_or("");
        if !locale_str.is_empty() {
            let lang_name = match locale_str {
                "ko" => "Korean (한국어)",
                "en" => "English",
                "ja" => "Japanese (日本語)",
                "zh" => "Chinese (中文)",
                _ => locale_str,
            };
            let locale_instruction = format!(
                "\n<system_directive priority=\"high\">\n\
                 RESPONSE LANGUAGE: You MUST respond in {}.\n\
                 All artifacts (task, plan, walkthrough) MUST also be written in {}.\n\
                 Use the user's preferred language consistently throughout all outputs.\n\
                 </system_directive>\n",
                lang_name, lang_name
            );
            prompt_blocks.push(ContentBlock::Text(TextContent::new(locale_instruction)));
        }

        let vault_dir = deps.brain.vault_dir();
        let session_artifact_dir = vault_dir.join("sessions").join(session_id);
        let _ = std::fs::create_dir_all(&session_artifact_dir);
        prompt_blocks.push(ContentBlock::Text(TextContent::new(
            ARTIFACT_INSTRUCTION.to_string(),
        )));
        prompt_blocks.push(ContentBlock::Text(TextContent::new(
            deps.context_prefix.clone(),
        )));
    }

    Ok(PreparedSessionPromptContext {
        current_agent_id,
        session_info,
        prompt_blocks,
    })
}
