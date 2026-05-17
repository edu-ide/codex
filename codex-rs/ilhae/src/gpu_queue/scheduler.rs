use std::collections::VecDeque;
use std::error::Error;
use std::fmt;

use super::api::LeaseInfo;
use super::api::LeaseMode;
use super::api::LeaseRequest;
use super::api::LeaseState;
use super::api::StatusResponse;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeaseSchedulerError {
    InvalidRequest(String),
    LeaseNotFound(String),
}

impl fmt::Display for LeaseSchedulerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LeaseSchedulerError::InvalidRequest(message) => write!(f, "{message}"),
            LeaseSchedulerError::LeaseNotFound(lease_id) => {
                write!(f, "GPU lease `{lease_id}` was not found")
            }
        }
    }
}

impl Error for LeaseSchedulerError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseOutcome {
    pub released: LeaseInfo,
    pub promoted: Option<LeaseInfo>,
}

#[derive(Debug, Default)]
pub struct LeaseScheduler {
    active_lease: Option<LeaseInfo>,
    pending_leases: VecDeque<LeaseInfo>,
    next_id: u64,
}

impl LeaseScheduler {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn request_lease(
        &mut self,
        request: LeaseRequest,
        llm_was_preempted: bool,
        now: u64,
    ) -> Result<LeaseInfo, LeaseSchedulerError> {
        validate_request(&request)?;

        let state = if self.active_lease.is_some() {
            LeaseState::Pending
        } else {
            LeaseState::Granted
        };
        let lease_id = self.next_lease_id();
        let lease = LeaseInfo {
            lease_id,
            owner: request.owner,
            kind: request.kind,
            mode: request.mode,
            state,
            preempt_llm: request.preempt_llm,
            llm_was_preempted,
            ttl_seconds: request.ttl_seconds,
            queued_at: now,
            granted_at: (state == LeaseState::Granted).then_some(now),
            expires_at: (state == LeaseState::Granted).then_some(now + request.ttl_seconds),
        };

        match state {
            LeaseState::Granted => self.active_lease = Some(lease.clone()),
            LeaseState::Pending => self.pending_leases.push_back(lease.clone()),
        }

        Ok(lease)
    }

    pub fn release_lease(
        &mut self,
        lease_id: &str,
        now: u64,
    ) -> Result<ReleaseOutcome, LeaseSchedulerError> {
        if self
            .active_lease
            .as_ref()
            .is_some_and(|lease| lease.lease_id == lease_id)
        {
            let released = self.active_lease.take().expect("active lease checked");
            let promoted = self.promote_next(now);
            return Ok(ReleaseOutcome { released, promoted });
        }

        if let Some(index) = self
            .pending_leases
            .iter()
            .position(|lease| lease.lease_id == lease_id)
        {
            let released = self
                .pending_leases
                .remove(index)
                .expect("pending lease index checked");
            return Ok(ReleaseOutcome {
                released,
                promoted: None,
            });
        }

        Err(LeaseSchedulerError::LeaseNotFound(lease_id.to_string()))
    }

    pub fn expire_leases(&mut self, now: u64) -> Vec<LeaseInfo> {
        let Some(active) = self.active_lease.as_ref() else {
            return Vec::new();
        };
        if !active
            .expires_at
            .is_some_and(|expires_at| expires_at <= now)
        {
            return Vec::new();
        }

        let expired = self.active_lease.take().expect("active lease checked");
        self.promote_next(now);
        vec![expired]
    }

    pub fn heartbeat_lease(
        &mut self,
        lease_id: &str,
        now: u64,
    ) -> Result<LeaseInfo, LeaseSchedulerError> {
        if let Some(active) = self.active_lease.as_mut()
            && active.lease_id == lease_id
        {
            active.expires_at = Some(now + active.ttl_seconds);
            return Ok(active.clone());
        }

        if let Some(pending) = self
            .pending_leases
            .iter()
            .find(|lease| lease.lease_id == lease_id)
        {
            return Ok(pending.clone());
        }

        Err(LeaseSchedulerError::LeaseNotFound(lease_id.to_string()))
    }

    pub fn mark_llm_was_preempted(
        &mut self,
        lease_id: &str,
        llm_was_preempted: bool,
    ) -> Result<LeaseInfo, LeaseSchedulerError> {
        if let Some(active) = self.active_lease.as_mut()
            && active.lease_id == lease_id
        {
            active.llm_was_preempted = llm_was_preempted;
            return Ok(active.clone());
        }

        if let Some(pending) = self
            .pending_leases
            .iter_mut()
            .find(|lease| lease.lease_id == lease_id)
        {
            pending.llm_was_preempted = llm_was_preempted;
            return Ok(pending.clone());
        }

        Err(LeaseSchedulerError::LeaseNotFound(lease_id.to_string()))
    }

    pub fn lease(&self, lease_id: &str) -> Option<LeaseInfo> {
        self.active_lease
            .iter()
            .chain(self.pending_leases.iter())
            .find(|lease| lease.lease_id == lease_id)
            .cloned()
    }

    pub fn status(&self, _now: u64) -> StatusResponse {
        StatusResponse {
            uptime_seconds: 0,
            llm_state: super::api::LlmRuntimeState::Unknown,
            active_lease: self.active_lease.clone(),
            pending_leases: self.pending_leases.iter().cloned().collect(),
        }
    }

    fn next_lease_id(&mut self) -> String {
        self.next_id += 1;
        format!("gpu-lease-{}", self.next_id)
    }

    fn promote_next(&mut self, now: u64) -> Option<LeaseInfo> {
        let mut next = self.pending_leases.pop_front()?;
        next.state = LeaseState::Granted;
        next.granted_at = Some(now);
        next.expires_at = Some(now + next.ttl_seconds);
        self.active_lease = Some(next.clone());
        Some(next)
    }
}

fn validate_request(request: &LeaseRequest) -> Result<(), LeaseSchedulerError> {
    if request.owner.trim().is_empty() {
        return Err(LeaseSchedulerError::InvalidRequest(
            "lease owner must not be empty".to_string(),
        ));
    }
    if request.kind.trim().is_empty() {
        return Err(LeaseSchedulerError::InvalidRequest(
            "lease kind must not be empty".to_string(),
        ));
    }
    if request.mode == LeaseMode::Exclusive && request.ttl_seconds == 0 {
        return Err(LeaseSchedulerError::InvalidRequest(
            "exclusive GPU leases require ttlSeconds > 0".to_string(),
        ));
    }
    Ok(())
}
