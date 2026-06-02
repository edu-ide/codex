use anyhow::Result;
use anyhow::anyhow;
use chrono::DateTime;
use chrono::Utc;
use codex_protocol::ThreadId;
use sqlx::Row;
use sqlx::sqlite::SqliteRow;

use super::epoch_millis_to_datetime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadGoalStatus {
    Active,
    Paused,
    Blocked,
    UsageLimited,
    BudgetLimited,
    Complete,
}

impl ThreadGoalStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Paused => "paused",
            Self::Blocked => "blocked",
            Self::UsageLimited => "usage_limited",
            Self::BudgetLimited => "budget_limited",
            Self::Complete => "complete",
        }
    }

    pub fn is_active(self) -> bool {
        self == Self::Active
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, Self::BudgetLimited | Self::Complete)
    }
}

impl TryFrom<&str> for ThreadGoalStatus {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self> {
        match value {
            "active" => Ok(Self::Active),
            "paused" => Ok(Self::Paused),
            "blocked" => Ok(Self::Blocked),
            "usage_limited" => Ok(Self::UsageLimited),
            "budget_limited" => Ok(Self::BudgetLimited),
            "complete" => Ok(Self::Complete),
            other => Err(anyhow!("unknown thread goal status `{other}`")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadGoalLoopPhase {
    KnowledgeLoop,
    KairosLoop,
    SuperLoop,
    PlanLoop,
    BrainResearchLoop,
    CodebaseResearchLoop,
    AgentSkillResearchLoop,
    WebResearchLoop,
    ResearchLoop,
    DecisionLoop,
    WikiLoop,
    LogLoop,
    ImprovementLoop,
    CleanupLoop,
    ExecutionLoop,
    VerificationLoop,
    ContextInjection,
}

impl ThreadGoalLoopPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::KnowledgeLoop => "knowledge_loop",
            Self::KairosLoop => "kairos_loop",
            Self::SuperLoop => "super_loop",
            Self::PlanLoop => "plan_loop",
            Self::BrainResearchLoop => "brain_research_loop",
            Self::CodebaseResearchLoop => "codebase_research_loop",
            Self::AgentSkillResearchLoop => "agent_skill_research_loop",
            Self::WebResearchLoop => "web_research_loop",
            Self::ResearchLoop => "research_loop",
            Self::DecisionLoop => "decision_loop",
            Self::WikiLoop => "wiki_loop",
            Self::LogLoop => "log_loop",
            Self::ImprovementLoop => "improvement_loop",
            Self::CleanupLoop => "cleanup_loop",
            Self::ExecutionLoop => "execution_loop",
            Self::VerificationLoop => "verification_loop",
            Self::ContextInjection => "context_injection",
        }
    }
}

impl TryFrom<&str> for ThreadGoalLoopPhase {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self> {
        match value {
            "knowledge_loop" => Ok(Self::KnowledgeLoop),
            "kairos_loop" => Ok(Self::KairosLoop),
            "super_loop" => Ok(Self::SuperLoop),
            "plan_loop" => Ok(Self::PlanLoop),
            "brain_research_loop" => Ok(Self::BrainResearchLoop),
            "codebase_research_loop" => Ok(Self::CodebaseResearchLoop),
            "agent_skill_research_loop" => Ok(Self::AgentSkillResearchLoop),
            "web_research_loop" => Ok(Self::WebResearchLoop),
            "research_loop" => Ok(Self::ResearchLoop),
            "decision_loop" => Ok(Self::DecisionLoop),
            "wiki_loop" => Ok(Self::WikiLoop),
            "log_loop" => Ok(Self::LogLoop),
            "improvement_loop" => Ok(Self::ImprovementLoop),
            "cleanup_loop" => Ok(Self::CleanupLoop),
            "execution_loop" => Ok(Self::ExecutionLoop),
            "verification_loop" => Ok(Self::VerificationLoop),
            "context_injection" => Ok(Self::ContextInjection),
            other => Err(anyhow!("unknown thread goal loop phase `{other}`")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadGoalLoopStatus {
    InProgress,
    Completed,
    Failed,
}

impl ThreadGoalLoopStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InProgress => "in_progress",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed)
    }
}

impl TryFrom<&str> for ThreadGoalLoopStatus {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self> {
        match value {
            "in_progress" => Ok(Self::InProgress),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            other => Err(anyhow!("unknown thread goal loop status `{other}`")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadGoalLoopState {
    pub cycle_number: i64,
    pub phase: ThreadGoalLoopPhase,
    pub status: ThreadGoalLoopStatus,
    pub summary: String,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadGoalLoopHistoryEntry {
    pub id: String,
    pub cycle_number: i64,
    pub phase: ThreadGoalLoopPhase,
    pub status: ThreadGoalLoopStatus,
    pub title: String,
    pub summary: String,
    pub detail: Option<String>,
    pub error: Option<String>,
    pub started_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadGoalLoopEvent {
    pub id: String,
    pub phase: ThreadGoalLoopPhase,
    pub status: ThreadGoalLoopStatus,
    pub title: String,
    pub summary: String,
    pub detail: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadGoal {
    pub thread_id: ThreadId,
    pub goal_id: String,
    pub objective: String,
    pub status: ThreadGoalStatus,
    pub token_budget: Option<i64>,
    pub superloop_enabled: bool,
    pub tokens_used: i64,
    pub time_used_seconds: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub loop_state: Option<ThreadGoalLoopState>,
    pub loop_history: Vec<ThreadGoalLoopHistoryEntry>,
}

pub(crate) struct ThreadGoalRow {
    pub thread_id: String,
    pub goal_id: String,
    pub objective: String,
    pub status: String,
    pub token_budget: Option<i64>,
    pub superloop_enabled: i64,
    pub tokens_used: i64,
    pub time_used_seconds: i64,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub loop_cycle_number: i64,
    pub loop_phase: Option<String>,
    pub loop_status: Option<String>,
    pub loop_summary: Option<String>,
    pub loop_updated_at_ms: Option<i64>,
}

impl ThreadGoalRow {
    pub(crate) fn try_from_row(row: &SqliteRow) -> Result<Self> {
        Ok(Self {
            thread_id: row.try_get("thread_id")?,
            goal_id: row.try_get("goal_id")?,
            objective: row.try_get("objective")?,
            status: row.try_get("status")?,
            token_budget: row.try_get("token_budget")?,
            superloop_enabled: row.try_get("superloop_enabled")?,
            tokens_used: row.try_get("tokens_used")?,
            time_used_seconds: row.try_get("time_used_seconds")?,
            created_at_ms: row.try_get("created_at_ms")?,
            updated_at_ms: row.try_get("updated_at_ms")?,
            loop_cycle_number: row.try_get("loop_cycle_number")?,
            loop_phase: row.try_get("loop_phase")?,
            loop_status: row.try_get("loop_status")?,
            loop_summary: row.try_get("loop_summary")?,
            loop_updated_at_ms: row.try_get("loop_updated_at_ms")?,
        })
    }
}

pub(crate) struct ThreadGoalLoopHistoryRow {
    pub id: String,
    pub cycle_number: i64,
    pub phase: String,
    pub status: String,
    pub title: String,
    pub summary: String,
    pub detail: Option<String>,
    pub error: Option<String>,
    pub started_at_ms: i64,
    pub updated_at_ms: i64,
    pub completed_at_ms: Option<i64>,
}

impl ThreadGoalLoopHistoryRow {
    pub(crate) fn try_from_row(row: &SqliteRow) -> Result<Self> {
        Ok(Self {
            id: row.try_get("id")?,
            cycle_number: row.try_get("cycle_number")?,
            phase: row.try_get("phase")?,
            status: row.try_get("status")?,
            title: row.try_get("title")?,
            summary: row.try_get("summary")?,
            detail: row.try_get("detail")?,
            error: row.try_get("error")?,
            started_at_ms: row.try_get("started_at_ms")?,
            updated_at_ms: row.try_get("updated_at_ms")?,
            completed_at_ms: row.try_get("completed_at_ms")?,
        })
    }
}

impl TryFrom<ThreadGoalLoopHistoryRow> for ThreadGoalLoopHistoryEntry {
    type Error = anyhow::Error;

    fn try_from(row: ThreadGoalLoopHistoryRow) -> Result<Self> {
        Ok(Self {
            id: row.id,
            cycle_number: row.cycle_number,
            phase: ThreadGoalLoopPhase::try_from(row.phase.as_str())?,
            status: ThreadGoalLoopStatus::try_from(row.status.as_str())?,
            title: row.title,
            summary: row.summary,
            detail: row.detail,
            error: row.error,
            started_at: epoch_millis_to_datetime(row.started_at_ms)?,
            updated_at: epoch_millis_to_datetime(row.updated_at_ms)?,
            completed_at: row
                .completed_at_ms
                .map(epoch_millis_to_datetime)
                .transpose()?,
        })
    }
}

impl TryFrom<ThreadGoalRow> for ThreadGoal {
    type Error = anyhow::Error;

    fn try_from(row: ThreadGoalRow) -> Result<Self> {
        let loop_state = match (
            row.loop_phase,
            row.loop_status,
            row.loop_summary,
            row.loop_updated_at_ms,
        ) {
            (Some(phase), Some(status), Some(summary), Some(updated_at_ms)) => {
                Some(ThreadGoalLoopState {
                    cycle_number: row.loop_cycle_number,
                    phase: ThreadGoalLoopPhase::try_from(phase.as_str())?,
                    status: ThreadGoalLoopStatus::try_from(status.as_str())?,
                    summary,
                    updated_at: epoch_millis_to_datetime(updated_at_ms)?,
                })
            }
            _ => None,
        };
        Ok(Self {
            thread_id: ThreadId::try_from(row.thread_id)?,
            goal_id: row.goal_id,
            objective: row.objective,
            status: ThreadGoalStatus::try_from(row.status.as_str())?,
            token_budget: row.token_budget,
            superloop_enabled: row.superloop_enabled != 0,
            tokens_used: row.tokens_used,
            time_used_seconds: row.time_used_seconds,
            created_at: epoch_millis_to_datetime(row.created_at_ms)?,
            updated_at: epoch_millis_to_datetime(row.updated_at_ms)?,
            loop_state,
            loop_history: Vec::new(),
        })
    }
}
