use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TeamTimelineKind {
    UserPrompt,
    DelegationStarted,
    TaskSubmitted,
    TaskStatus,
    AgentResponse,
    DelegationCompleted,
    LeaderFinal,
    SystemNotice,
}

impl TeamTimelineKind {
    pub fn default_channel_id(self) -> &'static str {
        match self {
            Self::UserPrompt => "team",
            Self::DelegationStarted => "a2a:delegation_start",
            Self::TaskSubmitted => "a2a:task_submitted",
            Self::TaskStatus => "a2a:task_status",
            Self::AgentResponse => "team",
            Self::DelegationCompleted => "a2a:delegation_complete",
            Self::LeaderFinal => "team",
            Self::SystemNotice => "system",
        }
    }

    pub fn priority(self) -> i32 {
        match self {
            Self::UserPrompt => 0,
            Self::DelegationStarted => 10,
            Self::TaskSubmitted => 20,
            Self::TaskStatus => 30,
            Self::AgentResponse => 40,
            Self::DelegationCompleted => 50,
            Self::LeaderFinal => 60,
            Self::SystemNotice => 100,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamTimelineEvent {
    pub kind: TeamTimelineKind,
    pub role: String,
    pub content: String,
    pub agent_id: String,
    pub thinking: String,
    pub tool_calls_json: String,
    pub content_blocks_json: String,
    pub channel_id: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub total_tokens: i64,
    pub duration_ms: i64,
    pub metadata: serde_json::Value,
}

impl TeamTimelineEvent {
    pub fn new(
        kind: TeamTimelineKind,
        role: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            role: role.into(),
            content: content.into(),
            agent_id: String::new(),
            thinking: String::new(),
            tool_calls_json: String::new(),
            content_blocks_json: String::new(),
            channel_id: kind.default_channel_id().to_string(),
            input_tokens: 0,
            output_tokens: 0,
            total_tokens: 0,
            duration_ms: 0,
            metadata: serde_json::Value::Null,
        }
    }

    pub fn with_agent_id(mut self, agent_id: impl Into<String>) -> Self {
        self.agent_id = agent_id.into();
        self
    }

    pub fn with_channel_id(mut self, channel_id: impl Into<String>) -> Self {
        self.channel_id = channel_id.into();
        self
    }

    pub fn with_content_blocks_json(mut self, content_blocks_json: impl Into<String>) -> Self {
        self.content_blocks_json = content_blocks_json.into();
        self
    }

    pub fn with_tool_calls_json(mut self, tool_calls_json: impl Into<String>) -> Self {
        self.tool_calls_json = tool_calls_json.into();
        self
    }

    pub fn with_metadata(mut self, metadata: serde_json::Value) -> Self {
        self.metadata = metadata;
        self
    }
}
