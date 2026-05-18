use axum::body::Body;
use axum::body::Bytes;
use axum::http::StatusCode;
use axum::response::Response;
use serde_json::Value;

pub(super) struct BackendResponse {
    status: StatusCode,
    headers: Vec<(String, Vec<u8>)>,
    body: Bytes,
}

pub(super) async fn backend_response(
    response: reqwest::Response,
) -> anyhow::Result<BackendResponse> {
    let status = StatusCode::from_u16(response.status().as_u16())?;
    let headers = response
        .headers()
        .iter()
        .filter(|(name, _)| should_forward_header(name.as_str()))
        .map(|(name, value)| (name.as_str().to_string(), value.as_bytes().to_vec()))
        .collect();
    let bytes = response.bytes().await?;
    Ok(BackendResponse {
        status,
        headers,
        body: bytes,
    })
}

impl BackendResponse {
    pub(super) fn prompt_id(&self) -> Option<String> {
        let parsed = serde_json::from_slice::<Value>(&self.body).ok()?;
        parsed
            .get("prompt_id")
            .and_then(Value::as_str)
            .map(ToString::to_string)
    }

    pub(super) fn into_axum(self) -> anyhow::Result<Response> {
        let mut builder = Response::builder().status(self.status);
        for (name, value) in self.headers {
            builder = builder.header(name, value);
        }
        Ok(builder.body(Body::from(self.body))?)
    }

    pub(super) fn completed_history_for(&self, prompt_id: &str) -> bool {
        serde_json::from_slice::<Value>(&self.body)
            .ok()
            .and_then(|history| history.get(prompt_id).cloned())
            .is_some()
    }

    pub(super) fn body_bytes(&self) -> Bytes {
        self.body.clone()
    }
}

pub(super) fn should_forward_header(name: &str) -> bool {
    !matches!(
        name.to_ascii_lowercase().as_str(),
        "connection"
            | "content-length"
            | "host"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
    )
}
