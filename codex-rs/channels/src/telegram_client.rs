//! Telegram bot client for ilhae-proxy.
//!
//! Connects to the Telegram Bot API via `teloxide` and bridges messages
//! to/from the relay command handler. Acts as a virtual relay client
//! with a synthetic client_id.

use std::collections::HashMap;
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup, ParseMode};
use tokio::sync::{RwLock, mpsc};
use tracing::{error, info, warn};

use codex_ilhae::notification_store::NotificationStore;
use codex_ilhae::relay_server::{RelayCommand, RelayCommandWithClient, RelayEvent};
use codex_ilhae::session_store::SessionStore;
use codex_ilhae::settings_store::TelegramSettings;

/// Synthetic client ID for the Telegram bot in the relay system.
const TELEGRAM_CLIENT_ID: u64 = u64::MAX - 1;

/// Pending response waiting for a relay CommandResponse.
pub struct PendingResponse {
    pub chat_id: ChatId,
    pub message_id: Option<teloxide::types::MessageId>,
}

/// Shared state for the Telegram bot.
pub struct TelegramBotState {
    pub command_tx: mpsc::Sender<RelayCommandWithClient>,
    pub relay_tx: mpsc::Sender<RelayEvent>,
    pub store: Arc<SessionStore>,
    pub notification_store: Arc<NotificationStore>,
    pub settings_store: Arc<codex_ilhae::settings_store::SettingsStore>,
    pub settings: TelegramSettings,
    /// Default working directory for Telegram sessions.
    pub default_cwd: String,
    /// Maps request_id → pending response info
    pub pending: RwLock<HashMap<String, PendingResponse>>,
    /// Maps Telegram chat_id → session_id for persistent sessions
    pub chat_sessions: RwLock<HashMap<i64, String>>,
}

/// Start the Telegram bot if enabled in settings.
/// Returns a JoinHandle that can be aborted to stop the bot.
pub fn start(
    settings: TelegramSettings,
    command_tx: mpsc::Sender<RelayCommandWithClient>,
    relay_tx: mpsc::Sender<RelayEvent>,
    store: Arc<SessionStore>,
    notification_store: Arc<NotificationStore>,
    settings_store: Arc<codex_ilhae::settings_store::SettingsStore>,
    mut event_rx: mpsc::Receiver<String>,
    default_cwd: String,
) -> tokio::task::JoinHandle<()> {
    // ── Restore chat_sessions from DB before constructing state ──
    // Query all telegram sessions and rebuild chat_id → session_id mapping
    let mut restored_sessions = HashMap::new();
    if let Ok(tg_sessions) = store.list_sessions_by_channel("telegram") {
        for session in &tg_sessions {
            if !session.channel_meta.is_empty() {
                if let Ok(meta) = serde_json::from_str::<serde_json::Value>(&session.channel_meta) {
                    if let Some(chat_id) = meta.get("chat_id").and_then(|v| v.as_i64()) {
                        // Only keep the most recently updated session per chat_id
                        restored_sessions
                            .entry(chat_id)
                            .or_insert_with(|| session.id.clone());
                    }
                }
            }
        }
        if !restored_sessions.is_empty() {
            tracing::info!(
                "[Telegram] Restored {} chat-session mappings from DB",
                restored_sessions.len()
            );
        }
    }

    let bot_state = Arc::new(TelegramBotState {
        command_tx,
        relay_tx,
        store,
        notification_store,
        settings_store,
        settings: settings.clone(),
        default_cwd,
        pending: RwLock::new(HashMap::new()),
        chat_sessions: RwLock::new(restored_sessions),
    });

    let bot = Bot::new(&settings.bot_token);

    // Spawn event listener for relay responses
    let state_for_events = bot_state.clone();
    let bot_for_events = bot.clone();
    tokio::spawn(async move {
        while let Some(json) = event_rx.recv().await {
            if let Ok(event) = serde_json::from_str::<RelayEvent>(&json) {
                handle_relay_event(&bot_for_events, &state_for_events, event).await;
            }
        }
    });

    // Spawn the teloxide dispatcher
    let state_for_handler = bot_state.clone();
    let state_for_callback = bot_state.clone();
    tokio::spawn(async move {
        info!("[Telegram] Bot starting...");

        let handler =
            dptree::entry()
                .branch(Update::filter_message().endpoint(
                    move |bot: Bot, msg: Message, state: Arc<TelegramBotState>| async move {
                        handle_message(bot, msg, state).await
                    },
                ))
                .branch(Update::filter_callback_query().endpoint(
                    move |bot: Bot,
                          q: teloxide::types::CallbackQuery,
                          state: Arc<TelegramBotState>| async move {
                        handle_callback_query(bot, q, state).await
                    },
                ));

        Dispatcher::builder(bot, handler)
            .dependencies(dptree::deps![state_for_handler, state_for_callback])
            .enable_ctrlc_handler()
            .build()
            .dispatch()
            .await;

        info!("[Telegram] Bot stopped");
    })
}

