//! Translate Ilhae profiles into Codex provider configuration.

use super::super::IlhaeProfileConfig;
use super::super::IlhaeProfileNativeRuntimeConfig;
use super::super::native_runtime_effective_base_url;
use super::super::native_runtime_model_context_window;
use super::super::profile_engine_id;
use super::super::resolve_ilhae_profile_model_name;
use super::catalog::profile_uses_native_catalog;
use std::collections::BTreeMap;
use std::path::Path;

pub(super) fn string_map_as_toml_value(values: &BTreeMap<String, String>) -> toml::Value {
    let mut table = toml::value::Table::new();
    for (key, value) in values {
        table.insert(key.trim().to_string(), toml::Value::String(value.clone()));
    }
    toml::Value::Table(table)
}

pub(super) fn native_runtime_effective_query_params(
    runtime: &IlhaeProfileNativeRuntimeConfig,
) -> BTreeMap<String, String> {
    let mut query_params = BTreeMap::new();
    if let Some(overrides) = runtime.query_params.as_ref() {
        for (key, value) in overrides {
            let key = key.trim();
            if !key.is_empty() {
                query_params.insert(key.to_string(), value.clone());
            }
        }
    }
    query_params
}

pub(super) fn native_runtime_for_profile(
    profile: &IlhaeProfileConfig,
) -> Option<&IlhaeProfileNativeRuntimeConfig> {
    Some(&profile.native_runtime)
}

pub(super) fn native_model_provider_id_for_profile(profile_id: &str) -> String {
    format!("ilhae-native-{profile_id}")
}

pub(super) fn native_model_provider_table(
    runtime: &IlhaeProfileNativeRuntimeConfig,
) -> toml::value::Table {
    let mut table = toml::value::Table::new();
    let base_url = native_runtime_effective_base_url(runtime);
    table.insert(
        "name".to_string(),
        toml::Value::String(
            runtime
                .provider
                .as_deref()
                .map(str::trim)
                .filter(|provider| !provider.is_empty())
                .unwrap_or("llama-server")
                .to_string(),
        ),
    );
    table.insert("base_url".to_string(), toml::Value::String(base_url));
    table.insert(
        "wire_api".to_string(),
        toml::Value::String("responses".to_string()),
    );
    table.insert(
        "requires_openai_auth".to_string(),
        toml::Value::Boolean(false),
    );
    let query_params = native_runtime_effective_query_params(runtime);
    if !query_params.is_empty() {
        table.insert(
            "query_params".to_string(),
            string_map_as_toml_value(&query_params),
        );
    }
    let mut http_headers = runtime.http_headers.clone().unwrap_or_default();
    let proxy_token = runtime
        .proxy_token
        .as_deref()
        .map(str::trim)
        .filter(|proxy_token| !proxy_token.is_empty());
    if let Some(proxy_token) = proxy_token {
        http_headers.insert("X-Ilhae-Runtime-Token".to_string(), proxy_token.to_string());
    }
    if !http_headers.is_empty() {
        table.insert(
            "http_headers".to_string(),
            string_map_as_toml_value(&http_headers),
        );
    }
    let mut env_http_headers = runtime.env_http_headers.clone().unwrap_or_default();
    if proxy_token.is_some() {
        env_http_headers.retain(|name, _| !name.eq_ignore_ascii_case("X-Ilhae-Runtime-Token"));
    }
    if !env_http_headers.is_empty() {
        table.insert(
            "env_http_headers".to_string(),
            string_map_as_toml_value(&env_http_headers),
        );
    }
    if let Some(request_max_retries) = runtime.request_max_retries {
        if let Ok(request_max_retries) = i64::try_from(request_max_retries) {
            table.insert(
                "request_max_retries".to_string(),
                toml::Value::Integer(request_max_retries),
            );
        }
    }
    if let Some(stream_max_retries) = runtime.stream_max_retries {
        if let Ok(stream_max_retries) = i64::try_from(stream_max_retries) {
            table.insert(
                "stream_max_retries".to_string(),
                toml::Value::Integer(stream_max_retries),
            );
        }
    }
    table
}

pub(super) fn insert_native_model_provider(
    model_providers: &mut toml::value::Table,
    profile_id: &str,
    profile: &IlhaeProfileConfig,
) {
    let Some(runtime) = native_runtime_for_profile(profile) else {
        return;
    };
    if native_runtime_effective_base_url(runtime).is_empty() {
        return;
    }
    model_providers.insert(
        native_model_provider_id_for_profile(profile_id),
        toml::Value::Table(native_model_provider_table(runtime)),
    );
}

pub(super) fn model_provider_id_for_ilhae_profile(
    profile_id: &str,
    profile: &IlhaeProfileConfig,
) -> String {
    let native = native_runtime_for_profile(profile);
    let model_provider = native
        .filter(|runtime| !native_runtime_effective_base_url(runtime).is_empty())
        .map(|_| native_model_provider_id_for_profile(profile_id))
        .or_else(|| {
            native
                .and_then(|runtime| runtime.provider.clone())
                .filter(|provider| !provider.trim().is_empty())
        })
        .unwrap_or_else(|| profile_engine_id(profile));

    // Engine aliases use the configured llama-server provider in both the
    // runtime projection and its local model catalog.
    if model_provider == "ilhae" || model_provider == "codex" {
        "llama-server".to_string()
    } else {
        model_provider
    }
}

pub(super) fn codex_profile_table_for_ilhae_profile(
    profile_id: &str,
    profile: &IlhaeProfileConfig,
    user_config: &toml::Value,
    model_catalog_path: &Path,
) -> toml::value::Table {
    let native = native_runtime_for_profile(profile);
    let mut table = toml::value::Table::new();
    let model_name = resolve_ilhae_profile_model_name(profile_id, profile);
    let model_context_window = native
        .map(native_runtime_model_context_window)
        .unwrap_or(32_768);
    let model_provider = model_provider_id_for_ilhae_profile(profile_id, profile);

    table.insert("model".to_string(), toml::Value::String(model_name));
    table.insert(
        "model_context_window".to_string(),
        toml::Value::Integer(model_context_window as i64),
    );
    table.insert(
        "model_provider".to_string(),
        toml::Value::String(model_provider),
    );
    if profile_uses_native_catalog(profile_id, profile, user_config) {
        table.insert(
            "model_catalog_json".to_string(),
            toml::Value::String(model_catalog_path.display().to_string()),
        );
    }

    table
}
