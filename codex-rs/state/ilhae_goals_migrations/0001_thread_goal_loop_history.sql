ALTER TABLE thread_goals ADD COLUMN superloop_enabled INTEGER NOT NULL DEFAULT 0;
ALTER TABLE thread_goals ADD COLUMN loop_cycle_number INTEGER NOT NULL DEFAULT 0;
ALTER TABLE thread_goals ADD COLUMN loop_phase TEXT CHECK(loop_phase IN ('knowledge_loop', 'kairos_loop', 'super_loop', 'plan_loop', 'brain_research_loop', 'codebase_research_loop', 'agent_skill_research_loop', 'web_research_loop', 'research_loop', 'decision_loop', 'wiki_loop', 'log_loop', 'improvement_loop', 'cleanup_loop', 'execution_loop', 'verification_loop', 'context_injection'));
ALTER TABLE thread_goals ADD COLUMN loop_status TEXT CHECK(loop_status IN ('in_progress', 'completed', 'failed'));
ALTER TABLE thread_goals ADD COLUMN loop_summary TEXT;
ALTER TABLE thread_goals ADD COLUMN loop_updated_at_ms INTEGER;

CREATE TABLE thread_goal_loop_history (
    thread_id TEXT NOT NULL REFERENCES thread_goals(thread_id) ON DELETE CASCADE,
    goal_id TEXT NOT NULL,
    id TEXT NOT NULL,
    cycle_number INTEGER NOT NULL,
    phase TEXT NOT NULL CHECK(phase IN ('knowledge_loop', 'kairos_loop', 'super_loop', 'plan_loop', 'brain_research_loop', 'codebase_research_loop', 'agent_skill_research_loop', 'web_research_loop', 'research_loop', 'decision_loop', 'wiki_loop', 'log_loop', 'improvement_loop', 'cleanup_loop', 'execution_loop', 'verification_loop', 'context_injection')),
    status TEXT NOT NULL CHECK(status IN ('in_progress', 'completed', 'failed')),
    title TEXT NOT NULL,
    summary TEXT NOT NULL,
    detail TEXT,
    error TEXT,
    started_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    completed_at_ms INTEGER,
    PRIMARY KEY (thread_id, goal_id, id)
);

CREATE INDEX idx_thread_goal_loop_history_thread_goal_updated
ON thread_goal_loop_history(thread_id, goal_id, updated_at_ms DESC);
