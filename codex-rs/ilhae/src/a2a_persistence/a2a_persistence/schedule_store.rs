use std::sync::Arc;

use a2a_rs::proxy::extract_text_from_parts;
use a2a_rs::store::InMemoryTaskStore;
use a2a_rs::types::*;

use crate::session_store::SessionStore;

// ═════════════════════════════════════════════════════════════════════
// PersistenceScheduleStore — dual-write ScheduleStore
// ═════════════════════════════════════════════════════════════════════

/// ScheduleStore that writes to both in-memory store and ilhae SessionStore.
pub struct PersistenceScheduleStore {
    inner: InMemoryTaskStore,
    store: Arc<SessionStore>,
    #[allow(dead_code)]
    role: String,
}

impl PersistenceScheduleStore {
    pub fn new(store: Arc<SessionStore>, role: String) -> Self {
        Self {
            inner: InMemoryTaskStore::new(),
            store,
            role,
        }
    }
}

impl a2a_rs::store::TaskStore for PersistenceScheduleStore {
    async fn save(&self, task: Task) -> Result<(), a2a_rs::error::A2AError> {
        // Save to in-memory store
        self.inner.save(task.clone()).await?;

        // Also persist to SessionStore
        let state_str = format!("{:?}", task.status.state).to_lowercase();
        let result_text = task
            .status
            .message
            .as_ref()
            .map(|m| extract_text_from_parts(&m.parts))
            .unwrap_or_default();

        let _ = self.store.update_task_status(
            &task.id,
            &state_str,
            if result_text.is_empty() {
                None
            } else {
                Some(&result_text)
            },
        );

        Ok(())
    }

    async fn load(&self, schedule_id: &str) -> Result<Option<Task>, a2a_rs::error::A2AError> {
        self.inner.load(schedule_id).await
    }

    async fn list(
        &self,
        params: a2a_rs::store::TaskListParams,
    ) -> Result<a2a_rs::store::TaskListResponse, a2a_rs::error::A2AError> {
        self.inner.list(params).await
    }
}
