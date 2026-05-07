use super::TurnError;
use crate::RequestId;
use codex_protocol::items::LoopLifecycleItem;
use codex_protocol::protocol::LoopLifecycleKind;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use std::collections::BTreeMap;
use ts_rs::TS;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct DeprecationNoticeNotification {
    /// Concise summary of what is deprecated.
    pub summary: String,
    /// Optional extra guidance, such as migration steps or rationale.
    pub details: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WarningNotification {
    /// Optional thread target when the warning applies to a specific thread.
    pub thread_id: Option<String>,
    /// Concise warning message for the user.
    pub message: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct GuardianWarningNotification {
    /// Thread target for the guardian warning.
    pub thread_id: String,
    /// Concise guardian warning message for the user.
    pub message: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ErrorNotification {
    pub error: TurnError,
    // Set to true if the error is transient and the app-server process will automatically retry.
    // If true, this will not interrupt a turn.
    pub will_retry: bool,
    pub thread_id: String,
    pub turn_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ServerRequestResolvedNotification {
    pub thread_id: String,
    pub request_id: RequestId,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct LoopLifecycleProgressNotification {
    pub thread_id: String,
    pub turn_id: String,
    pub item_id: String,
    pub kind: LoopLifecycleKind,
    pub summary: String,
    pub detail: Option<String>,
    pub counts: Option<BTreeMap<String, i64>>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(tag = "event", rename_all = "snake_case")]
#[ts(tag = "event")]
#[ts(export_to = "v2/")]
pub enum IlhaeLoopLifecycleNotification {
    #[serde(rename_all = "camelCase")]
    #[ts(rename_all = "camelCase")]
    Started {
        session_id: String,
        item: LoopLifecycleItem,
    },
    #[serde(rename_all = "camelCase")]
    #[ts(rename_all = "camelCase")]
    Progress {
        session_id: String,
        item_id: String,
        kind: LoopLifecycleKind,
        summary: String,
        detail: Option<String>,
        counts: Option<BTreeMap<String, i64>>,
    },
    #[serde(rename_all = "camelCase")]
    #[ts(rename_all = "camelCase")]
    Completed {
        session_id: String,
        item: LoopLifecycleItem,
    },
    #[serde(rename_all = "camelCase")]
    #[ts(rename_all = "camelCase")]
    Failed {
        session_id: String,
        item: LoopLifecycleItem,
    },
}
