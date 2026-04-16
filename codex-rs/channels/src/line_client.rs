use serde_json::json;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::info;

use codex_ilhae::approval_manager::{ApprovalEvent, ApprovalManager};
use codex_ilhae::relay_server::RelayEvent;
use codex_ilhae::settings_store::GenericChannelSettings;

pub async fn start(
    settings: GenericChannelSettings,
    approval_mgr: Arc<ApprovalManager>,
    _relay_tx: mpsc::Sender<RelayEvent>,
) -> tokio::task::JoinHandle<()> {
    let client = reqwest::Client::new();
    let token = settings.api_token.clone();

    tokio::spawn(async move {
        info!("[LINE] Starting approval listener");
        let mut approval_rx = approval_mgr.subscribe();

        while let Ok(event) = approval_rx.recv().await {
            if let ApprovalEvent::NewRequest(req) = event {
                info!("[LINE] Sending approval request for {}", req.tool_title);

                // LINE Flex Message for Buttons
                let payload = json!({
                    "to": "USER_ID_FROM_EXTRA_OR_REPLACE",
                    "messages": [{
                        "type": "text",
                        "text": format!("🛡️ [일해 AI 권한 승인 요청]
도구: {}
설명: {}", req.tool_title, req.description)
                    }]
                });

                let _ = client
                    .post("https://api.line.me/v2/bot/message/broadcast")
                    .header("Authorization", format!("Bearer {}", token))
                    .json(&payload)
                    .send()
                    .await;
            }
        }
    })
}
