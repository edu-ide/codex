//! Task storage trait and in-memory implementation.
//!
//! Mirrors `a2a-js/src/server/store.ts`.

use crate::types::{ListTasksRequest, ListTasksResponse, Task};

/// Storage provider for tasks.
///
/// Implement this trait to use a custom backend (database, Redis, etc.).
pub trait TaskStore: Send + Sync + 'static {
    /// Save (upsert) a task.
    fn save(
        &self,
        task: Task,
    ) -> impl std::future::Future<Output = Result<(), crate::error::A2AError>> + Send;

    /// Load a task by ID. Returns `None` if not found.
    fn load(
        &self,
        task_id: &str,
    ) -> impl std::future::Future<Output = Result<Option<Task>, crate::error::A2AError>> + Send;

    /// List tasks with filtering and cursor-based pagination (v1.0).
    fn list(
        &self,
        _request: &ListTasksRequest,
    ) -> impl std::future::Future<Output = Result<ListTasksResponse, crate::error::A2AError>> + Send
    {
        async {
            Ok(ListTasksResponse {
                tasks: vec![],
                next_page_token: String::new(),
                page_size: 0,
                total_size: 0,
            })
        }
    }
}

/// In-memory task store (default).
pub struct InMemoryTaskStore {
    store: tokio::sync::Mutex<std::collections::HashMap<String, Task>>,
}

impl InMemoryTaskStore {
    pub fn new() -> Self {
        Self {
            store: tokio::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }
}

impl Default for InMemoryTaskStore {
    fn default() -> Self {
        Self::new()
    }
}

impl TaskStore for InMemoryTaskStore {
    async fn save(&self, task: Task) -> Result<(), crate::error::A2AError> {
        self.store.lock().await.insert(task.id.clone(), task);
        Ok(())
    }

    async fn load(&self, task_id: &str) -> Result<Option<Task>, crate::error::A2AError> {
        Ok(self.store.lock().await.get(task_id).cloned())
    }

    async fn list(
        &self,
        request: &ListTasksRequest,
    ) -> Result<ListTasksResponse, crate::error::A2AError> {
        let guard = self.store.lock().await;
        let mut tasks: Vec<Task> = guard
            .values()
            .filter(|task| {
                if let Some(ref ctx) = request.context_id {
                    if &task.context_id != ctx {
                        return false;
                    }
                }
                if let Some(ref status) = request.status {
                    if &task.status.state != status {
                        return false;
                    }
                }
                true
            })
            .cloned()
            .collect();

        // Sort by last_modified descending (newest first).
        tasks.sort_by(|a, b| {
            b.last_modified
                .as_deref()
                .unwrap_or("")
                .cmp(a.last_modified.as_deref().unwrap_or(""))
        });

        let total_size = tasks.len() as i32;
        let page_size = request.page_size.unwrap_or(50).min(100).max(1);

        // Cursor-based pagination: page_token is the offset index as string.
        let offset: usize = request
            .page_token
            .as_deref()
            .and_then(|t| t.parse().ok())
            .unwrap_or(0);

        let page: Vec<Task> = tasks
            .into_iter()
            .skip(offset)
            .take(page_size as usize)
            .map(|mut task| {
                if !request.include_artifacts.unwrap_or(false) {
                    task.artifacts.clear();
                }
                if let Some(max_history) = request.history_length {
                    let len = task.history.len();
                    if max_history >= 0 && (max_history as usize) < len {
                        task.history = task.history.split_off(len - max_history as usize);
                    }
                }
                task
            })
            .collect();

        let next_offset = offset + page.len();
        let next_page_token = if (next_offset as i32) < total_size {
            next_offset.to_string()
        } else {
            String::new()
        };

        Ok(ListTasksResponse {
            tasks: page,
            next_page_token,
            page_size,
            total_size,
        })
    }
}

