use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use anyhow::Context;
use axum::Json;
use axum::Router;
use axum::extract::Path;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::get;
use axum::routing::post;
use tokio::sync::Mutex;
use tokio::sync::Notify;

use super::api::ErrorResponse;
use super::api::HealthResponse;
use super::api::LeaseInfo;
use super::api::LeaseRequest;
use super::api::LeaseResponse;
use super::api::LeaseState;
use super::api::LlmCommandResponse;
use super::api::ReleaseLeaseResponse;
use super::api::StatusResponse;
use super::runtime::NativeLlmRuntime;
use super::scheduler::LeaseScheduler;
use super::scheduler::LeaseSchedulerError;

#[derive(Clone)]
pub struct GpuQueueDaemon {
    inner: Arc<GpuQueueDaemonInner>,
}

struct GpuQueueDaemonInner {
    scheduler: Mutex<LeaseScheduler>,
    runtime: NativeLlmRuntime,
    notify: Notify,
    started_at: Instant,
}

impl GpuQueueDaemon {
    pub fn new(runtime: NativeLlmRuntime) -> Self {
        Self {
            inner: Arc::new(GpuQueueDaemonInner {
                scheduler: Mutex::new(LeaseScheduler::new()),
                runtime,
                notify: Notify::new(),
                started_at: Instant::now(),
            }),
        }
    }

    pub async fn acquire_lease(&self, request: LeaseRequest) -> anyhow::Result<LeaseResponse> {
        let wait_timeout_seconds = request.wait_timeout_seconds;
        self.expire_stale_leases().await;
        let lease = {
            let mut scheduler = self.inner.scheduler.lock().await;
            scheduler.request_lease(request, false, now_epoch_secs())?
        };

        if lease.state == LeaseState::Pending {
            let Some(wait_timeout_seconds) = wait_timeout_seconds else {
                return Ok(LeaseResponse::from(&lease));
            };
            return self
                .wait_for_pending_lease(lease, wait_timeout_seconds)
                .await;
        }

        self.preempt_for_granted_lease(&lease).await
    }

    pub async fn release_lease(&self, lease_id: &str) -> anyhow::Result<ReleaseLeaseResponse> {
        let outcome = {
            let mut scheduler = self.inner.scheduler.lock().await;
            scheduler.release_lease(lease_id, now_epoch_secs())?
        };

        let mut promoted = outcome.promoted;
        let mut llm_restarted = false;
        if let Some(promoted_lease) = promoted.as_ref() {
            if outcome.released.llm_was_preempted && promoted_lease.preempt_llm {
                promoted = Some(
                    self.mark_llm_was_preempted(&promoted_lease.lease_id, true)
                        .await?,
                );
            } else if promoted_lease.preempt_llm {
                promoted = Some(self.preempt_lease_by_id(&promoted_lease.lease_id).await?);
            }
        } else if outcome.released.llm_was_preempted {
            self.inner.runtime.start().await?;
            llm_restarted = true;
        }
        self.inner.notify.notify_waiters();

        Ok(ReleaseLeaseResponse {
            released: outcome.released,
            promoted,
            llm_restarted,
        })
    }

    pub async fn heartbeat_lease(&self, lease_id: &str) -> anyhow::Result<LeaseResponse> {
        let lease = {
            let mut scheduler = self.inner.scheduler.lock().await;
            scheduler.heartbeat_lease(lease_id, now_epoch_secs())?
        };
        Ok(LeaseResponse::from(&lease))
    }

    pub async fn status(&self) -> StatusResponse {
        self.expire_stale_leases().await;
        let mut status = {
            let scheduler = self.inner.scheduler.lock().await;
            scheduler.status(now_epoch_secs())
        };
        status.uptime_seconds = self.inner.started_at.elapsed().as_secs();
        status.llm_state = self.inner.runtime.state().await;
        status
    }

    pub async fn llm_start(&self) -> anyhow::Result<LlmCommandResponse> {
        self.inner.runtime.start().await?;
        Ok(LlmCommandResponse {
            state: self.inner.runtime.state().await,
        })
    }