/// Handle incoming Telegram messages.
async fn handle_message(
    bot: Bot,
    msg: Message,
    state: Arc<TelegramBotState>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let chat_id = msg.chat.id;

    // Check allowlist
    if !state.settings.allowed_chat_ids.is_empty()
        && !state.settings.allowed_chat_ids.contains(&chat_id.0)
    {
        bot.send_message(chat_id, "⛔ 인가되지 않은 채팅입니다.")
            .await?;
        return Ok(());
    }

    let text = msg.text().unwrap_or("").trim();
    if text.is_empty() {
        return Ok(());
    }

    // Handle commands
    if text.starts_with('/') {
        return handle_command(&bot, &msg, &state, text).await;
    }

    // Regular message → send to agent via relay chat command
    send_chat(&bot, &msg, &state, text).await
}

/// Handle callback queries from inline keyboards (e.g. permission buttons).
async fn handle_callback_query(
    bot: Bot,
    q: teloxide::types::CallbackQuery,
    state: Arc<TelegramBotState>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Answer the callback query to dismiss the loading indicator
    bot.answer_callback_query(&q.id).await?;

    let data = q.data.unwrap_or_default();
    // Callback data format: "perm:{permission_id}:{option_id}"
    if let Some(rest) = data.strip_prefix("perm:") {
        let parts: Vec<&str> = rest.splitn(2, ':').collect();
        if parts.len() == 2 {
            let permission_id = parts[0];
            let option_id = parts[1];

            let cmd = RelayCommand {
                action: "permission_response".to_string(),
                payload: serde_json::json!({
                    "permission_id": permission_id,
                    "option_id": option_id,
                }),
                request_id: Some(uuid::Uuid::new_v4().to_string()),
            };

            let _ = state
                .command_tx
                .send(RelayCommandWithClient {
                    cmd,
                    client_id: TELEGRAM_CLIENT_ID,
                })
                .await;

            // Edit the original message to show the decision
            let decision_text = match option_id {
                "always_allow" => "✅ Always Allow (이후 자동 허용)",
                "deny" => "❌ Rejected",
                _ => "✅ Allowed",
            };
            if let Some(msg) = q.message {
                let (msg_id, chat_id_for_edit) = match &msg {
                    teloxide::types::MaybeInaccessibleMessage::Regular(m) => (m.id, m.chat.id),
                    teloxide::types::MaybeInaccessibleMessage::Inaccessible(m) => {
                        (m.message_id, m.chat.id)
                    }
                };
                // Try to remove inline keyboard after decision
                let _ = bot
                    .edit_message_reply_markup(
                        teloxide::types::Recipient::Id(chat_id_for_edit),
                        msg_id,
                    )
                    .await;
            }

            // Send confirmation
            if let Some(chat_id_raw) = state.settings.allowed_chat_ids.first() {
                let _ = bot.send_message(ChatId(*chat_id_raw), decision_text).await;
            }
        }
    }

    Ok(())
}

