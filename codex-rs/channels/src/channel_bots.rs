//! Channel bot startup helpers — start Telegram / Discord / Slack / Kakao / LINE / WhatsApp
//! bots based on settings stored in SharedState.

use std::sync::Arc;
use tracing::info;

use crate::shared_state::SharedState;

/// Start the Telegram bot if enabled in settings.
pub async fn start_telegram_if_enabled(shared: &Arc<SharedState>) {
    let tg_settings = shared.infra.settings_store.get().channels.telegram.clone();
    if tg_settings.enabled && !tg_settings.bot_token.is_empty() {
        let (tg_event_tx, tg_event_rx) = tokio::sync::mpsc::channel::<String>(256);
        shared.infra.relay_state.set_telegram_tx(tg_event_tx).await;

        let handle = crate::telegram_client::start(
            tg_settings,
            shared.infra.relay_state.command_tx.clone(),
            shared.infra.relay_tx.clone(),
            shared.infra.brain.sessions().clone(),
            shared.infra.notification_store.clone(),
            shared.infra.settings_store.clone(),
            tg_event_rx,
            shared
                .infra
                .ilhae_dir
                .join("ws")
                .to_string_lossy()
                .to_string(),
        );
        shared.infra.relay_state.set_telegram_handle(handle).await;
        info!("[Telegram] Bot started");
    } else {
        info!("[Telegram] Bot disabled (telegram.enabled=false or bot_token empty)");
    }
}

/// Start the Discord bot if enabled in settings.
pub async fn start_discord_if_enabled(shared: &Arc<SharedState>) {
    let discord_settings = shared.infra.settings_store.get().channels.discord.clone();
    if discord_settings.enabled && !discord_settings.bot_token.is_empty() {
        crate::discord_client::start(
            discord_settings,
            shared.infra.settings_store.clone(),
            shared.infra.approval_manager.clone(),
            shared.infra.relay_tx.clone(),
        )
        .await;
        info!("[Discord] Bot started");
    } else {
        info!("[Discord] Bot disabled (discord.enabled=false or bot_token empty)");
    }
}

/// Start the Slack bot if enabled in settings.
pub async fn start_slack_if_enabled(shared: &Arc<SharedState>) {
    let slack_settings = shared.infra.settings_store.get().channels.slack.clone();
    if slack_settings.enabled && !slack_settings.api_token.is_empty() {
        crate::slack_client::start(
            slack_settings,
            shared.infra.approval_manager.clone(),
            shared.infra.relay_tx.clone(),
        )
        .await;
        info!("[Slack] Bot started");
    } else {
        info!("[Slack] Bot disabled (slack.enabled=false or api_token empty)");
    }
}

/// Start the KakaoTalk bot if enabled in settings.
pub async fn start_kakao_if_enabled(shared: &Arc<SharedState>) {
    let kakao_settings = shared.infra.settings_store.get().channels.kakao.clone();
    if kakao_settings.enabled && !kakao_settings.app_key.is_empty() {
        crate::kakao_client::start(
            kakao_settings,
            shared.infra.approval_manager.clone(),
            shared.infra.relay_tx.clone(),
        )
        .await;
        info!("[Kakao] Bot started");
    }
}

/// Start the LINE bot if enabled in settings.
pub async fn start_line_if_enabled(shared: &Arc<SharedState>) {
    let line_settings = shared.infra.settings_store.get().channels.line.clone();
    if line_settings.enabled && !line_settings.api_token.is_empty() {
        crate::line_client::start(
            line_settings,
            shared.infra.approval_manager.clone(),
            shared.infra.relay_tx.clone(),
        )
        .await;
        info!("[LINE] Bot started");
    }
}

/// Start the WhatsApp bot if enabled in settings.
pub async fn start_whatsapp_if_enabled(shared: &Arc<SharedState>) {
    let wa_settings = shared.infra.settings_store.get().channels.whatsapp.clone();
    if wa_settings.enabled && !wa_settings.api_token.is_empty() {
        crate::whatsapp_client::start(
            wa_settings,
            shared.infra.approval_manager.clone(),
            shared.infra.relay_tx.clone(),
        )
        .await;
        info!("[WhatsApp] Bot started");
    }
}
