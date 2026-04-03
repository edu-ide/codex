#![recursion_limit = "512"]

// ── Extracted modules (testable via lib crate) ──
pub mod builtins;
pub mod config;
pub mod process_lifecycle;
pub mod shared_state;

// ── Hexagonal architecture: ports & adapters ──
pub mod adapters;
pub mod port_config;
pub mod ports;

// ── Core domain modules ──
pub mod admin_builtins;
pub mod admin_proxy;
pub mod capabilities;
pub mod context_proxy;
pub mod engine_env;
pub mod helpers;
pub mod hygiene_loop;
pub mod knowledge_loop;
pub mod mcp_manager;
pub mod memory_provider;

pub mod acp_ws_server;
pub mod config_builder;
pub mod mock_agent;
pub mod mock_provider;
pub mod notification_store;
pub mod persistence_proxy;
pub mod plugins;
pub mod relay_commands;
pub mod relay_proxy;
pub mod relay_server;
pub mod session_context_service;
pub mod session_persistence_service;
pub mod session_recall_service;
pub mod session_store;
pub mod session_timeline;
pub mod settings_store;
pub mod settings_types;
pub mod super_loop;
pub mod superpowers_skills;
pub mod team_parser;
pub mod team_timeline;
pub mod tools_proxy;
pub mod turn_accumulator;
pub mod types;

// ── Agent infrastructure ──
pub mod agent_pool;
pub mod agent_router;
pub mod process_supervisor;

// ── Protocol bridges ──
pub mod a2a_client;
pub mod a2a_transport;

// ── Channel integrations (kept here for backward compat, delegates to codex-channels) ──
#[allow(dead_code)]
pub mod approval_manager;
pub mod browser_manager;
pub mod browser_tools;
pub mod channel_bots;
#[allow(dead_code)]
pub mod channel_dock;
#[allow(dead_code)]
pub mod cron_service;
pub mod discord_client;
pub mod kakao_client;
pub mod line_client;
pub mod slack_client;
pub mod startup;
pub mod startup_phases;
#[allow(dead_code)]
pub mod telegram_client;
pub mod whatsapp_client;

// ── A2A Persistence Proxy (modularized) ──
pub mod a2a_persistence;

// ── Team orchestration ──
// NOTE: team_orchestration depends on main.rs-only functions
// (spawn_local_a2a_server, TeamSpawnEnv).
// When those functions are fully extracted, un-comment this:
// pub mod team_orchestration;

// ── Native CLI entry point ──
pub mod startup_main;
pub use startup_main::{
    BootstrappedIlhaeRuntime, bootstrap_ilhae_runtime, current_native_backend_capability_profile,
    current_native_backend_engine, ensure_native_runtime_for_cli, native_runtime_context,
    prepare_native_turn_inputs, prepare_session_turn_inputs, run_ilhae_proxy,
};

// ═══════════════════════════════════════════════════════════════
// Crate-root re-exports: mirror main.rs's `pub use helpers::*; use types::*; use plugins::*;`
// so that `crate::X` references in submodules resolve correctly for the lib crate.
// ═══════════════════════════════════════════════════════════════
pub use helpers::*;
pub use plugins::*;
pub use process_lifecycle::append_child_pid;
pub use relay_server::broadcast_event;
pub use shared_state::SharedState;
pub use types::*;

// ═══════════════════════════════════════════════════════════════
// SSoT: Artifact creation instructions — the ONLY source of truth.
// Used by: prompt.rs (solo), role_parser.rs (team GEMINI.md),
//          runner.rs (legacy team GEMINI.md), a2a_persistence/
// ═══════════════════════════════════════════════════════════════

/// Artifact system prompt injected into every LLM context.
/// Using `artifact_save` MCP tool (NOT write_to_file legacy).
pub const ARTIFACT_INSTRUCTION: &str = r#"
<system_directive priority="critical">
MANDATORY ARTIFACT CREATION RULES:

For ANY non-trivial task (more than a simple question), you MUST create ALL THREE artifacts:
1. Create a `task` artifact FIRST — a checklist with `- [ ]` items for each step
2. Create a `plan` artifact — your approach, files to change, and verification plan
3. Create a `walkthrough` artifact LAST — summarize what was done and validation results

HOW TO CREATE ARTIFACTS:
Use the `artifact_save` tool (NOT write_file or write_to_file) with these parameters:
- artifact_type: "task", "plan", "walkthrough", or "other"
- content: the markdown content of the artifact
- summary: a brief description of what the artifact contains

The tool automatically handles file paths and versioning. Do NOT:
- Use write_file, write_to_file or mkdir for artifacts
- Specify file paths for artifacts
- Write artifacts to /tmp or the working directory
- Output artifact content as plain text in chat — ALWAYS use the tool call

Do NOT skip ANY of the three artifacts — this is a system requirement, not optional.
</system_directive>
"#;

/// Short version for team GEMINI.md Rules section.
pub const ARTIFACT_RULES_SHORT: &str = r#"- **ALWAYS use `artifact_save` tool** to create/update artifact files. NEVER use `write_to_file` or `write_file` for artifacts.
- The `artifact_save` tool automatically handles file paths and versioning. Parameters:
  - `artifact_type`: "task", "plan", "walkthrough", or "other"
  - `content`: the markdown content (including YAML frontmatter)
  - `summary`: a brief description of what changed
- **NEVER output artifact content as plain text in chat.** Always use the `artifact_save` tool call."#;