/// Handle slash commands.
async fn handle_command(
    bot: &Bot,
    msg: &Message,
    state: &Arc<TelegramBotState>,
    text: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let chat_id = msg.chat.id;

    // Parse command: "/cmd@bot_name args" -> "/cmd"
    let raw_cmd = text.split_whitespace().next().unwrap_or("");
    let cmd = raw_cmd.split('@').next().unwrap_or("");

    match cmd {
        "/start" => {
            bot.send_message(chat_id, "👋 일해.ai 텔레그램 봇입니다!\n\n메시지를 보내면 에이전트가 답변합니다.\n\n/sessions — 세션 목록\n/schedules — 할일 목록\n/new — 새 세션\n/yolo — YOLO 모드 토글 (자동 권한 승인)\n/browser — 브라우저 토글\n/headless — Headless 모드 토글")
                .await?;
        }
        "/sessions" => match state.store.list_sessions() {
            Ok(sessions) => {
                if sessions.is_empty() {
                    bot.send_message(chat_id, "📋 세션이 없습니다.").await?;
                } else {
                    let list: String = sessions
                        .iter()
                        .take(20)
                        .enumerate()
                        .map(|(i, s)| {
                            format!(
                                "{}. *{}* ({}건)",
                                i + 1,
                                escape_markdown(&s.title),
                                s.message_count
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    bot.send_message(chat_id, format!("📋 *세션 목록*\n\n{}", list))
                        .parse_mode(ParseMode::MarkdownV2)
                        .await?;
                }
            }
            Err(e) => {
                bot.send_message(chat_id, format!("❌ 오류: {}", e)).await?;
            }
        },
        "/schedules" => {
            let cmd = RelayCommand {
                action: "task_list".to_string(),
                payload: serde_json::json!({}),
                request_id: Some(uuid::Uuid::new_v4().to_string()),
            };
            let request_id = cmd.request_id.clone().unwrap();

            // Register pending response
            {
                let mut pending = state.pending.write().await;
                pending.insert(
                    request_id.clone(),
                    PendingResponse {
                        chat_id,
                        message_id: None,
                    },
                );
            }

            let _ = state
                .command_tx
                .send(RelayCommandWithClient {
                    cmd,
                    client_id: TELEGRAM_CLIENT_ID,
                })
                .await;
        }
        "/new" => {
            // Clear session mapping for this chat
            {
                let mut sessions = state.chat_sessions.write().await;
                sessions.remove(&chat_id.0);
            }
            bot.send_message(chat_id, "🆕 새 세션을 시작합니다. 메시지를 보내주세요!")
                .await?;
        }
        "/notifications" | "/notif" => match state.notification_store.list(0, 10) {
            Ok(notifs) => {
                if notifs.is_empty() {
                    bot.send_message(chat_id, "🔔 알림이 없습니다.").await?;
                } else {
                    let list: String = notifs
                        .iter()
                        .map(|n| {
                            let icon = match n.level.as_str() {
                                "error" => "🔴",
                                "warning" => "🟡",
                                "success" => "🟢",
                                _ => "🔵",
                            };
                            let read_mark = if n.read { "✓" } else { "●" };
                            format!("{} {} {}", icon, read_mark, n.message)
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    bot.send_message(chat_id, format!("🔔 알림 목록\n\n{}", list))
                        .await?;
                }
            }
            Err(e) => {
                bot.send_message(chat_id, format!("❌ 오류: {}", e)).await?;
            }
        },
        "/yolo" => {
            let current = state.settings_store.get().permissions.approval_preset == "full-access";
            let new_val = !current;
            let preset = if new_val { "full-access" } else { "auto" };
            match state.settings_store.set_value(
                "permissions.approval_preset",
                serde_json::Value::String(preset.to_string()),
            ) {
                Ok(_) => {
                    let emoji = if new_val { "🟢" } else { "🔴" };
                    let status = if new_val { "ON" } else { "OFF" };
                    bot.send_message(
                        chat_id,
                        format!(
                            "{} YOLO 모드 {}\n\n{}",
                            emoji,
                            status,
                            if new_val {
                                "모든 권한 요청이 자동 승인됩니다."
                            } else {
                                "권한 요청 시 승인이 필요합니다."
                            }
                        ),
                    )
                    .await?;
                }
                Err(e) => {
                    bot.send_message(chat_id, format!("❌ YOLO 설정 변경 실패: {}", e))
                        .await?;
                }
            }
        }
        "/browser" => {
            let current = state.settings_store.get().browser.enabled;
            let new_val = !current;
            match state
                .settings_store
                .set_value("browser.enabled", serde_json::Value::Bool(new_val))
            {
                Ok(_) => {
                    let emoji = if new_val { "🌐" } else { "🔴" };
                    let status = if new_val { "ON" } else { "OFF" };
                    bot.send_message(
                        chat_id,
                        format!(
                            "{} 브라우저 {}\n\n{}",
                            emoji,
                            status,
                            if new_val {
                                "브라우저 자동화가 활성화됩니다."
                            } else {
                                "브라우저 자동화가 비활성화됩니다."
                            }
                        ),
                    )
                    .await?;
                }
                Err(e) => {
                    bot.send_message(chat_id, format!("❌ 브라우저 설정 변경 실패: {}", e))
                        .await?;
                }
            }
        }
        "/headless" => {
            let current = state.settings_store.get().browser.headless;
            let new_val = !current;
            match state
                .settings_store
                .set_value("browser.headless", serde_json::Value::Bool(new_val))
            {
                Ok(_) => {
                    let emoji = if new_val { "👻" } else { "👁️" };
                    let status = if new_val { "ON" } else { "OFF" };
                    bot.send_message(
                        chat_id,
                        format!(
                            "{} Headless 모드 {}\n\n{}",
                            emoji,
                            status,
                            if new_val {
                                "브라우저 창을 띄우지 않고 백그라운드에서 실행합니다."
                            } else {
                                "브라우저 창을 띄워서 실행합니다."
                            }
                        ),
                    )
                    .await?;
                }
                Err(e) => {
                    bot.send_message(chat_id, format!("❌ Headless 설정 변경 실패: {}", e))
                        .await?;
                }
            }
        }
        _ => {
            bot.send_message(chat_id, "❓ 알 수 없는 명령입니다.\n/start — 도움말")
                .await?;
        }
    }

    Ok(())
}

/// Send a chat message to the agent via relay command handler.
async fn send_chat(
    bot: &Bot,
    msg: &Message,
    state: &Arc<TelegramBotState>,
    text: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let chat_id = msg.chat.id;

    // Send "typing" indicator
    let typing_msg = bot
        .send_message(chat_id, "⏳ 에이전트에게 전달 중...")
        .await?;

    // Get or create session ID for this chat
    let session_id = {
        let sessions = state.chat_sessions.read().await;
        sessions.get(&chat_id.0).cloned()
    };
    let session_id = match session_id {
        Some(id) => id,
        None => {
            // Create a new session
            let id = uuid::Uuid::new_v4().to_string();
            let title = text.chars().take(40).collect::<String>();
            let channel_meta = serde_json::json!({"chat_id": chat_id.0}).to_string();
            match state.store.create_session_with_channel_meta(
                &id,
                &title,
                "",
                &state.default_cwd,
                "telegram",
                &channel_meta,
            ) {
                Ok(()) => {
                    let mut sessions = state.chat_sessions.write().await;
                    sessions.insert(chat_id.0, id.clone());
                    id
                }
                Err(e) => {
                    bot.send_message(chat_id, format!("❌ 세션 생성 실패: {}", e))
                        .await?;
                    return Ok(());
                }
            }
        }
    };

    let request_id = uuid::Uuid::new_v4().to_string();
    let cmd = RelayCommand {
        action: "chat_message".to_string(),
        payload: serde_json::json!({
            "session_id": session_id,
            "text": text,
            "attachments": [],
        }),
        request_id: Some(request_id.clone()),
    };

    // Register pending response
    {
        let mut pending = state.pending.write().await;
        pending.insert(
            request_id.clone(),
            PendingResponse {
                chat_id,
                message_id: Some(typing_msg.id),
            },
        );
    }

    if let Err(e) = state
        .command_tx
        .send(RelayCommandWithClient {
            cmd,
            client_id: TELEGRAM_CLIENT_ID,
        })
        .await
    {
        error!("[Telegram] Failed to send command: {}", e);
        bot.send_message(chat_id, "❌ 에이전트 연결 실패").await?;
    }

    Ok(())
}

/// Handle relay events directed at the Telegram bot.
async fn handle_relay_event(bot: &Bot, state: &Arc<TelegramBotState>, event: RelayEvent) {
    match event {
        RelayEvent::CommandResponse {
            request_id,
            result,
            error,
        } => {
            let pending = {
                let mut map = state.pending.write().await;
                map.remove(&request_id)
            };
            if let Some(pending) = pending {
                let text = if let Some(err) = error {
                    format!("❌ {}", err)
                } else {
                    // Try to extract meaningful text from result
                    format_result(&result)
                };

                if let Some(msg_id) = pending.message_id {
                    // Edit the "typing" message with the result
                    let _ = bot.edit_message_text(pending.chat_id, msg_id, &text).await;
                } else {
                    let _ = bot.send_message(pending.chat_id, &text).await;
                }
            }
        }
        RelayEvent::SessionNotification { session_id, update } => {
            // Check if any Telegram chat is watching this session
            let sessions = state.chat_sessions.read().await;
            for (&chat_id_raw, sid) in sessions.iter() {
                if sid == &session_id {
                    // Forward session update to Telegram
                    if let Some(text) = extract_notification_text(&update) {
                        if !text.trim().is_empty() {
                            let _ = bot.send_message(ChatId(chat_id_raw), &text).await;
                        }
                    }
                }
            }
        }
        RelayEvent::UiNotification { message, level, .. } => {
            // Forward UI notifications to all allowed chats
            let icon = match level.as_str() {
                "error" => "🔴",
                "warning" => "🟡",
                "success" => "🟢",
                _ => "🔵",
            };
            let text = format!("{} {}", icon, message);

            // Save to notification store
            let _ = state.notification_store.add(&message, &level, "agent");

            // Send to all allowed chats
            for &chat_id in &state.settings.allowed_chat_ids {
                let _ = bot.send_message(ChatId(chat_id), &text).await;
            }
        }
        RelayEvent::PermissionRequest {
            permission_id,
            session_id,
            tool_title,
            tool_kind,
            description,
            options,
        } => {
            // Find the Telegram chat watching this session
            let sessions = state.chat_sessions.read().await;
            let target_chat = sessions
                .iter()
                .find(|(_, sid)| **sid == session_id)
                .map(|(&chat_id_raw, _)| ChatId(chat_id_raw));

            // If no specific chat found, send to first allowed chat
            let chat_id = target_chat
                .unwrap_or_else(|| ChatId(*state.settings.allowed_chat_ids.first().unwrap_or(&0)));

            if chat_id.0 == 0 {
                warn!(
                    "[Telegram] No chat found for permission request (session {})",
                    session_id
                );
                return;
            }

            // Build inline keyboard with permission options
            let mut buttons: Vec<Vec<InlineKeyboardButton>> = Vec::new();
            for opt in &options {
                let opt_id = opt.get("id").and_then(|v| v.as_str()).unwrap_or("allow");
                let opt_title = opt.get("title").and_then(|v| v.as_str()).unwrap_or("Allow");
                let callback_data = format!("perm:{}:{}", permission_id, opt_id);
                buttons.push(vec![InlineKeyboardButton::callback(
                    opt_title.to_string(),
                    callback_data,
                )]);
            }
            let keyboard = InlineKeyboardMarkup::new(buttons);

            // Format the permission message
            let desc_preview = if description.len() > 200 {
                format!("{}...", &description[..200])
            } else {
                description.clone()
            };

            // Try MarkdownV2 first, fallback to plain text if it fails
            let md_text = format!(
                "🔐 *권한 요청*\n\n\
                 🔧 *도구*: {}\n\
                 📁 *종류*: {}\n\n\
                 ```\n{}\n```\n\n\
                 아래 버튼을 눌러 허용/거부하세요.",
                escape_markdown(&tool_title),
                escape_markdown(&tool_kind),
                escape_markdown(&desc_preview),
            );

            let result = bot
                .send_message(chat_id, &md_text)
                .parse_mode(ParseMode::MarkdownV2)
                .reply_markup(keyboard.clone())
                .await;

            if let Err(e) = result {
                warn!(
                    "[Telegram] MarkdownV2 send failed, using plain text fallback: {}",
                    e
                );
                // Plain text fallback — always reaches the user
                let plain_text = format!(
                    "🔐 권한 요청\n\n\
                     🔧 도구: {}\n\
                     📁 종류: {}\n\n\
                     {}\n\n\
                     아래 버튼을 눌러 허용/거부하세요.",
                    tool_title, tool_kind, desc_preview,
                );
                let _ = bot
                    .send_message(chat_id, &plain_text)
                    .reply_markup(keyboard)
                    .await;
            }
        }
        _ => {}
    }
}

/// Format a relay command response result for Telegram display.
pub fn format_result(result: &serde_json::Value) -> String {
    // Chat message responses include the actual AI text
    if let Some(text) = result.get("text").and_then(|v| v.as_str()) {
        if !text.is_empty() {
            // If thinking was separated by the protocol, text is already clean
            let thinking = result
                .get("thinking")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            if !thinking.is_empty() {
                // Thinking was properly separated — text is the clean response
                return text.to_string();
            }

            // Thinking might be embedded in text (agent sends thinking as AgentMessageChunk)
            // Try to extract only the final meaningful response
            // Common pattern: thinking blocks followed by actual response
            // Look for the last paragraph block that looks like actual response
            let cleaned = strip_embedded_thinking(text);
            if !cleaned.is_empty() {
                return cleaned;
            }

            return text.to_string();
        }
    }

    if let Some(ok) = result.get("ok").and_then(|v| v.as_bool()) {
        if ok {
            return "✅ 완료".to_string();
        }
    }

    // For task_list responses
    if let Some(schedules) = result.as_array() {
        if schedules.is_empty() {
            return "📋 할일이 없습니다.".to_string();
        }
        return schedules
            .iter()
            .take(20)
            .enumerate()
            .map(|(i, t)| {
                let title = t
                    .get("title")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Untitled");
                let done = t.get("done").and_then(|v| v.as_bool()).unwrap_or(false);
                let check = if done { "✅" } else { "⬜" };
                format!("{}. {} {}", i + 1, check, title)
            })
            .collect::<Vec<_>>()
            .join("\n");
    }

    // Fallback: pretty-print JSON
    serde_json::to_string_pretty(result).unwrap_or_else(|_| result.to_string())
}

/// Extract displayable text from a SessionNotification update.
///
/// IMPORTANT: We intentionally skip `content_delta` (text chunks) and `thought_delta`
/// (thinking chunks) because the final accumulated text is already sent via
/// `CommandResponse.result.text`. Sending each streaming chunk as a new Telegram
/// message causes the "message accumulation" bug where text keeps piling up.
///
/// We only extract actionable notifications here:
/// - Tool call starts/completions (so user can see agent is working)
pub fn extract_notification_text(update: &serde_json::Value) -> Option<String> {
    // Tool call status updates — show brief status
    if let Some(tc) = update.get("ToolCall").or_else(|| update.get("tool_call")) {
        let title = tc.get("title").and_then(|v| v.as_str()).unwrap_or("tool");
        let status = tc.get("status").and_then(|v| v.as_str()).unwrap_or("");
        match status {
            "in_progress" => return Some(format!("🔧 {} 실행 중...", title)),
            "completed" => return Some(format!("✅ {} 완료", title)),
            "failed" | "error" => {
                let err = tc
                    .get("error")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown error");
                return Some(format!("❌ {} 실패: {}", title, err));
            }
            _ => {}
        }
    }

    // Skip content_delta / thought_delta — these cause accumulation
    // The final text is delivered via CommandResponse
    None
}

/// Escape MarkdownV2 special characters for Telegram.
pub fn escape_markdown(text: &str) -> String {
    let special = [
        '_', '*', '[', ']', '(', ')', '~', '`', '>', '#', '+', '-', '=', '|', '{', '}', '.', '!',
    ];
    let mut result = String::with_capacity(text.len());
    for c in text.chars() {
        if special.contains(&c) {
            result.push('\\');
        }
        result.push(c);
    }
    result
}

/// Strip embedded thinking content from agent response text.
///
/// When the agent (e.g. Gemini CLI) sends thinking as AgentMessageChunk instead
/// of AgentThoughtChunk, the thinking text gets embedded in `buf.content` along
/// with the actual response. This function tries to extract only the final
/// meaningful response.
///
/// Gemini thinking pattern: blocks of short, meta-commentary text followed by
/// the actual response. We split by double newlines and take content after
/// the last thinking-style block.
pub fn strip_embedded_thinking(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    // Split by double newlines to find paragraph blocks
    let blocks: Vec<&str> = trimmed.split("\n\n").collect();

    if blocks.len() <= 1 {
        // Single block — return as-is
        return trimmed.to_string();
    }

    // Heuristic: thinking blocks are typically:
    // - Short titles like "Acknowledging a Greeting" or "Responding to Korean Query"
    // - Commentary like "I've decided to..." or "I'm zeroing in on..."
    // The actual response is typically the last block(s) that don't match thinking patterns

    // Find the last block that looks like actual content (not a thinking heading or meta-commentary)
    let mut response_start_idx = 0;
    for (i, block) in blocks.iter().enumerate() {
        let block_trimmed = block.trim();
        // Skip empty blocks
        if block_trimmed.is_empty() {
            continue;
        }
        // Thinking headings: short, single-line, no period, starts with capital
        let is_heading = block_trimmed.lines().count() == 1
            && block_trimmed.len() < 60
            && !block_trimmed.contains('.')
            && block_trimmed
                .chars()
                .next()
                .map(|c| c.is_uppercase())
                .unwrap_or(false);

        // Thinking commentary: starts with "I " or "I'" (first person meta-commentary)
        let is_commentary = block_trimmed.starts_with("I ")
            || block_trimmed.starts_with("I'")
            || block_trimmed.starts_with("The ")
            || block_trimmed.starts_with("Let me ");

        if !is_heading && !is_commentary {
            response_start_idx = i;
            // Don't break — we want the LAST non-thinking block
        }
    }

    // Collect from the identified response start to the end
    let response_blocks: Vec<&str> = blocks[response_start_idx..]
        .iter()
        .map(|b| b.trim())
        .filter(|b| !b.is_empty())
        .collect();

    if response_blocks.is_empty() {
        // Fallback: return the last block
        return blocks.last().unwrap_or(&"").trim().to_string();
    }

    response_blocks.join("\n\n")
}
