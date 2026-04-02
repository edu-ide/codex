use slack_morphism::prelude::*;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::approval_manager::{ApprovalEvent, ApprovalManager};
use crate::relay_server::RelayEvent;
use crate::settings_store::GenericChannelSettings;

pub async fn start(
    settings: GenericChannelSettings,
    approval_mgr: Arc<ApprovalManager>,
    _relay_tx: mpsc::Sender<RelayEvent>,
) -> tokio::task::JoinHandle<()> {
    let bot_token: SlackApiToken =
        SlackApiToken::new(SlackApiTokenValue(settings.api_token.clone()));

    let client = SlackClient::new(
        SlackClientHyperConnector::new().expect("Failed to create Slack connector"),
    );
    let client = Arc::new(client);

    let channel_id_str = settings
        .extra
        .get("default_channel")
        .and_then(|v| v.as_str())
        .unwrap_or("general")
        .to_string();
    let channel_id = SlackChannelId::new(channel_id_str);

    let approval_mgr_clone = approval_mgr.clone();
    let client_for_notif = client.clone();
    let bot_token_for_notif = bot_token.clone();

    tokio::spawn(async move {
        // 1. Start Approval Listener (Outbound)
        let mut approval_rx = approval_mgr_clone.subscribe();
        let client_inner = client_for_notif.clone();
        let token_inner = bot_token_for_notif.clone();
        let chan_inner = channel_id.clone();

        tokio::spawn(async move {
            while let Ok(event) = approval_rx.recv().await {
                if let ApprovalEvent::NewRequest(req) = event {
                    let session = client_inner.open_session(&token_inner);

                    let blocks: Vec<SlackBlock> =
                        vec![SlackBlock::Section(SlackSectionBlock::new().with_text(
                            SlackBlockText::MarkDown(SlackBlockMarkDownText::new(format!(
                                "🛡️ *권한 승인 요청 (Slack)*\n*도구*: `{}`\n*설명*: {}",
                                req.tool_title, req.description
                            ))),
                        ))];

                    let post_msg_req = SlackApiChatPostMessageRequest::new(
                        chan_inner.clone(),
                        SlackMessageContent::new().with_blocks(blocks),
                    );
                    if let Err(e) = session.chat_post_message(&post_msg_req).await {
                        warn!("[Slack] Failed to post approval message: {:?}", e);
                    }
                }
            }
        });

        info!("[Slack] Approval listener started (outbound only, no socket mode)");

        // Keep the task alive
        std::future::pending::<()>().await;
    })
}
