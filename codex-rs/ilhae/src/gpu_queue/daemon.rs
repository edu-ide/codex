use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
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
use axum::response::sse::Event;
use axum::response::sse::KeepAlive;
use axum::response::sse::Sse;
use axum::routing::get;
use axum::routing::post;
use tokio::sync::Mutex;
use tokio::sync::Notify;
use tokio::sync::broadcast;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;

use super::api::ErrorResponse;
use super::api::GpuQueueRuntimeEvent;
use super::api::GpuQueueRuntimeEventType;
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
    event_tx: broadcast::Sender<GpuQueueRuntimeEvent>,
    event_sequence: AtomicU64,
    started_at: Instant,
}

impl GpuQueueDaemon {
    pub fn new(runtime: NativeLlmRuntime) -> Self {
        let (event_tx, _) = broadcast::channel(256);
        Self {
            inner: Arc::new(GpuQueueDaemonInner {
                scheduler: Mutex::new(LeaseScheduler::new()),
                runtime,
                notify: Notify::new(),
                event_tx,
                event_sequence: AtomicU64::new(1),
                started_at: Instant::now(),
            }),
        }
    }

    pub fn subscribe_events(&self) -> broadcast::Receiver<GpuQueueRuntimeEvent> {
        self.inner.event_tx.subscribe()
    }

