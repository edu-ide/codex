use crate::config_api::apply_runtime_feature_enablement;
use codex_app_server_protocol::AskForApproval;
use codex_app_server_protocol::SandboxMode;
use codex_cloud_requirements::cloud_requirements_loader;
use codex_core::config::Config;
use codex_core::config::ConfigOverrides;
use codex_core::config_loader::CloudRequirementsLoader;
use codex_login::AuthManager;
use codex_login::default_client::set_default_client_residency_requirement;
use codex_protocol::protocol::SandboxPolicy;
use codex_state::ThreadMetadata;
use codex_utils_json_to_toml::json_to_toml;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::RwLock;
use toml::Value as TomlValue;
use tracing::warn;

pub(super) fn replace_cloud_requirements_loader(
    cloud_requirements: &RwLock<CloudRequirementsLoader>,
    auth_manager: Arc<AuthManager>,
    chatgpt_base_url: String,
    codex_home: PathBuf,
) {
    let loader = cloud_requirements_loader(auth_manager, chatgpt_base_url, codex_home);
    if let Ok(mut guard) = cloud_requirements.write() {
        *guard = loader;
    } else {
        warn!("failed to update cloud requirements loader");
    }
}

pub(super) async fn sync_default_client_residency_requirement(
    cli_overrides: &[(String, TomlValue)],
    cloud_requirements: &RwLock<CloudRequirementsLoader>,
) {
    let loader = cloud_requirements
        .read()
        .map(|guard| guard.clone())
        .unwrap_or_default();
    match codex_core::config::ConfigBuilder::default()
        .cli_overrides(cli_overrides.to_vec())
        .cloud_requirements(loader)
        .build()
        .await
    {
        Ok(config) => set_default_client_residency_requirement(config.enforce_residency.value()),
        Err(err) => warn!(
            error = %err,
            "failed to sync default client residency requirement after auth refresh"
        ),
    }
}

pub(super) async fn derive_config_from_params(
    cli_overrides: &[(String, TomlValue)],
    request_overrides: Option<HashMap<String, serde_json::Value>>,
    typesafe_overrides: ConfigOverrides,
    cloud_requirements: &CloudRequirementsLoader,
    codex_home: &Path,
    runtime_feature_enablement: &BTreeMap<String, bool>,
) -> std::io::Result<Config> {
    let merged_cli_overrides = cli_overrides
        .iter()
        .cloned()
        .chain(
            request_overrides
                .unwrap_or_default()
                .into_iter()
                .map(|(k, v)| (k, json_to_toml(v))),
        )
        .collect::<Vec<_>>();

    let mut config = codex_core::config::ConfigBuilder::default()
        .codex_home(codex_home.to_path_buf())
        .cli_overrides(merged_cli_overrides)
        .harness_overrides(typesafe_overrides)
        .cloud_requirements(cloud_requirements.clone())
        .build()
        .await?;
    apply_runtime_feature_enablement(&mut config, runtime_feature_enablement);
    Ok(config)
}

pub(super) async fn derive_config_for_cwd(
    cli_overrides: &[(String, TomlValue)],
    request_overrides: Option<HashMap<String, serde_json::Value>>,
    typesafe_overrides: ConfigOverrides,
    cwd: Option<PathBuf>,
    cloud_requirements: &CloudRequirementsLoader,
    codex_home: &Path,
    runtime_feature_enablement: &BTreeMap<String, bool>,
) -> std::io::Result<Config> {
    let merged_cli_overrides = cli_overrides
        .iter()
        .cloned()
        .chain(
            request_overrides
                .unwrap_or_default()
                .into_iter()
                .map(|(k, v)| (k, json_to_toml(v))),
        )
        .collect::<Vec<_>>();

    let mut config = codex_core::config::ConfigBuilder::default()
        .codex_home(codex_home.to_path_buf())
        .cli_overrides(merged_cli_overrides)
        .harness_overrides(typesafe_overrides)
        .fallback_cwd(cwd)
        .cloud_requirements(cloud_requirements.clone())
        .build()
        .await?;
    apply_runtime_feature_enablement(&mut config, runtime_feature_enablement);
    Ok(config)
}

pub(super) fn preserve_runtime_model_provider_base_url(
    runtime_model_provider_id: &str,
    runtime_base_url: Option<&str>,
    derived_config: &mut Config,
) {
    if derived_config.model_provider_id != runtime_model_provider_id {
        return;
    }
    let Some(runtime_base_url) = runtime_base_url else {
        return;
    };

    let runtime_base_url = runtime_base_url.to_string();
    derived_config.model_provider.base_url = Some(runtime_base_url.clone());
    if let Some(provider) = derived_config
        .model_providers
        .get_mut(runtime_model_provider_id)
    {
        provider.base_url = Some(runtime_base_url);
    }
}

