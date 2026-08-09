use crate::config::IlhaeProfileNativeRuntimeConfig;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct EnsureNativeRuntimeRequest {
    pub profile_id: String,
    pub thinking_mode: String,
    pub config: IlhaeProfileNativeRuntimeConfig,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct EnsureNativeRuntimeResponse {
    pub profile_id: String,
    pub runtime_config_sha256: String,
    pub upstream_base_url: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct StopNativeRuntimeRequest {
    pub profile_id: String,
    pub config: IlhaeProfileNativeRuntimeConfig,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct StopNativeRuntimeResponse {
    pub profile_id: String,
}

#[cfg(test)]
#[path = "protocol_tests.rs"]
mod tests;
