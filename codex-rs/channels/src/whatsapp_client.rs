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
        info!("[WhatsApp] Starting approval listener");
        let mut approval_rx = approval_mgr.subscribe();

        while let Ok(event) = approval_rx.recv().await {
            if let ApprovalEvent::NewRequest(req) = event {
                info!("[WhatsApp] Sending approval request for {}", req.tool_title);

                // WhatsApp Business API Template
                let payload = json!({
                    "messaging_product": "whatsapp",
                    "to": "PHONE_NUMBER_FROM_EXTRA",
                    "type": "template",
                    "template": {
                        "name": "approval_request",
                        "language": { "code": "ko" }
                    }
                });

                let _ = client
                    .post("https://graph.facebook.com/v17.0/PHONE_ID/messages")
                    .header("Authorization", format!("Bearer {}", token))
                    .json(&payload)
                    .send()
                    .await;
            }
        }
    })
}
