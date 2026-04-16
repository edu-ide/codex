use serde_json::json;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{error, info};

use codex_ilhae::approval_manager::{ApprovalEvent, ApprovalManager};
use codex_ilhae::relay_server::RelayEvent;
use codex_ilhae::settings_store::KakaoSettings;

pub async fn start(
    settings: KakaoSettings,
    approval_mgr: Arc<ApprovalManager>,
    _relay_tx: mpsc::Sender<RelayEvent>,
) -> tokio::task::JoinHandle<()> {
    let client = reqwest::Client::new();
    let app_key = settings.app_key.clone();

    tokio::spawn(async move {
        info!("[Kakao] Starting approval listener");

        let mut approval_rx = approval_mgr.subscribe();

        while let Ok(event) = approval_rx.recv().await {
            if let ApprovalEvent::NewRequest(req) = event {
                info!("[Kakao] Sending approval request for {}", req.tool_title);

                // Build Kakao Custom Template or Simple Text Message
                // For PoC, we'll use the 'Default Memo' template (Send to Me) if configured,
                // or a mock for sending to specific users.

                let text = format!(
                    "🛡️ [일해 AI 권한 승인 요청]
도구: {}
설명: {}

데스크톱 또는 텔레그램에서 승인해주세요.",
                    req.tool_title, req.description
                );

                // Kakao REST API: https://kapi.kakao.com/v2/api/talk/memo/default/send
                let payload = json!({
                    "object_type": "text",
                    "text": text,
                    "link": {
                        "web_url": "https://app.ugot.uk",
                        "mobile_web_url": "https://app.ugot.uk"
                    },
                    "button_title": "일해 앱 열기"
                });

                let res = client
                    .post("https://kapi.kakao.com/v2/api/talk/memo/default/send")
                    .header("Authorization", format!("Bearer {}", app_key))
                    .form(&[("template_object", payload.to_string())])
                    .send()
                    .await;

                match res {
                    Ok(resp) if resp.status().is_success() => {
                        info!("[Kakao] Approval notification sent");
                    }
                    Ok(resp) => {
                        error!("[Kakao] Failed to send: HTTP {}", resp.status());
                    }
                    Err(e) => {
                        error!("[Kakao] Network error: {:?}", e);
                    }
                }
            }
        }
    })
}
