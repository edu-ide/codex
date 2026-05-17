use anyhow::Context;
use serde::Serialize;
use serde::de::DeserializeOwned;

use super::api::ErrorResponse;
use super::api::LeaseRequest;
use super::api::LeaseResponse;
use super::api::LlmCommandResponse;
use super::api::ReleaseLeaseResponse;
use super::api::StatusResponse;

#[derive(Clone)]
pub struct GpuQueueClient {
    base_url: String,
    http: reqwest::Client,
}

impl GpuQueueClient {
    pub fn from_addr(addr: &str) -> Self {
        let base_url = if addr.starts_with("http://") || addr.starts_with("https://") {
            addr.trim_end_matches('/').to_string()
        } else {
            format!("http://{}", addr.trim_end_matches('/'))
        };
        Self {
            base_url,
            http: reqwest::Client::new(),
        }
    }

    pub async fn status(&self) -> anyhow::Result<StatusResponse> {
        self.get("/status").await
    }

    pub async fn acquire_lease(&self, request: &LeaseRequest) -> anyhow::Result<LeaseResponse> {
        self.post_json("/leases", request).await
    }

    pub async fn heartbeat_lease(&self, lease_id: &str) -> anyhow::Result<LeaseResponse> {
        self.post_empty(&format!("/leases/{lease_id}/heartbeat"))
            .await
    }

    pub async fn release_lease(&self, lease_id: &str) -> anyhow::Result<ReleaseLeaseResponse> {
        self.post_empty(&format!("/leases/{lease_id}/release"))
            .await
    }

    pub async fn llm_start(&self) -> anyhow::Result<LlmCommandResponse> {
        self.post_empty("/llm/start").await
    }

    pub async fn llm_stop(&self) -> anyhow::Result<LlmCommandResponse> {
        self.post_empty("/llm/stop").await
    }

    pub async fn llm_restart(&self) -> anyhow::Result<LlmCommandResponse> {
        self.post_empty("/llm/restart").await
    }

    async fn get<T>(&self, path: &str) -> anyhow::Result<T>
    where
        T: DeserializeOwned,
    {
        let response = self
            .http
            .get(self.url(path))
            .send()
            .await
            .with_context(|| format!("failed to call GPU queue API GET {path}"))?;
        parse_response(response).await
    }

    async fn post_empty<T>(&self, path: &str) -> anyhow::Result<T>
    where
        T: DeserializeOwned,
    {
        let response = self
            .http
            .post(self.url(path))
            .send()
            .await
            .with_context(|| format!("failed to call GPU queue API POST {path}"))?;
        parse_response(response).await
    }

    async fn post_json<T, B>(&self, path: &str, body: &B) -> anyhow::Result<T>
    where
        T: DeserializeOwned,
        B: Serialize + ?Sized,
    {
        let response = self
            .http
            .post(self.url(path))
            .json(body)
            .send()
            .await
            .with_context(|| format!("failed to call GPU queue API POST {path}"))?;
        parse_response(response).await
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }
}

async fn parse_response<T>(response: reqwest::Response) -> anyhow::Result<T>
where
    T: DeserializeOwned,
{
    let status = response.status();
    let text = response.text().await?;
    if status.is_success() {
        return serde_json::from_str(&text)
            .with_context(|| format!("failed to parse GPU queue API response body: {text}"));
    }

    if let Ok(error) = serde_json::from_str::<ErrorResponse>(&text) {
        anyhow::bail!("GPU queue API returned {status}: {}", error.error);
    }

    anyhow::bail!("GPU queue API returned {status}: {text}");
}