    pub async fn llm_stop(&self) -> anyhow::Result<LlmCommandResponse> {
        self.inner.runtime.stop().await?;
        Ok(LlmCommandResponse {
            state: self.inner.runtime.state().await,
        })
    }

    pub async fn llm_restart(&self) -> anyhow::Result<LlmCommandResponse> {
        self.inner.runtime.restart().await?;
        Ok(LlmCommandResponse {
            state: self.inner.runtime.state().await,
        })
    }

    async fn wait_for_pending_lease(
        &self,
        lease: LeaseInfo,
        wait_timeout_seconds: u64,
    ) -> anyhow::Result<LeaseResponse> {
        let timeout = Duration::from_secs(wait_timeout_seconds);
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(active) = self.active_lease_if(&lease.lease_id).await {
                return self.preempt_for_granted_lease(&active).await;
            }

            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                let mut scheduler = self.inner.scheduler.lock().await;
                let _ = scheduler.release_lease(&lease.lease_id, now_epoch_secs());
                anyhow::bail!(
                    "timed out waiting for GPU lease `{}` after {}s",
                    lease.lease_id,
                    wait_timeout_seconds
                );
            }

            if tokio::time::timeout(remaining, self.inner.notify.notified())
                .await
                .is_err()
            {
                let mut scheduler = self.inner.scheduler.lock().await;
                let _ = scheduler.release_lease(&lease.lease_id, now_epoch_secs());
                anyhow::bail!(
                    "timed out waiting for GPU lease `{}` after {}s",
                    lease.lease_id,
                    wait_timeout_seconds
                );
            }
        }
    }

    async fn preempt_for_granted_lease(&self, lease: &LeaseInfo) -> anyhow::Result<LeaseResponse> {
        if !lease.preempt_llm {
            return Ok(LeaseResponse::from(lease));
        }

        let was_running = self.inner.runtime.is_running().await?;
        if was_running {
            if let Err(err) = self.inner.runtime.stop().await {
                let mut scheduler = self.inner.scheduler.lock().await;
                let _ = scheduler.release_lease(&lease.lease_id, now_epoch_secs());
                self.inner.notify.notify_waiters();
                return Err(err).with_context(|| {
                    format!(
                        "failed to stop local LLM runtime before granting GPU lease `{}`",
                        lease.lease_id
                    )
                });
            }
            return Ok(LeaseResponse::from(
                &self.mark_llm_was_preempted(&lease.lease_id, true).await?,
            ));
        }

        Ok(LeaseResponse::from(lease))
    }

    async fn preempt_lease_by_id(&self, lease_id: &str) -> anyhow::Result<LeaseInfo> {
        let Some(lease) = self.active_lease_if(lease_id).await else {
            anyhow::bail!("GPU lease `{lease_id}` is not active");
        };
        let response = self.preempt_for_granted_lease(&lease).await?;
        let Some(lease) = self.lease(&response.lease_id).await else {
            anyhow::bail!(
                "GPU lease `{}` disappeared during preemption",
                response.lease_id
            );
        };
        Ok(lease)
    }

    async fn mark_llm_was_preempted(
        &self,
        lease_id: &str,
        llm_was_preempted: bool,
    ) -> anyhow::Result<LeaseInfo> {
        let mut scheduler = self.inner.scheduler.lock().await;
        Ok(scheduler.mark_llm_was_preempted(lease_id, llm_was_preempted)?)
    }

    async fn lease(&self, lease_id: &str) -> Option<LeaseInfo> {
        let scheduler = self.inner.scheduler.lock().await;
        scheduler.lease(lease_id)
    }

    async fn active_lease_if(&self, lease_id: &str) -> Option<LeaseInfo> {
        let scheduler = self.inner.scheduler.lock().await;
        scheduler
            .status(now_epoch_secs())
            .active_lease
            .filter(|lease| lease.lease_id == lease_id)
    }

    async fn expire_stale_leases(&self) {
        let expired = {
            let mut scheduler = self.inner.scheduler.lock().await;
            scheduler.expire_leases(now_epoch_secs())
        };
        if expired.is_empty() {
            return;
        }

        let status = {
            let scheduler = self.inner.scheduler.lock().await;
            scheduler.status(now_epoch_secs())
        };
        if let Some(active) = status.active_lease.as_ref()
            && active.preempt_llm
        {
            let _ = self.mark_llm_was_preempted(&active.lease_id, true).await;
            self.inner.notify.notify_waiters();
            return;
        }
        if expired.iter().any(|lease| lease.llm_was_preempted) {
            let _ = self.inner.runtime.start().await;
        }
        self.inner.notify.notify_waiters();
    }
}

