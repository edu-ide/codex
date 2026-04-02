// commands

use crate::SharedState;
use crate::relay_server::RelayEvent;
use crate::telegram_client;
use crate::{RELAY_DESKTOP_READY_TIMEOUT_MS, relay_wait_timeout_from_payload};
use serde_json::json;
use tracing::info;
pub async fn handle_relay_bootstrap(
    ctx: &SharedState,
    cmd: &crate::relay_server::RelayCommand,
    _client_id: u32,
    maybe_respond: impl Fn(Option<&str>, serde_json::Value, Option<String>),
) {
    let timeout = relay_wait_timeout_from_payload(&cmd.payload, RELAY_DESKTOP_READY_TIMEOUT_MS);
    let maybe_cx = ctx.infra.relay_conductor_cx.wait_for(timeout).await;
    if maybe_cx.is_some() {
        maybe_respond(
            cmd.request_id.as_deref(),
            json!({
                "desktop_ready": true,
                "wait_ms": timeout.as_millis() as u64
            }),
            None,
        );
    } else {
        maybe_respond(
            cmd.request_id.as_deref(),
            json!({
                "desktop_ready": false,
                "wait_ms": timeout.as_millis() as u64
            }),
            Some(format!(
                "desktop client connection is not ready (waited {}ms)",
                timeout.as_millis()
            )),
        );
    }
}

pub async fn handle_reload_channels(
    ctx: &SharedState,
    cmd: &crate::relay_server::RelayCommand,
    client_id: u32,
    _maybe_respond: impl Fn(Option<&str>, serde_json::Value, Option<String>),
) {
    info!("[Relay] reload_channels command");
    let ctx_clone = ctx.clone();
    let req_id = cmd.request_id.clone();
    let sender_id = client_id;
    tokio::spawn(async move {
        // 1. Stop existing Telegram bot
        ctx_clone.infra.relay_state.clear_telegram().await;
        info!("[reload_channels] Telegram bot stopped");

        // 2. Re-read settings
        let tg_settings = ctx_clone
            .infra
            .settings_store
            .get()
            .channels
            .telegram
            .clone();

        // 3. Restart if enabled
        let active = if tg_settings.enabled && !tg_settings.bot_token.is_empty() {
            let (tg_event_tx, tg_event_rx) = tokio::sync::mpsc::channel::<String>(256);
            ctx_clone
                .infra
                .relay_state
                .set_telegram_tx(tg_event_tx)
                .await;

            let handle = telegram_client::start(
                tg_settings,
                ctx_clone.infra.relay_state.command_tx.clone(),
                ctx_clone.infra.relay_tx.clone(),
                ctx_clone.infra.brain.sessions().clone(),
                ctx_clone.infra.notification_store.clone(),
                ctx_clone.infra.settings_store.clone(),
                tg_event_rx,
                ctx_clone
                    .infra
                    .ilhae_dir
                    .join("ws")
                    .to_string_lossy()
                    .to_string(),
            );
            ctx_clone
                .infra
                .relay_state
                .set_telegram_handle(handle)
                .await;
            info!("[reload_channels] Telegram bot restarted");
            true
        } else {
            info!("[reload_channels] Telegram bot disabled");
            false
        };

        // Respond to the client
        if let Some(rid) = req_id {
            let result = json!({
                "telegram_active": active,
                "message": if active { "Telegram 봇이 재시작되었습니다" } else { "Telegram 봇이 비활성화되었습니다" }
            });
            ctx_clone
                .infra
                .relay_state
                .send_to_client(
                    sender_id.into(),
                    &serde_json::to_string(&RelayEvent::CommandResponse {
                        request_id: rid,
                        result,
                        error: None,
                    })
                    .unwrap_or_default(),
                )
                .await;
        }
    });
}

