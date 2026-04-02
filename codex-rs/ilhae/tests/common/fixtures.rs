//! Shared test fixtures for session creation and message persistence.
//!
//! Reduces boilerplate in team/artifact tests that need session + message setup.

#![allow(dead_code)]

use ilhae_proxy::session_store::SessionStore;
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;

/// Create a SessionStore with optional BrainSessionWriter attached.
pub fn create_store(dir: &PathBuf, with_brain: bool) -> SessionStore {
    let mut store = SessionStore::new(dir).expect("SessionStore open failed");
    if with_brain {
        let brain_dir = dir.join("brain");
        let writer = brain_session_rs::brain_session_writer::BrainSessionWriter::new(&brain_dir);
        store.set_brain_writer(writer);
    }
    store
}

/// Create a SessionStore wrapped in Arc (for concurrent/proxy tests).
pub fn create_arc_store(dir: &PathBuf) -> Arc<SessionStore> {
    Arc::new(SessionStore::new(dir).expect("SessionStore open failed"))
}

/// Create a team session with channel and return session_id.
pub fn create_team_session(store: &SessionStore, agent: &str) -> String {
    let session_id = uuid::Uuid::new_v4().to_string();
    store
        .ensure_session_with_channel(&session_id, agent, ".", "team")
        .expect("ensure_session failed");
    session_id
}

/// Create a desktop session and return session_id.
pub fn create_desktop_session(store: &SessionStore, agent: &str) -> String {
    let session_id = uuid::Uuid::new_v4().to_string();
    store
        .ensure_session_with_channel(&session_id, agent, ".", "desktop")
        .expect("ensure_session failed");
    session_id
}

/// Persist a user message with content blocks.
pub fn persist_user_message(store: &SessionStore, session_id: &str, agent: &str, prompt: &str) {
    let content_blocks = serde_json::to_string(&vec![json!({"type": "text", "text": prompt})])
        .unwrap_or_else(|_| "[]".to_string());

    store
        .add_full_message_with_blocks(
            session_id,
            "user",
            prompt,
            agent,
            "",
            "[]",
            &content_blocks,
            0,
            0,
            0,
            0,
        )
        .expect("Failed to persist user message");
}

/// Persist an assistant message with content blocks.
pub fn persist_assistant_message(
    store: &SessionStore,
    session_id: &str,
    agent: &str,
    text: &str,
    thinking: &str,
) {
    let content_blocks = serde_json::to_string(&vec![json!({"type": "text", "text": text})])
        .unwrap_or_else(|_| "[]".to_string());

    store
        .add_full_message_with_blocks(
            session_id,
            "assistant",
            text,
            agent,
            thinking,
            "[]",
            &content_blocks,
            0,
            0,
            0,
            0,
        )
        .expect("Failed to persist assistant message");
}
