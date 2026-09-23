//! Adapt product model selection to the Codex model catalog.

use super::super::IlhaeProfileConfig;
use super::super::IlhaeTomlConfig;
use super::super::native_runtime_effective_base_url;
use super::super::resolve_ilhae_profile_model_name;
use super::ILHAE_CODEX_MODEL_CATALOG_FILE;
use super::providers::model_provider_id_for_ilhae_profile;
use std::collections::BTreeMap;
use std::path::Path;
use std::path::PathBuf;

// Native runtimes and separately configured routers both own their model lists.
// In particular, a router without a model_path must not inherit the OpenAI catalog.
pub(super) fn profile_uses_native_catalog(
    profile_id: &str,
    profile: &IlhaeProfileConfig,
    user_config: &toml::Value,
) -> bool {
    if !native_runtime_effective_base_url(&profile.native_runtime).is_empty() {
        return true;
    }
    let provider_id = model_provider_id_for_ilhae_profile(profile_id, profile);
    let Some(provider) = user_config
        .get("model_providers")
        .and_then(|providers| providers.get(&provider_id))
    else {
        return false;
    };
    provider
        .get("base_url")
        .and_then(toml::Value::as_str)
        .is_some_and(|url| !url.trim().is_empty())
        && matches!(
            provider.get("requires_openai_auth"),
            None | Some(toml::Value::Boolean(false))
        )
}

pub(super) fn native_model_catalog(
    config: &IlhaeTomlConfig,
    user_config: &toml::Value,
) -> Option<codex_protocol::openai_models::ModelsResponse> {
    let mut models_by_slug = BTreeMap::new();
    for (profile_id, profile) in &config.profiles {
        if !profile_uses_native_catalog(profile_id, profile, user_config) {
            continue;
        }
        let slug = resolve_ilhae_profile_model_name(profile_id, profile);
        models_by_slug.entry(slug.clone()).or_insert_with(|| {
            let mut model = codex_models_manager::model_info::model_info_from_slug(&slug);
            model.visibility = codex_protocol::openai_models::ModelVisibility::List;
            model
        });
    }
    (!models_by_slug.is_empty()).then(|| codex_protocol::openai_models::ModelsResponse {
        models: models_by_slug.into_values().collect(),
    })
}

pub(super) fn serialize_native_model_catalog(
    catalog: &codex_protocol::openai_models::ModelsResponse,
) -> Result<Vec<u8>, String> {
    let mut bytes = serde_json::to_vec_pretty(catalog)
        .map_err(|error| format!("Failed to serialize native model catalog: {error}"))?;
    bytes.push(b'\n');
    Ok(bytes)
}

pub(super) fn transition_native_model_catalog(
    catalog_path: &Path,
    current: &codex_protocol::openai_models::ModelsResponse,
) -> codex_protocol::openai_models::ModelsResponse {
    let mut models_by_slug = std::fs::read(catalog_path)
        .ok()
        .and_then(|bytes| {
            serde_json::from_slice::<codex_protocol::openai_models::ModelsResponse>(&bytes).ok()
        })
        .unwrap_or_default()
        .models
        .into_iter()
        .map(|model| (model.slug.clone(), model))
        .collect::<BTreeMap<_, _>>();
    for model in &current.models {
        models_by_slug.insert(model.slug.clone(), model.clone());
    }
    codex_protocol::openai_models::ModelsResponse {
        models: models_by_slug.into_values().collect(),
    }
}

pub(super) fn absolute_native_model_catalog_path(codex_home: &Path) -> Result<PathBuf, String> {
    let path = codex_home.join(ILHAE_CODEX_MODEL_CATALOG_FILE);
    if path.is_absolute() {
        return Ok(path);
    }
    std::env::current_dir()
        .map(|current_dir| current_dir.join(path))
        .map_err(|error| format!("Failed to resolve Codex runtime directory: {error}"))
}
