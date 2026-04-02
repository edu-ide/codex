//! Unified memory data layer.
//!
//! Thin wrapper around `brain_rs::memory_sections` (SSoT).
//! Adds Proxy-specific concerns: broadcast change events and A2A context injection.
//!
//! Used by:
//! - MCP Tools (memory_read)
//! - MCP Resources (ilhae://memory/*)
//! - MCP Resource Templates (ilhae://memory/{section})
//! - Auto-injection (build_dynamic_instructions — Tier 1)

use std::sync::OnceLock;
use tokio::sync::broadcast;

/// Event emitted when a memory resource changes.
#[derive(Clone, Debug)]
pub struct ResourceChangeEvent {
    /// The MCP resource URI that changed, e.g. "ilhae://memory/system"
    pub uri: String,
}

/// Global broadcast channel for resource change events.
/// Capacity of 16 is enough since events are consumed quickly by background schedules.
static CHANGE_TX: OnceLock<broadcast::Sender<ResourceChangeEvent>> = OnceLock::new();

fn change_sender() -> &'static broadcast::Sender<ResourceChangeEvent> {
    CHANGE_TX.get_or_init(|| broadcast::channel(16).0)
}

/// Subscribe to resource change events.
/// Returns a receiver that will get notified when any memory resource is written.
pub fn subscribe_changes() -> broadcast::Receiver<ResourceChangeEvent> {
    change_sender().subscribe()
}

/// Emit a resource change event (called internally after successful writes).
fn emit_change(section: &str) {
    let uri = format!("ilhae://memory/{}", section);
    let _ = change_sender().send(ResourceChangeEvent { uri });
}

/// Resolve the data_dir used by brain-rs.
fn data_dir() -> std::path::PathBuf {
    brain_rs::BrainService::resolve_data_dir()
}

// ─── Delegated to brain_rs::memory_sections (SSoT) ──────────────────────────

/// Read a single global memory file (SYSTEM, IDENTITY, SOUL, USER).
pub fn read_global(name: &str) -> String {
    brain_rs::memory_sections::read_global(&data_dir(), name)
}

/// Read all global memory files, sorted alphabetically.
/// Returns Vec<(section_name, content)>.
pub fn read_all_global() -> Vec<(String, String)> {
    brain_rs::memory_sections::read_all_global(&data_dir())
}

/// Read daily log entries (most recent first, up to `limit`).
pub fn read_daily(limit: usize) -> String {
    brain_rs::memory_sections::read_daily(&data_dir(), limit)
}

/// Read project-specific memory files.
pub fn read_project() -> String {
    brain_rs::memory_sections::read_project(&data_dir())
}

/// Read a specific section by name.
/// Valid sections: system, identity, soul, user, daily, project, all.
pub fn read_section(section: &str) -> Result<String, String> {
    brain_rs::memory_sections::read_section(&data_dir(), None, section)
}

/// Write to a global memory section file.
pub fn write_section(section: &str, content: &str) -> Result<String, String> {
    let result = brain_rs::memory_sections::write_section(&data_dir(), section, content)?;
    // Emit resource change event for MCP subscription notifications
    emit_change(section);
    Ok(result)
}

// ── A2A Context Bridge ──────────────────────────────────────────────────

/// Build core identity context for team agent delegation.
///
/// Reads SYSTEM, IDENTITY, SOUL, USER from global memory and formats them
/// as a context block that can be prepended to A2A task queries.
pub fn build_team_context() -> String {
    let sections = read_all_global();
    if sections.is_empty() {
        return String::new();
    }

    let mut ctx = String::new();
    for (name, content) in &sections {
        let name_lower = name.to_lowercase();
        if matches!(name_lower.as_str(), "system" | "identity" | "soul" | "user") {
            ctx.push_str(&format!("## {}\n{}\n\n", name.to_uppercase(), content));
        }
    }
    ctx
}

/// Enrich a task query with memory context for team agent delegation.
///
/// Wraps core identity context in `<agent_context>` tags and prepends it
/// to the original query. Optionally includes the leader's current working
/// directory so the team agent operates in the same context.
pub fn inject_context(query: &str, cwd: Option<&str>) -> String {
    let ctx = build_team_context();
    let cwd_section = cwd
        .filter(|c| !c.is_empty())
        .map(|c| format!("## CWD\n{}\n\n", c))
        .unwrap_or_default();
    if ctx.is_empty() && cwd_section.is_empty() {
        return query.to_string();
    }
    format!(
        "<agent_context>\n{}{}</agent_context>\n\n{}",
        cwd_section,
        ctx.trim(),
        query
    )
}
