use brain_rs::schedule::ScheduleStore;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Proxy wrapper for Codex's proactive scheduling.
/// This service no longer stores data itself; it delegates to Codex.
pub struct CronService {
    codex_store: Arc<ScheduleStore>,
}

impl CronService {
    pub fn new(codex_store: Arc<ScheduleStore>) -> Self {
        Self { codex_store }
    }

    pub async fn list_jobs(&self) -> anyhow::Result<serde_json::Value> {
        let tasks = self.codex_store.list();
        Ok(serde_json::json!({ "tasks": tasks }))
    }

    pub async fn add_job(&self, schedule: &str, prompt: &str) -> anyhow::Result<String> {
        let task = self
            .codex_store
            .add(
                prompt,
                None,
                None,
                Some("cron"),
                Vec::new(),
                Some(prompt),
                Some(schedule),
                None,
                None,
                Some(true),
                None,
            )
            .map_err(anyhow::Error::msg)?;
        Ok(task.id)
    }

    pub async fn remove_job(&self, id: &str) -> anyhow::Result<bool> {
        self.codex_store.remove(id).map_err(anyhow::Error::msg)?;
        Ok(true)
    }

    /// ilhae-proxy's tick loop will call this to find and trigger due tasks.
    pub async fn check_and_trigger(&self) -> Vec<String> {
        // Here we call Codex's autonomy engine or store to find what's due.
        // For now, returning empty as the actual runner logic is being moved to Codex.
        Vec::new()
    }
}
