use serenity::async_trait;
use serenity::builder::{CreateActionRow, CreateButton, CreateMessage};
use serenity::model::application::ButtonStyle;
use serenity::model::application::Interaction;
use serenity::model::channel::Message;
use serenity::model::gateway::Ready;
use serenity::prelude::*;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{error, info};

use codex_ilhae::approval_manager::ApprovalManager;
use codex_ilhae::relay_server::RelayEvent;
use codex_ilhae::settings_store::{DiscordSettings, SettingsStore};

pub struct Handler {
    pub settings_store: Arc<SettingsStore>,
    pub approval_mgr: Arc<ApprovalManager>,
}

#[async_trait]
impl EventHandler for Handler {
    async fn message(&self, ctx: Context, msg: Message) {
        if msg.author.bot {
            return;
        }

        let content = msg.content.trim();
        if !content.starts_with('!') {
            return;
        }

        match content {
            "!ping" => {
                let _ = msg
                    .channel_id
                    .say(&ctx.http, "Pong! 일해 AI 디스코드 봇이 작동 중입니다.")
                    .await;
            }
            "!help" | "!start" => {
                let help = "👋 **일해.ai 디스코드 봇**\n\n\
                    `!yolo` — YOLO 모드 토글\n\
                    `!browser` — 브라우저 사용 토글\n\
                    `!headless` — Headless 모드 토글\n\
                    `!ping` — 상태 확인";
                let _ = msg.channel_id.say(&ctx.http, help).await;
            }
            "!yolo" => {
                let current =
                    self.settings_store.get().permissions.approval_preset == "full-access";
                let new_val = !current;
                let preset = if new_val { "full-access" } else { "auto" };
                match self.settings_store.set_value(
                    "permissions.approval_preset",
                    serde_json::Value::String(preset.to_string()),
                ) {
                    Ok(()) => {
                        let status = if new_val { "ON ✅" } else { "OFF ❌" };
                        let _ = msg
                            .channel_id
                            .say(&ctx.http, format!("🛡️ **YOLO 모드**: {}", status))
                            .await;
                    }
                    Err(e) => {
                        error!("Failed to set YOLO mode: {:?}", e);
                        let _ = msg
                            .channel_id
                            .say(&ctx.http, "YOLO 모드 설정에 실패했습니다.")
                            .await;
                    }
                }
            }
            "!browser" => {
                let current = self.settings_store.get().browser.enabled;
                let new_val = !current;
                if let Ok(_) = self
                    .settings_store
                    .set_value("browser.enabled", serde_json::Value::Bool(new_val))
                {
                    let status = if new_val { "ON 🌐" } else { "OFF 🔴" };
                    let _ = msg
                        .channel_id
                        .say(&ctx.http, format!("🌐 **브라우저 활성화**: {}", status))
                        .await;
                }
            }
            "!headless" => {
                let current = self.settings_store.get().browser.headless;
                let new_val = !current;
                if let Ok(_) = self
                    .settings_store
                    .set_value("browser.headless", serde_json::Value::Bool(new_val))
                {
                    let status = if new_val { "ON 👻" } else { "OFF 👁️" };
                    let _ = msg
                        .channel_id
                        .say(&ctx.http, format!("👻 **Headless 모드**: {}", status))
                        .await;
                }
            }
            _ => {}
        }
    }

    async fn interaction_create(&self, ctx: Context, interaction: Interaction) {
        if let Interaction::Component(component) = interaction {
            let custom_id = &component.data.custom_id;

            // Format: "perm_approve:{permission_id}:{option_id}"
            if custom_id.starts_with("perm_") {
                let parts: Vec<&str> = custom_id.split(':').collect();
                if parts.len() >= 3 {
                    let action = parts[0];
                    let permission_id = parts[1];
                    let option_id = parts[2];

                    info!("[Discord] Button clicked: {} for {}", action, permission_id);

                    // Resolve the approval
                    self.approval_mgr
                        .resolve(
                            permission_id,
                            option_id.to_string(),
                            Some(format!("discord:{}", component.user.tag())),
                        )
                        .await;

                    // Update the message to show it's resolved
                    let response = serenity::builder::CreateInteractionResponse::UpdateMessage(
                        serenity::builder::CreateInteractionResponseMessage::new()
                            .content(format!(
                                "✅ **승인됨**: {} (by {})",
                                option_id,
                                component.user.tag()
                            ))
                            .components(vec![]), // Remove buttons
                    );
                    let _ = component.create_response(&ctx.http, response).await;
                }
            }
        }
    }

    async fn ready(&self, _: Context, ready: Ready) {
        info!("[Discord] {} is connected!", ready.user.name);
    }
}

pub async fn start(
    settings: DiscordSettings,
    settings_store: Arc<SettingsStore>,
    approval_mgr: Arc<ApprovalManager>,
    _relay_tx: mpsc::Sender<RelayEvent>,
) -> tokio::task::JoinHandle<()> {
    let token = settings.bot_token.clone();
    let guild_ids = settings.guild_ids.clone();
    let handler = Handler {
        settings_store: settings_store.clone(),
        approval_mgr: approval_mgr.clone(),
    };

    tokio::spawn(async move {
        let mut client = Client::builder(
            &token,
            GatewayIntents::GUILD_MESSAGES | GatewayIntents::MESSAGE_CONTENT,
        )
        .event_handler(handler)
        .await
        .expect("Err creating client");

        // Background loop to listen for Approval Requests and send to Discord
        let http = client.http.clone();
        let mut approval_rx = approval_mgr.subscribe();

        tokio::spawn(async move {
            while let Ok(event) = approval_rx.recv().await {
                if let codex_ilhae::approval_manager::ApprovalEvent::NewRequest(req) = event {
                    // For Discord, we need a channel to send the message.
                    // For now, we'll try to find a system channel or use configured guild IDs.
                    // This is a simplified implementation — in a real setup, we might map session_id to a specific thread/channel.

                    for guild_id_str in &guild_ids {
                        if let Ok(guild_id) = guild_id_str.parse::<u64>() {
                            let guild_id = serenity::model::id::GuildId::new(guild_id);
                            // Send to first available text channel for simplicity in PoC
                            if let Ok(channels) = guild_id.channels(&http).await {
                                if let Some(channel) = channels.values().find(|c| c.is_text_based())
                                {
                                    let mut buttons = Vec::new();
                                    for opt in &req.options {
                                        let style = if opt.id.contains("allow")
                                            || opt.id.contains("approve")
                                        {
                                            ButtonStyle::Success
                                        } else {
                                            ButtonStyle::Danger
                                        };
                                        buttons.push(
                                            CreateButton::new(format!(
                                                "perm_resolve:{}:{}",
                                                req.permission_id, opt.id
                                            ))
                                            .label(&opt.title)
                                            .style(style),
                                        );
                                    }
                                    let msg = CreateMessage::new()
                                        .content(format!("🛡️ **권한 승인 요청 (Discord)**\n**도구**: `{}`\n**설명**: {}\n\n아래 버튼을 눌러 승인하거나 거절하세요.", req.tool_title, req.description))
                                        .components(vec![CreateActionRow::Buttons(buttons)]);
                                    let _ = channel.id.send_message(&http, msg).await;
                                }
                            }
                        }
                    }
                }
            }
        });

        if let Err(why) = client.start().await {
            error!("[Discord] Client error: {:?}", why);
        }
    })
}
