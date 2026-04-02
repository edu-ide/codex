use std::sync::Arc;
use std::time::Duration;

use agent_client_protocol_schema::{ContentBlock, TextContent};
use brain_knowledge_rs::memory_store;
use brain_rs::BrainService;
use tracing::{info, warn};

use crate::helpers::is_ilhae_native_agent_id;

const PINNED_FETCH_TIMEOUT_MS: u64 = 400;
const MEMORY_SEARCH_TIMEOUT_MS: u64 = 500;

pub struct SessionRecallDeps {
    pub brain: Arc<BrainService>,
}

pub async fn prepare_prompt_recall_blocks(
    deps: &SessionRecallDeps,
    session_id: &str,
    is_subagent: bool,
    current_agent_id: &str,
    user_text: &str,
) -> Vec<ContentBlock> {
    if is_subagent {
        return Vec::new();
    }

    let mut prompt_blocks = Vec::new();
    let brain = deps.brain.clone();

    if is_ilhae_native_agent_id(current_agent_id) && !user_text.trim().is_empty() {
        let query = user_text.to_string();
        let recall_session_id = session_id.to_string();
        let recall_brain = brain.clone();
        match tokio::time::timeout(
            Duration::from_millis(MEMORY_SEARCH_TIMEOUT_MS),
            tokio::task::spawn_blocking(move || {
                recall_brain.ilhae_priority_recall(&recall_session_id, &query, 5)
            }),
        )
        .await
        {
            Ok(Ok(Ok(recall_text))) => {
                if !recall_text.trim().is_empty() {
                    info!(
                        "Auto-recall: injecting priority recall context for session {}",
                        session_id
                    );
                    prompt_blocks.push(ContentBlock::Text(TextContent::new(recall_text)));
                }
            }
            Ok(Ok(Err(err))) => {
                warn!(
                    "Auto-recall search failed for session {}: {}",
                    session_id, err
                );
            }
            Ok(Err(err)) => {
                warn!(
                    "Auto-recall worker join failed for session {}: {}",
                    session_id, err
                );
            }
            Err(_) => {
                warn!(
                    "Auto-recall search timed out ({}ms) for session {}",
                    MEMORY_SEARCH_TIMEOUT_MS, session_id
                );
            }
        }
    }

    let pinned_brain = brain.clone();
    match tokio::time::timeout(
        Duration::from_millis(PINNED_FETCH_TIMEOUT_MS),
        tokio::task::spawn_blocking(move || pinned_brain.memory_list_pinned()),
    )
    .await
    {
        Ok(Ok(Ok(pinned))) => {
            let pinned_text = memory_store::format_pinned_for_prompt(&pinned);
            if !pinned_text.is_empty() {
                prompt_blocks.push(ContentBlock::Text(TextContent::new(pinned_text)));
            }
        }
        Ok(Ok(Err(err))) => {
            warn!(
                "Pinned memory fetch failed for session {}: {}",
                session_id, err
            );
        }
        Ok(Err(err)) => {
            warn!(
                "Pinned memory worker join failed for session {}: {}",
                session_id, err
            );
        }
        Err(_) => {
            warn!(
                "Pinned memory fetch timed out ({}ms) for session {}",
                PINNED_FETCH_TIMEOUT_MS, session_id
            );
        }
    }

    prompt_blocks
}