pub(super) fn collect_resume_override_mismatches(
    request: &codex_app_server_protocol::ThreadResumeParams,
    config_snapshot: &codex_core::ThreadConfigSnapshot,
) -> Vec<String> {
    let mut mismatch_details = Vec::new();

    if let Some(requested_model) = request.model.as_deref()
        && requested_model != config_snapshot.model
    {
        mismatch_details.push(format!(
            "model requested={requested_model} active={}",
            config_snapshot.model
        ));
    }
    if let Some(requested_provider) = request.model_provider.as_deref()
        && requested_provider != config_snapshot.model_provider_id
    {
        mismatch_details.push(format!(
            "model_provider requested={requested_provider} active={}",
            config_snapshot.model_provider_id
        ));
    }
    if let Some(requested_service_tier) = request.service_tier.as_ref()
        && requested_service_tier != &config_snapshot.service_tier
    {
        mismatch_details.push(format!(
            "service_tier requested={requested_service_tier:?} active={:?}",
            config_snapshot.service_tier
        ));
    }
    if let Some(requested_cwd) = request.cwd.as_deref() {
        let requested_cwd_path = std::path::PathBuf::from(requested_cwd);
        if requested_cwd_path != config_snapshot.cwd.as_path() {
            mismatch_details.push(format!(
                "cwd requested={} active={}",
                requested_cwd_path.display(),
                config_snapshot.cwd.display()
            ));
        }
    }
    if let Some(requested_approval) = request.approval_policy.as_ref() {
        let active_approval: AskForApproval = config_snapshot.approval_policy.into();
        if requested_approval != &active_approval {
            mismatch_details.push(format!(
                "approval_policy requested={requested_approval:?} active={active_approval:?}"
            ));
        }
    }
    if let Some(requested_review_policy) = request.approvals_reviewer.as_ref() {
        let active_review_policy: codex_app_server_protocol::ApprovalsReviewer =
            config_snapshot.approvals_reviewer.into();
        if requested_review_policy != &active_review_policy {
            mismatch_details.push(format!(
                "approvals_reviewer requested={requested_review_policy:?} active={active_review_policy:?}"
            ));
        }
    }
    if let Some(requested_sandbox) = request.sandbox.as_ref() {
        let sandbox_matches = matches!(
            (requested_sandbox, &config_snapshot.sandbox_policy),
            (SandboxMode::ReadOnly, SandboxPolicy::ReadOnly { .. })
                | (
                    SandboxMode::WorkspaceWrite,
                    SandboxPolicy::WorkspaceWrite { .. }
                )
                | (
                    SandboxMode::DangerFullAccess,
                    SandboxPolicy::DangerFullAccess
                )
                | (
                    SandboxMode::DangerFullAccess,
                    SandboxPolicy::ExternalSandbox { .. }
                )
        );
        if !sandbox_matches {
            mismatch_details.push(format!(
                "sandbox requested={requested_sandbox:?} active={:?}",
                config_snapshot.sandbox_policy
            ));
        }
    }
    if let Some(requested_personality) = request.personality.as_ref()
        && config_snapshot.personality.as_ref() != Some(requested_personality)
    {
        mismatch_details.push(format!(
            "personality requested={requested_personality:?} active={:?}",
            config_snapshot.personality
        ));
    }

    if request.config.is_some() {
        mismatch_details
            .push("config overrides were provided and ignored while running".to_string());
    }
    if request.base_instructions.is_some() {
        mismatch_details
            .push("baseInstructions override was provided and ignored while running".to_string());
    }
    if request.developer_instructions.is_some() {
        mismatch_details.push(
            "developerInstructions override was provided and ignored while running".to_string(),
        );
    }
    if request.persist_extended_history {
        mismatch_details.push(
            "persistExtendedHistory override was provided and ignored while running".to_string(),
        );
    }

    mismatch_details
}

pub(super) fn merge_persisted_resume_metadata(
    request_overrides: &mut Option<HashMap<String, serde_json::Value>>,
    typesafe_overrides: &mut ConfigOverrides,
    persisted_metadata: &ThreadMetadata,
) {
    if has_model_resume_override(request_overrides.as_ref(), typesafe_overrides) {
        return;
    }

    typesafe_overrides.model = persisted_metadata.model.clone();

    if let Some(reasoning_effort) = persisted_metadata.reasoning_effort {
        request_overrides.get_or_insert_with(HashMap::new).insert(
            "model_reasoning_effort".to_string(),
            serde_json::Value::String(reasoning_effort.to_string()),
        );
    }
}

fn has_model_resume_override(
    request_overrides: Option<&HashMap<String, serde_json::Value>>,
    typesafe_overrides: &ConfigOverrides,
) -> bool {
    typesafe_overrides.model.is_some()
        || typesafe_overrides.model_provider.is_some()
        || request_overrides.is_some_and(|overrides| overrides.contains_key("model"))
        || request_overrides
            .is_some_and(|overrides| overrides.contains_key("model_reasoning_effort"))
}
