use serde::Deserialize;
use serde::Serialize;

pub const DEFAULT_LISTEN_ADDR: &str = "127.0.0.1:43290";

pub fn default_listen_addr() -> String {
    std::env::var("ILHAE_GPU_QUEUE_ADDR").unwrap_or_else(|_| DEFAULT_LISTEN_ADDR.to_string())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LeaseMode {
    Exclusive,
    Shared,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LeaseState {
    Granted,
    Pending,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LlmRuntimeState {
    Running,
    Stopped,
    Starting,
    Stopping,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LeaseRequest {
    pub owner: String,
    pub kind: String,
    pub mode: LeaseMode,
    pub preempt_llm: bool,
    pub ttl_seconds: u64,
    pub wait_timeout_seconds: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LeaseInfo {
    pub lease_id: String,
    pub owner: String,
    pub kind: String,
    pub mode: LeaseMode,
    pub state: LeaseState,
    pub preempt_llm: bool,
    pub llm_was_preempted: bool,
    pub ttl_seconds: u64,
    pub queued_at: u64,
    pub granted_at: Option<u64>,
    pub expires_at: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LeaseResponse {
    pub lease_id: String,
    pub state: LeaseState,
    pub llm_was_preempted: bool,
}

impl From<&LeaseInfo> for LeaseResponse {
    fn from(lease: &LeaseInfo) -> Self {
        Self {
            lease_id: lease.lease_id.clone(),
            state: lease.state,
            llm_was_preempted: lease.llm_was_preempted,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseLeaseResponse {
    pub released: LeaseInfo,
    pub promoted: Option<LeaseInfo>,
    pub llm_restarted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusResponse {
    pub uptime_seconds: u64,
    pub llm_state: LlmRuntimeState,
    pub active_lease: Option<LeaseInfo>,
    pub pending_leases: Vec<LeaseInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthResponse {
    pub ok: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmCommandResponse {
    pub state: LlmRuntimeState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorResponse {
    pub error: String,
}