pub fn router(daemon: GpuQueueDaemon) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/status", get(status))
        .route("/leases", post(acquire_lease))
        .route("/leases/:lease_id/heartbeat", post(heartbeat_lease))
        .route("/leases/:lease_id/release", post(release_lease))
        .route("/llm/start", post(llm_start))
        .route("/llm/stop", post(llm_stop))
        .route("/llm/restart", post(llm_restart))
        .with_state(daemon)
}

pub async fn run_daemon(listen: &str, profile_id: Option<String>) -> anyhow::Result<()> {
    let addr: SocketAddr = listen
        .parse()
        .with_context(|| format!("invalid GPU queue listen address `{listen}`"))?;
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind GPU queue daemon to {addr}"))?;
    println!("GPU queue daemon listening on http://{addr}");
    axum::serve(
        listener,
        router(GpuQueueDaemon::new(NativeLlmRuntime::new(profile_id))),
    )
    .await?;
    Ok(())
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse { ok: true })
}

async fn status(State(daemon): State<GpuQueueDaemon>) -> Json<StatusResponse> {
    Json(daemon.status().await)
}

async fn acquire_lease(
    State(daemon): State<GpuQueueDaemon>,
    Json(request): Json<LeaseRequest>,
) -> HandlerResult<LeaseResponse> {
    daemon
        .acquire_lease(request)
        .await
        .map(Json)
        .map_err(api_error)
}

async fn heartbeat_lease(
    State(daemon): State<GpuQueueDaemon>,
    Path(lease_id): Path<String>,
) -> HandlerResult<LeaseResponse> {
    daemon
        .heartbeat_lease(&lease_id)
        .await
        .map(Json)
        .map_err(api_error)
}

async fn release_lease(
    State(daemon): State<GpuQueueDaemon>,
    Path(lease_id): Path<String>,
) -> HandlerResult<ReleaseLeaseResponse> {
    daemon
        .release_lease(&lease_id)
        .await
        .map(Json)
        .map_err(api_error)
}

async fn llm_start(State(daemon): State<GpuQueueDaemon>) -> HandlerResult<LlmCommandResponse> {
    daemon.llm_start().await.map(Json).map_err(api_error)
}

async fn llm_stop(State(daemon): State<GpuQueueDaemon>) -> HandlerResult<LlmCommandResponse> {
    daemon.llm_stop().await.map(Json).map_err(api_error)
}

async fn llm_restart(State(daemon): State<GpuQueueDaemon>) -> HandlerResult<LlmCommandResponse> {
    daemon.llm_restart().await.map(Json).map_err(api_error)
}

type HandlerResult<T> = Result<Json<T>, (StatusCode, Json<ErrorResponse>)>;

fn api_error(error: anyhow::Error) -> (StatusCode, Json<ErrorResponse>) {
    let status = if let Some(scheduler_error) = error.downcast_ref::<LeaseSchedulerError>() {
        match scheduler_error {
            LeaseSchedulerError::InvalidRequest(_) => StatusCode::BAD_REQUEST,
            LeaseSchedulerError::LeaseNotFound(_) => StatusCode::NOT_FOUND,
        }
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    };
    (
        status,
        Json(ErrorResponse {
            error: error.to_string(),
        }),
    )
}

fn now_epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