    pub async fn acquire_lease(&self, request: LeaseRequest) -> anyhow::Result<LeaseResponse> {
        let wait_timeout_seconds = request.wait_timeout_seconds;
        self.expire_stale_leases().await;
        let lease = {
            let mut scheduler = self.inner.scheduler.lock().await;
            scheduler.request_lease(request, false, now_epoch_secs())?
        };

        if lease.state == LeaseState::Pending {
            self.emit_runtime_event(
                GpuQueueRuntimeEventType::LeaseQueued,
                format!("GPU queue queued {}", lease_summary(&lease)),
                Some(lease.clone()),
            )
            .await;
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

        self.emit_runtime_event(
            GpuQueueRuntimeEventType::LeaseReleased,
            format!("GPU queue released {}", lease_summary(&outcome.released)),
            Some(outcome.released.clone()),
        )
        .await;

        let mut promoted = outcome.promoted;
        let mut llm_restarted = false;
        if let Some(promoted_lease) = promoted.as_ref() {
            if outcome.released.llm_was_preempted && promoted_lease.preempt_llm {
                let promoted_lease = self
                    .mark_llm_was_preempted(&promoted_lease.lease_id, true)
                    .await?;
                self.emit_runtime_event(
                    GpuQueueRuntimeEventType::LeaseGranted,
                    format!("GPU queue granted {}", lease_summary(&promoted_lease)),
                    Some(promoted_lease.clone()),
                )
                .await;
                promoted = Some(promoted_lease);
            } else if promoted_lease.preempt_llm {
                promoted = Some(self.preempt_lease_by_id(&promoted_lease.lease_id).await?);
            } else {
                self.emit_runtime_event(
                    GpuQueueRuntimeEventType::LeaseGranted,
                    format!("GPU queue granted {}", lease_summary(promoted_lease)),
                    Some(promoted_lease.clone()),
                )
                .await;
            }
        } else if outcome.released.llm_was_preempted {
            self.emit_runtime_event(
                GpuQueueRuntimeEventType::LlmStarting,
                "Restarting local LLM runtime after GPU lease release",
                Some(outcome.released.clone()),
            )
            .await;
            if let Err(err) = self.inner.runtime.start().await {
                self.emit_runtime_event(
                    GpuQueueRuntimeEventType::LlmStartFailed,
                    "Failed to restart local LLM runtime after GPU lease release",
                    Some(outcome.released.clone()),
                )
                .await;
                return Err(err);
            }
            llm_restarted = true;
            self.emit_runtime_event(
                GpuQueueRuntimeEventType::LlmRunning,
                "Local LLM runtime is running after GPU lease release",
                Some(outcome.released.clone()),
            )
            .await;
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
        self.emit_runtime_event(
            GpuQueueRuntimeEventType::LlmStarting,
            "Starting local LLM runtime",
            None,
        )
        .await;
        if let Err(err) = self.inner.runtime.start().await {
            self.emit_runtime_event(
                GpuQueueRuntimeEventType::LlmStartFailed,
                "Failed to start local LLM runtime",
                None,
            )
            .await;
            return Err(err);
        }
        self.emit_runtime_event(
            GpuQueueRuntimeEventType::LlmRunning,
            "Local LLM runtime is running",
            None,
        )
        .await;
        Ok(LlmCommandResponse {
            state: self.inner.runtime.state().await,
        })
    }

    pub async fn llm_stop(&self) -> anyhow::Result<LlmCommandResponse> {
        self.emit_runtime_event(
            GpuQueueRuntimeEventType::LlmStopping,
            "Stopping local LLM runtime",
            None,
        )
        .await;
        if let Err(err) = self.inner.runtime.stop().await {
            self.emit_runtime_event(
                GpuQueueRuntimeEventType::LlmStopFailed,
                "Failed to stop local LLM runtime",
                None,
            )
            .await;
            return Err(err);
        }
        self.emit_runtime_event(
            GpuQueueRuntimeEventType::LlmStopped,
            "Local LLM runtime is stopped",
            None,
        )
        .await;
        Ok(LlmCommandResponse {
            state: self.inner.runtime.state().await,
        })
    }

    pub async fn llm_restart(&self) -> anyhow::Result<LlmCommandResponse> {
        self.llm_stop().await?;
        self.llm_start().await
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
            self.emit_runtime_event(
                GpuQueueRuntimeEventType::LeaseGranted,
                format!("GPU queue granted {}", lease_summary(lease)),
                Some(lease.clone()),
            )
            .await;
            return Ok(LeaseResponse::from(lease));
        }

        let was_running = self.inner.runtime.is_running().await?;
        if was_running {
            self.emit_runtime_event(
                GpuQueueRuntimeEventType::LlmStopping,
                format!(
                    "Stopping local LLM runtime before granting {}",
                    lease_summary(lease)
                ),
                Some(lease.clone()),
            )
            .await;
            if let Err(err) = self.inner.runtime.stop().await {
                self.emit_runtime_event(
                    GpuQueueRuntimeEventType::LlmStopFailed,
                    format!(
                        "Failed to stop local LLM runtime before granting {}",
                        lease_summary(lease)
                    ),
                    Some(lease.clone()),
                )
                .await;
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
            let lease = self.mark_llm_was_preempted(&lease.lease_id, true).await?;
            self.emit_runtime_event(
                GpuQueueRuntimeEventType::LlmStopped,
                format!("Local LLM runtime stopped for {}", lease_summary(&lease)),
                Some(lease.clone()),
            )
            .await;
            self.emit_runtime_event(
                GpuQueueRuntimeEventType::LeaseGranted,
                format!("GPU queue granted {}", lease_summary(&lease)),
                Some(lease.clone()),
            )
            .await;
            return Ok(LeaseResponse::from(&lease));
        }

        self.emit_runtime_event(
            GpuQueueRuntimeEventType::LeaseGranted,
            format!("GPU queue granted {}", lease_summary(lease)),
            Some(lease.clone()),
        )
        .await;
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

        for lease in &expired {
            self.emit_runtime_event(
                GpuQueueRuntimeEventType::LeaseExpired,
                format!("GPU queue expired {}", lease_summary(lease)),
                Some(lease.clone()),
            )
            .await;
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
            self.emit_runtime_event(
                GpuQueueRuntimeEventType::LlmStarting,
                "Restarting local LLM runtime after GPU lease expiration",
                expired.first().cloned(),
            )
            .await;
            if self.inner.runtime.start().await.is_ok() {
                self.emit_runtime_event(
                    GpuQueueRuntimeEventType::LlmRunning,
                    "Local LLM runtime is running after GPU lease expiration",
                    expired.first().cloned(),
                )
                .await;
            } else {
                self.emit_runtime_event(
                    GpuQueueRuntimeEventType::LlmStartFailed,
                    "Failed to restart local LLM runtime after GPU lease expiration",
                    expired.first().cloned(),
                )
                .await;
            }
        }
        self.inner.notify.notify_waiters();
    }

    async fn emit_runtime_event(
        &self,
        event_type: GpuQueueRuntimeEventType,
        message: impl Into<String>,
        lease: Option<LeaseInfo>,
    ) {
        let event_id = self.inner.event_sequence.fetch_add(1, Ordering::Relaxed);
        let event = GpuQueueRuntimeEvent {
            event_id: format!("gpu-{event_id}"),
            created_at: now_epoch_secs(),
            event_type,
            message: message.into(),
            llm_state: self.inner.runtime.state().await,
            lease,
        };
        let _ = self.inner.event_tx.send(event);
    }
}

pub fn router(daemon: GpuQueueDaemon) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/status", get(status))
        .route("/events", get(events))
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

async fn events(
    State(daemon): State<GpuQueueDaemon>,
) -> Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>> {
    let stream = BroadcastStream::new(daemon.subscribe_events()).filter_map(|event| match event {
        Ok(event) => Event::default()
            .event("gpuQueueRuntimeEvent")
            .json_data(event)
            .ok()
            .map(Ok),
        Err(_) => None,
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
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

fn lease_summary(lease: &LeaseInfo) -> String {
    format!("{} {} lease `{}`", lease.owner, lease.kind, lease.lease_id)
}
