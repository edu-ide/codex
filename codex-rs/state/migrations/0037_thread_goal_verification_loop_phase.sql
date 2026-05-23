CREATE TABLE thread_goal_loop_history_saved AS
SELECT * FROM thread_goal_loop_history;

DROP TABLE thread_goal_loop_history;

CREATE TABLE thread_goals_new (
    thread_id TEXT PRIMARY KEY NOT NULL REFERENCES threads(id) ON DELETE CASCADE,
    goal_id TEXT NOT NULL,
    objective TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('active', 'paused', 'budget_limited', 'complete')),
    token_budget INTEGER,
    superloop_enabled INTEGER NOT NULL DEFAULT 0,
    tokens_used INTEGER NOT NULL DEFAULT 0,
    time_used_seconds INTEGER NOT NULL DEFAULT 0,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    loop_cycle_number INTEGER NOT NULL DEFAULT 0,
    loop_phase TEXT CHECK(loop_phase IN ('knowledge_loop', 'kairos_loop', 'super_loop', 'plan_loop', 'research_loop', 'decision_loop', 'wiki_loop', 'log_loop', 'improvement_loop', 'cleanup_loop', 'execution_loop', 'verification_loop', 'context_injection')),
    loop_status TEXT CHECK(loop_status IN ('in_progress', 'completed', 'failed')),
    loop_summary TEXT,
    loop_updated_at_ms INTEGER
);

INSERT INTO thread_goals_new (
    thread_id,
    goal_id,
    objective,
    status,
    token_budget,
    superloop_enabled,
    tokens_used,
    time_used_seconds,
    created_at_ms,
    updated_at_ms,
    loop_cycle_number,
    loop_phase,
    loop_status,
    loop_summary,
    loop_updated_at_ms
)
SELECT
    thread_id,
    goal_id,
    objective,
    status,
    token_budget,
    superloop_enabled,
    tokens_used,
    time_used_seconds,
    created_at_ms,
    updated_at_ms,
    loop_cycle_number,
    loop_phase,
    loop_status,
    loop_summary,
    loop_updated_at_ms
FROM thread_goals;

DROP TABLE thread_goals;
ALTER TABLE thread_goals_new RENAME TO thread_goals;

CREATE TABLE thread_goal_loop_history (
    thread_id TEXT NOT NULL REFERENCES thread_goals(thread_id) ON DELETE CASCADE,
    goal_id TEXT NOT NULL,
    id TEXT NOT NULL,
    cycle_number INTEGER NOT NULL,
    phase TEXT NOT NULL CHECK(phase IN ('knowledge_loop', 'kairos_loop', 'super_loop', 'plan_loop', 'research_loop', 'decision_loop', 'wiki_loop', 'log_loop', 'improvement_loop', 'cleanup_loop', 'execution_loop', 'verification_loop', 'context_injection')),
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

INSERT INTO thread_goal_loop_history (
    thread_id,
    goal_id,
    id,
    cycle_number,
    phase,
    status,
    title,
    summary,
    detail,
    error,
    started_at_ms,
    updated_at_ms,
    completed_at_ms
)
SELECT
    thread_id,
    goal_id,
    id,
    cycle_number,
    phase,
    status,
    title,
    summary,
    detail,
    error,
    started_at_ms,
    updated_at_ms,
    completed_at_ms
FROM thread_goal_loop_history_saved;

DROP TABLE thread_goal_loop_history_saved;

CREATE INDEX idx_thread_goal_loop_history_thread_goal_updated
ON thread_goal_loop_history(thread_id, goal_id, updated_at_ms DESC);
