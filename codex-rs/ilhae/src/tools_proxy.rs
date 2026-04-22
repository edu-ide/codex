//! ToolsProxy — MCP Tool Registration (Built-in + Browser tools).
//!
//! Contains `with_mcp_server` registrations for ilhae-tools (20 tools)
//! and browser-tools (19 tools via browser-use-rs), and team-tools (3 tools).

use sacp::{Agent, Conductor, ConnectTo, ConnectionTo, Proxy};
use std::sync::Arc;

use crate::register_browser_tools;
#[allow(unused_imports)]
use crate::{
    ArtifactEditInput, ArtifactGetInput, ArtifactListInput, ArtifactSaveInput, EmptyInput, IdInput,
    MemoryReadInput, MemoryToolDreamAnalyzeInput, MemoryToolDreamApplyInput,
    MemoryToolDreamPreviewInput, MemoryToolDreamPromoteInput, MemoryToolExtractInput,
    MemoryToolForgetInput, MemoryToolListInput, MemoryToolPinInput, MemoryToolPromoteInput,
    MemoryToolSearchInput, MemoryToolStatsInput, MemoryToolStoreInput, MemoryWriteInput,
    SessionIdInput, SessionRenameInput, TaskAddHistoryInput, TaskCreateInput, TaskUpdateInput,
    TeamDelegateInput, TeamProposeInput, UiNotifyInput, tool_to_plugin_id,
};

// ─── ToolsProxy state ──────────────────────────────────────────────────

pub struct ToolsProxy {
    pub state: Arc<crate::SharedState>,
}

impl ConnectTo<Conductor> for ToolsProxy {
    async fn connect_to(self, conductor: impl ConnectTo<Proxy>) -> Result<(), sacp::Error> {
        let s = self.state;
        let sub_handle;

        let base_builder = Proxy
            .builder()
            .name("tools-proxy")
            // ═══ Built-in MCP Tools (20 tools) ═══
            .with_mcp_server({
                let brain = s.infra.brain.clone();
                let bt_settings = s.infra.settings_store.clone();
                let notify_relay_tx = s.infra.relay_tx.clone();
                let notif_store = s.infra.notification_store.clone();
                let connection_sessions = s.sessions.connection_sessions.clone();

                let builder = sacp::mcp_server::McpServer::<Conductor, _>::builder(
                    "ilhae-tools".to_string(),
                )
                .instructions(
                    "일해 프록시 내장 도구. 세션·메모리·크론·미션·할일·UI 알림을 관리합니다.",
                );
                sub_handle = builder.subscription_handle();

                let builder = crate::register_memory_tools!(builder, brain, bt_settings);
                let builder = crate::register_knowledge_tools!(
                    builder,
                    s.infra.ilhae_dir.clone(),
                    s.infra.settings_store.clone()
                );
                let builder = crate::register_session_tools!(builder, brain, bt_settings);
                let builder = crate::register_task_tools!(builder, brain, bt_settings);
                let builder = crate::register_artifact_tools!(
                    builder,
                    brain,
                    bt_settings,
                    connection_sessions
                );
                let builder = crate::register_misc_tools!(
                    builder,
                    brain,
                    bt_settings,
                    notify_relay_tx,
                    notif_store,
                    s.clone()
                );

                builder.build()
            });

        // Closure to handle the connection and watcher
        let s_clone = s.clone();
        let sub_handle_clone = sub_handle.clone();
        let connect_handler = move |cx: ConnectionTo<Conductor>| {
            let s_conn = s_clone.clone();
            let subs = sub_handle_clone.clone();
            async move {
                s_conn.infra.relay_conductor_cx.try_add(cx.clone()).await;

                // ═══ MCP Resource Change Watcher ═══
                let cx_notif = cx.clone();
                tokio::spawn(async move {
                    let mut rx = crate::memory_provider::subscribe_changes();
                    loop {
                        match rx.recv().await {
                            Ok(event) => {
                                if subs.is_subscribed(&event.uri)
                                    || subs.is_subscribed("ilhae://memory/all")
                                {
                                    tracing::info!(
                                        "[MCP] Resource updated: {}, notifying subscribers",
                                        event.uri
                                    );
                                    if let Ok(notif) = sacp::UntypedMessage::new(
                                        "notifications/resources/updated",
                                        serde_json::json!({ "uri": event.uri }),
                                    ) {
                                        let _ = cx_notif.send_notification_to(Agent, notif);
                                    }
                                }
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                                tracing::warn!(
                                    "[MCP] Resource watcher lagged, missed {} events",
                                    n
                                );
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                                tracing::debug!(
                                    "[MCP] Resource change channel closed, stopping watcher"
                                );
                                break;
                            }
                        }
                    }
                });

                std::future::pending::<Result<(), sacp::Error>>().await
            }
        };

        if std::env::var("ILHAE_DREAM_MODE").is_ok() {
            // Dream mode: ilhae-tools + team-tools, NO browser tools
            let final_builder = crate::with_team_server!(base_builder, s);
            final_builder.connect_with(conductor, connect_handler).await
        } else {
            // Normal mode: browser tools + team servers
            let b_builder = base_builder.with_mcp_server({
                let session_handle = s.infra.browser_mgr.get_session();
                let bmgr = s.infra.browser_mgr.clone();
                let bsettings = s.infra.settings_store.clone();
                let b = sacp::mcp_server::McpServer::<Conductor, _>::builder("browser-tools".to_string())
                    .instructions("Browser automation tools for web navigation, interaction, and content extraction via CDP.");
                let b = register_browser_tools!(b, session_handle, bmgr, bsettings);
                b.build()
            });

            let final_builder = crate::with_team_server!(b_builder, s);
            final_builder.connect_with(conductor, connect_handler).await
        }
    }
}
