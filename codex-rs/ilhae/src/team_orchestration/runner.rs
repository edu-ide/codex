use std::sync::Arc;
use codex_core::collaboration::planner::{CollaborationPlanner, TeamPlan};
use crate::a2a_transport::A2ATransport;

/// Proxy-level Team Runner.
/// It no longer 'thinks' about the plan; it executes the plan provided by Codex.
pub struct TeamRunner {
    codex_planner: Arc<CollaborationPlanner>,
    transport: Arc<A2ATransport>,
}

use std::collections::HashSet;
use futures::stream::{FuturesUnordered, StreamExt};

impl TeamRunner {
    pub fn new(codex_planner: Arc<CollaborationPlanner>, transport: Arc<A2ATransport>) -> Self {
        Self {
            codex_planner,
            transport,
        }
    }

    /// Run mission with advanced parallel orchestration.
    pub async fn run_mission(&self, mission: &str) -> anyhow::Result<()> {
        // 1. Get the complex DAG plan from Codex
        let mut plan = self.codex_planner.create_plan(mission).await?;
        let mut completed_tasks = HashSet::new();
        let mut in_progress = FuturesUnordered::new();

        while completed_tasks.len() < plan.sub_tasks.len() {
            // Find tasks that are ready to run (all dependencies met and not yet started)
            for task in plan.sub_tasks.iter_mut() {
                if matches!(task.status, TaskStatus::Pending) && 
                   task.dependencies.iter().all(|dep| completed_tasks.contains(dep)) {
                    
                    info!("[Orchestrator] Dispatching {} to {}", task.id, task.assigned_agent);
                    task.status = TaskStatus::Running;
                    
                    let transport = self.transport.clone();
                    let agent_id = task.assigned_agent.clone();
                    let instruction = task.instruction.clone();
                    let task_id = task.id.clone();

                    in_progress.push(tokio::spawn(async move {
                        let result = transport.send_to_agent(&agent_id, &instruction).await;
                        (task_id, result)
                    }));
                }
            }

            // Wait for any running task to complete
            if let Some(finished) = in_progress.next().await {
                let (task_id, result) = finished?;
                if result.is_ok() {
                    info!("[Orchestrator] Task {} completed successfully", task_id);
                    completed_tasks.insert(task_id.clone());
                    if let Some(task) = plan.sub_tasks.iter_mut().find(|t| t.id == task_id) {
                        task.status = TaskStatus::Done;
                    }
                } else {
                    error!("[Orchestrator] Task {} failed", task_id);
                    // Handle failure/retries here
                }
            }
        }

        info!("[Orchestrator] Mission '{}' accomplished!", plan.goal);
        Ok(())
    }
}

use tracing::{info, error};
