use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;

use super::Session;
use super::TurnContext;
use crate::SkillInjections;
use crate::SkillLoadOutcome;
use crate::build_skill_injections;
use crate::collect_env_var_dependencies;
use crate::collect_explicit_skill_mentions;
use crate::mcp_skill_dependencies::maybe_prompt_and_install_mcp_dependencies;
use crate::mentions::build_connector_slug_counts;
use crate::mentions::build_skill_name_counts;
use crate::mentions::collect_explicit_app_ids;
use crate::mentions::collect_explicit_plugin_mentions;
use crate::plugins::AppConnectorId;
use crate::plugins::PluginCapabilitySummary;
use crate::plugins::PluginTelemetryMetadata;
use crate::plugins::build_plugin_injections;
use crate::resolve_skill_dependencies_for_turn;
use codex_analytics::AppInvocation;
use codex_analytics::TrackEventsContext;
use codex_analytics::build_track_events_context;
use codex_features::Feature;
use codex_mcp::ToolInfo;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::WarningEvent;
use codex_protocol::user_input::UserInput;
use tokio_util::sync::CancellationToken;

pub(super) struct TurnPreparation {
    pub tracking: TrackEventsContext,
    pub explicitly_enabled_connectors: HashSet<String>,
    pub skill_items: Vec<ResponseItem>,
    pub plugin_items: Vec<ResponseItem>,
    pub mentioned_plugin_metadata: Vec<PluginTelemetryMetadata>,
    pub mentioned_app_invocations: Vec<AppInvocation>,
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn prepare_turn_mentions_and_injections(
    sess: &Arc<Session>,
    turn_context: &Arc<TurnContext>,
    input: &[UserInput],
    cancellation_token: &CancellationToken,
    skills_outcome: Option<&SkillLoadOutcome>,
    mcp_tools: &HashMap<String, ToolInfo>,
    plugin_capability_summaries: &[PluginCapabilitySummary],
    effective_apps: Vec<AppConnectorId>,
) -> TurnPreparation {
    let mentioned_plugins = collect_explicit_plugin_mentions(input, plugin_capability_summaries);
    let available_connectors = if turn_context.apps_enabled() {
        let connectors = crate::connectors::merge_plugin_apps_with_accessible(
            effective_apps,
            crate::connectors::accessible_connectors_from_mcp_tools(mcp_tools),
        );
        crate::connectors::with_app_enabled_state(connectors, &turn_context.config)
    } else {
        Vec::new()
    };
    let connector_slug_counts = build_connector_slug_counts(&available_connectors);
    let skill_name_counts_lower = skills_outcome.as_ref().map_or_else(HashMap::new, |outcome| {
        build_skill_name_counts(&outcome.skills, &outcome.disabled_paths).1
    });
    let mentioned_skills = skills_outcome.as_ref().map_or_else(Vec::new, |outcome| {
        collect_explicit_skill_mentions(
            input,
            &outcome.skills,
            &outcome.disabled_paths,
            &connector_slug_counts,
        )
    });
    if turn_context
        .config
        .features
        .enabled(Feature::SkillEnvVarDependencyPrompt)
    {
        let env_var_dependencies = collect_env_var_dependencies(&mentioned_skills);
        resolve_skill_dependencies_for_turn(sess, turn_context, &env_var_dependencies).await;
    }

    maybe_prompt_and_install_mcp_dependencies(
        sess.as_ref(),
        turn_context.as_ref(),
        cancellation_token,
        &mentioned_skills,
    )
    .await;

    let tracking = build_track_events_context(
        turn_context.model_info.slug.clone(),
        sess.conversation_id.to_string(),
        turn_context.sub_id.clone(),
    );
    let session_telemetry = turn_context.session_telemetry.clone();
    let SkillInjections {
        items: skill_items,
        warnings: skill_warnings,
    } = build_skill_injections(
        &mentioned_skills,
        skills_outcome,
        Some(&session_telemetry),
        &sess.services.analytics_events_client,
        tracking.clone(),
    )
    .await;
    for message in skill_warnings {
        sess.send_event(turn_context, EventMsg::Warning(WarningEvent { message }))
            .await;
    }

    let plugin_items = build_plugin_injections(&mentioned_plugins, mcp_tools, &available_connectors);
    let mentioned_plugin_metadata = mentioned_plugins
        .iter()
        .filter_map(crate::plugins::PluginCapabilitySummary::telemetry_metadata)
        .collect::<Vec<_>>();

    let mut explicitly_enabled_connectors = collect_explicit_app_ids(input);
    explicitly_enabled_connectors.extend(super::collect_explicit_app_ids_from_skill_items(
        &skill_items,
        &available_connectors,
        &skill_name_counts_lower,
    ));
    let connector_names_by_id = available_connectors
        .iter()
        .map(|connector| (connector.id.as_str(), connector.name.as_str()))
        .collect::<HashMap<&str, &str>>();
    let mentioned_app_invocations = explicitly_enabled_connectors
        .iter()
        .map(|connector_id| AppInvocation {
            connector_id: Some(connector_id.clone()),
            app_name: connector_names_by_id
                .get(connector_id.as_str())
                .map(|name| (*name).to_string()),
            invocation_type: Some(codex_analytics::InvocationType::Explicit),
        })
        .collect::<Vec<_>>();

    TurnPreparation {
        tracking,
        explicitly_enabled_connectors,
        skill_items,
        plugin_items,
        mentioned_plugin_metadata,
        mentioned_app_invocations,
    }
}
