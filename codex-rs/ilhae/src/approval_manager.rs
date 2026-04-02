use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, broadcast, oneshot};
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

#[derive(Debug, Clone)]
pub struct ApprovalRecord {
    pub id: String,
    pub tool_name: String,
    pub description: String,
    pub decision: String,
    pub created_at_ms: u64,
    pub resolved_at_ms: Option<u64>,
}

pub type ApprovalDecision = Option<String>;

pub struct ApprovalManager {
    records: Arc<Mutex<HashMap<String, ApprovalRecord>>>,
    event_tx: broadcast::Sender<ApprovalEvent>,
}

impl ApprovalManager {
    pub fn new() -> Arc<Self> {
        let (event_tx, _) = broadcast::channel(64);
        Arc::new(Self {
            records: Arc::new(Mutex::new(HashMap::new())),
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
            decision: "pending".to_string(),
            created_at_ms: now_millis(),
            resolved_at_ms: None,
        };

        let (tx, rx) = oneshot::channel();

        self.records.lock().await.insert(id.clone(), record.clone());
        let _ = self.event_tx.send(ApprovalEvent::NewRequest(request));

        let manager = Arc::clone(self);
        let timeout_id = id.clone();
        tokio::spawn(async move {
            tokio::time::sleep(timeout).await;
            manager.handle_timeout(&timeout_id, tx).await;
        });

        debug!("[ApprovalManager] Registered local approval: {}", id);
        Ok((record, rx))
    }

    pub async fn resolve(
        self: &Arc<Self>,
        id: &str,
        option_id: String,
        resolved_by: Option<String>,
    ) -> bool {
        let decision = if option_id == "allow" || option_id == "allow_always" {
            "approved_for_session"
        } else {
            "denied"
        };

        let mut records = self.records.lock().await;
        let Some(record) = records.get_mut(id) else {
            return false;
        };
        if record.decision != "pending" {
            return false;
        }
        record.decision = decision.to_string();
        record.resolved_at_ms = Some(now_millis());
        drop(records);

        let _ = self.event_tx.send(ApprovalEvent::Resolved {
            permission_id: id.to_string(),
            option_id: option_id.clone(),
            resolved_by: resolved_by.unwrap_or_else(|| "unknown".into()),
        });
        info!("[ApprovalManager] Resolved local approval: {}", id);
        true
    }

    async fn handle_timeout(&self, id: &str, tx: oneshot::Sender<ApprovalDecision>) {
        let mut records = self.records.lock().await;
        let Some(record) = records.get_mut(id) else {
            return;
        };
        if record.decision != "pending" {
            return;
        }
        record.decision = "denied".to_string();
        record.resolved_at_ms = Some(now_millis());
        drop(records);

        let _ = tx.send(None);
        let _ = self.event_tx.send(ApprovalEvent::Expired {
            permission_id: id.to_string(),
        });
        warn!("[ApprovalManager] Timeout local approval: {}", id);
    }
}

pub fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
