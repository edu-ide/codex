pub mod admin;
pub mod browser;
pub mod chat;
pub mod plugins;
pub mod schedules;
pub mod sessions;
pub mod settings;
pub mod system;

use crate::relay_server::{RelayCommandWithClient, RelayEvent};
use tracing::{info, warn};

pub async fn dispatch(ctx: &crate::SharedState, relay_cmd: RelayCommandWithClient) {
    let cmd = &relay_cmd.cmd;
    let client_id = relay_cmd.client_id;
    info!(
        "[Relay] Command from mobile (client {}): action={}",
        client_id, cmd.action
    );

    let maybe_respond =
        |request_id: Option<&str>, result: serde_json::Value, error: Option<String>| {
            if let Some(rid) = request_id {
                let event = RelayEvent::CommandResponse {
                    request_id: rid.to_string(),
                    result,
                    error,
                };
                if let Ok(json) = serde_json::to_string(&event) {
                    let state = ctx.infra.relay_state.clone();
                    tokio::spawn(async move {
                        state
                            .send_to_client(client_id.try_into().unwrap(), &json)
                            .await;
                    });
                }
            }
        };

    match cmd.action.as_str() {
        "team_status" => {
            system::handle_team_status(ctx, cmd, client_id.try_into().unwrap(), maybe_respond).await
        }
        "team_presets" => {
            admin::handle_team_presets(ctx, cmd, client_id.try_into().unwrap(), maybe_respond).await
        }
        "team_save" => {
            admin::handle_team_save(ctx, cmd, client_id.try_into().unwrap(), maybe_respond).await
        }
        "chat_message" => {
            chat::handle_chat_message(ctx, cmd, client_id.try_into().unwrap(), maybe_respond).await
        }
        "settings_get" => {
            settings::handle_settings_get(ctx, cmd, client_id.try_into().unwrap(), maybe_respond)
                .await
        }
        "settings_set" => {
            settings::handle_settings_set(ctx, cmd, client_id.try_into().unwrap(), maybe_respond)
                .await
        }
        "codex_config_get" => {
            settings::handle_codex_config_get(
                ctx,
                cmd,
                client_id.try_into().unwrap(),
                maybe_respond,
            )
            .await
        }
        "codex_config_set" => {
            settings::handle_codex_config_set(
                ctx,
                cmd,
                client_id.try_into().unwrap(),
                maybe_respond,
            )
            .await
        }
        "task_list" => {
            schedules::handle_task_list(ctx, cmd, client_id.try_into().unwrap(), maybe_respond)
                .await
        }
        "task_create" => {
            schedules::handle_task_create(ctx, cmd, client_id.try_into().unwrap(), maybe_respond)
                .await
        }
        "task_update" => {
            schedules::handle_task_update(ctx, cmd, client_id.try_into().unwrap(), maybe_respond)
                .await
        }
        "task_delete" => {
            schedules::handle_task_delete(ctx, cmd, client_id.try_into().unwrap(), maybe_respond)
                .await
        }
        "session_load" => {
            sessions::handle_session_load(ctx, cmd, client_id.try_into().unwrap(), maybe_respond)
                .await
        }
        "ilhae/app/timeline/read" => {
            sessions::handle_session_load(ctx, cmd, client_id.try_into().unwrap(), maybe_respond)
                .await
        }
        "session_timeline" => {
            sessions::handle_session_timeline(
                ctx,
                cmd,
                client_id.try_into().unwrap(),
                maybe_respond,
            )
            .await
        }
        "session_list" => {
            sessions::handle_session_list(ctx, cmd, client_id.try_into().unwrap(), maybe_respond)
                .await
        }
        "ilhae/app/session/list" => {
            sessions::handle_session_list(ctx, cmd, client_id.try_into().unwrap(), maybe_respond)
                .await
        }
        "session_search" => {
            sessions::handle_session_search(ctx, cmd, client_id.try_into().unwrap(), maybe_respond)
                .await
        }
        "ilhae/app/session/search" => {
            sessions::handle_session_search(ctx, cmd, client_id.try_into().unwrap(), maybe_respond)
                .await
        }
        "session_delete" => {
            sessions::handle_session_delete(ctx, cmd, client_id.try_into().unwrap(), maybe_respond)
                .await
        }
        "ilhae/app/session/delete" => {
            sessions::handle_session_delete(ctx, cmd, client_id.try_into().unwrap(), maybe_respond)
                .await
        }
        "session_create" => {
            sessions::handle_session_create(ctx, cmd, client_id.try_into().unwrap(), maybe_respond)
                .await
        }
        "ilhae/app/session/create" => {
            sessions::handle_session_create(ctx, cmd, client_id.try_into().unwrap(), maybe_respond)
                .await
        }
        "browser_launch" => {
            browser::handle_browser_launch(ctx, cmd, client_id.try_into().unwrap(), maybe_respond)
                .await
        }
        "browser_stop" => {
            browser::handle_browser_stop(ctx, cmd, client_id.try_into().unwrap(), maybe_respond)
                .await
        }
        "plugin_list" => {
            plugins::handle_plugin_list(ctx, cmd, client_id.try_into().unwrap(), maybe_respond)
                .await
        }
        "plugin_toggle" => {
            plugins::handle_plugin_toggle(ctx, cmd, client_id.try_into().unwrap(), maybe_respond)
                .await
        }
        "relay_bootstrap" => {
            system::handle_relay_bootstrap(ctx, cmd, client_id.try_into().unwrap(), maybe_respond)
                .await
        }
        "reload_channels" => {
            system::handle_reload_channels(ctx, cmd, client_id.try_into().unwrap(), maybe_respond)
                .await
        }
        "relay_webhook" => {
            system::handle_relay_webhook(ctx, cmd, client_id.try_into().unwrap(), maybe_respond)
                .await
        }
        "permission_response" => {
            system::handle_permission_response(
                ctx,
                cmd,
                client_id.try_into().unwrap(),
                maybe_respond,
            )
            .await
        }
        _ => {
            warn!("[Relay] Unknown command action: {}", cmd.action);
            maybe_respond(
                cmd.request_id.as_deref(),
                serde_json::Value::Null,
                Some(format!("unknown action: {}", cmd.action)),
            );
        }
    }
}