pub async fn handle_relay_webhook(
    ctx: &SharedState,
    cmd: &crate::relay_server::RelayCommand,
    _client_id: u32,
    maybe_respond: impl Fn(Option<&str>, serde_json::Value, Option<String>),
) {
    // Central relay server forwards a webhook interaction
    let channel = cmd
        .payload
        .get("channel")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let permission_id = cmd
        .payload
        .get("permission_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let option_id = cmd
        .payload
        .get("option_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let user_info = cmd
        .payload
        .get("user")
        .and_then(|v| v.as_str())
        .unwrap_or("external");

    if !permission_id.is_empty() && !option_id.is_empty() {
        info!(
            "[Relay] Received remote webhook resolve: {}/{} from {}",
            permission_id, option_id, channel
        );
        ctx.infra
            .approval_manager
            .resolve(
                permission_id,
                option_id.to_string(),
                Some(format!("{}:{}", channel, user_info)),
            )
            .await;

        maybe_respond(
            cmd.request_id.as_deref(),
            json!({ "status": "resolved" }),
            None,
        );
    } else {
        maybe_respond(
            cmd.request_id.as_deref(),
            serde_json::Value::Null,
            Some("invalid webhook payload".to_string()),
        );
    }
}

pub async fn handle_permission_response(
    ctx: &SharedState,
    cmd: &crate::relay_server::RelayCommand,
    client_id: u32,
    maybe_respond: impl Fn(Option<&str>, serde_json::Value, Option<String>),
) {
    let payload = &cmd.payload;
    let permission_id = payload
        .get("permission_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let option_id = payload
        .get("option_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if permission_id.is_empty() || option_id.is_empty() {
        maybe_respond(
            cmd.request_id.as_deref(),
            serde_json::Value::Null,
            Some("permission_id and option_id are required".to_string()),
        );
    } else {
        let resolved_by = format!("relay-{}", client_id);
        let ok = ctx
            .infra
            .approval_manager
            .resolve(permission_id, option_id.to_string(), Some(resolved_by))
            .await;
        if ok {
            maybe_respond(cmd.request_id.as_deref(), json!({"ok": true}), None);
        } else {
            maybe_respond(
                cmd.request_id.as_deref(),
                serde_json::Value::Null,
                Some("Permission request not found or expired".to_string()),
            );
        }
    }
}

pub async fn handle_team_status(
    ctx: &SharedState,
    cmd: &crate::relay_server::RelayCommand,
    _client_id: u32,
    maybe_respond: impl Fn(Option<&str>, serde_json::Value, Option<String>),
) {
    let sv = ctx.team.supervisor.read().await;
    let agents: Vec<_> = sv
        .processes
        .iter()
        .map(|(name, proc)| {
            let card = proc.cached_agent_card.as_ref();
            let description = card
                .and_then(|c| c.get("description").and_then(|v| v.as_str()))
                .unwrap_or("");
            let model = card
                .and_then(|c| c.get("model").and_then(|v| v.as_str()))
                .unwrap_or("");
            let provider = card
                .and_then(|c| c.get("provider").and_then(|v| v.as_str()))
                .unwrap_or("");
            let capabilities = card
                .and_then(|c| c.get("capabilities"))
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            json!({
                "name": name,
                "port": proc.port,
                "enabled": proc.enabled,
                "alive": proc.last_healthy.is_some(),
                "restart_count": proc.restart_count,
                "is_leader": proc.is_leader,
                "engine": proc.engine,
                "role": proc.role,
                "has_agent_card": proc.cached_agent_card.is_some(),
                "description": description,
                "model": model,
                "provider": provider,
                "capabilities": capabilities,
            })
        })
        .collect();
    let leader = sv
        .processes
        .iter()
        .find(|(_, p)| p.is_leader)
        .map(|(n, _)| n.clone())
        .unwrap_or_default();
    let settings = ctx.infra.settings_store.get();
    let result = json!({
        "agent_count": agents.len(),
        "leader": leader,
        "agents": agents,
        "team_mode": settings.agent.team_mode,
        "engine": settings.agent.command,
        "mock_mode": crate::mock_provider::is_mock_mode(),
    });
    maybe_respond(cmd.request_id.as_deref(), result, None);
}
