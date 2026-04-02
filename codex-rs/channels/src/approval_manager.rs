use codex_core::tools::sandboxing::{ApprovalRecord, ApprovalStore};
use codex_protocol::protocol::ReviewDecision;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, oneshot};
use tracing::{debug, info, warn};

pub const DEFAULT_APPROVAL_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Debug, Clone)]
pub enum ApprovalEvent {
    NewRequest(ApprovalRequest),
    Resolved {
        permission_id: String,
        option_id: String,
        resolved_by: String,
    },
    Expired {
        permission_id: String,
    },
}

#[derive(Debug, Clone)]
pub struct ApprovalRequest {
    pub permission_id: String,
    pub session_id: String,
    pub tool_title: String,
    pub tool_kind: String,
    pub description: String,
    pub options: Vec<ApprovalOption>,
}

#[derive(Debug, Clone)]
pub struct ApprovalOption {
    pub id: String,
    pub title: String,
}

pub type ApprovalDecision = Option<String>;

/// Proxy-level ApprovalManager.
/// It no longer stores data; it delegates to Codex's ApprovalStore.
pub struct ApprovalManager {
    codex_store: Arc<tokio::sync::Mutex<ApprovalStore>>,
    event_tx: broadcast::Sender<ApprovalEvent>,
}

impl ApprovalManager {
    pub fn new(codex_store: Arc<tokio::sync::Mutex<ApprovalStore>>) -> Arc<Self> {
        let (event_tx, _) = broadcast::channel(64);
        Arc::new(Self {
            codex_store,
            event_tx,
        })
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ApprovalEvent> {
        self.event_tx.subscribe()
    }

    pub async fn register(
        self: &Arc<Self>,
        request: ApprovalRequest,
        timeout: Duration,
    ) -> Result<(ApprovalRecord, oneshot::Receiver<ApprovalDecision>), String> {
        let id = request.permission_id.clone();

        let record = ApprovalRecord {
            id: id.clone(),
            tool_name: request.tool_title.clone(),
            description: request.description.clone(),
            decision: ReviewDecision::Pending,
            created_at_ms: crate::approval_manager::now_millis(),
            resolved_at_ms: None,
        };

        let (tx, rx) = oneshot::channel();

        // 1. Delegate storage to Codex
        {
            let mut store = self.codex_store.lock().await;
            store.put_record(record.clone());
        }

        // 2. Broadcast to all clients (Desktop, Telegram, etc.)
        let _ = self
            .event_tx
            .send(ApprovalEvent::NewRequest(request.clone()));

        // 3. Handle timeout
        let manager = Arc::clone(self);
        let timeout_id = id.clone();
        tokio::spawn(async move {
            tokio::time::sleep(timeout).await;
            manager.handle_timeout(&timeout_id, tx).await;
        });

        debug!("[ApprovalManager] Registered via Codex: {}", id);
        Ok((record, rx))
    }

    pub async fn resolve(
        self: &Arc<Self>,
        id: &str,
        option_id: String,
        resolved_by: Option<String>,
    ) -> bool {
        let decision = if option_id == "allow" {
            ReviewDecision::ApprovedForSession
        } else {
            ReviewDecision::Rejected
        };

        // 1. Update Source of Truth in Codex
        let mut store = self.codex_store.lock().await;
        let success = store.resolve_record(id, decision);

        if success {
            // 2. Broadcast resolution
            let _ = self.event_tx.send(ApprovalEvent::Resolved {
                permission_id: id.to_string(),
                option_id: option_id.clone(),
                resolved_by: resolved_by.unwrap_or_else(|| "unknown".into()),
            });
            info!("[ApprovalManager] Resolved via Codex: {}", id);
        }
        success
    }

    async fn handle_timeout(&self, id: &str, tx: oneshot::Sender<ApprovalDecision>) {
        let mut store = self.codex_store.lock().await;
        if let Some(record) = store.get_record(id) {
            if matches!(record.decision, ReviewDecision::Pending) {
                store.resolve_record(id, ReviewDecision::Rejected); // Timeout counts as rejection
                let _ = tx.send(None);
                let _ = self.event_tx.send(ApprovalEvent::Expired {
                    permission_id: id.to_string(),
                });
                warn!("[ApprovalManager] Timeout via Codex: {}", id);
            }
        }
    }
}

pub fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
