use super::Session;
use super::TurnContext;
use crate::apps::render_apps_section;
use crate::commit_attribution::commit_message_trailer_instruction;
use crate::connectors;
use crate::environment_context::EnvironmentContext;
use crate::instructions::UserInstructions;
use crate::memories::prompts::build_memory_tool_developer_instructions;
use crate::plugins::render_plugins_section;
use crate::render_skills_section;
use codex_features::Feature;
use codex_protocol::models::DeveloperInstructions;
use codex_protocol::models::ResponseItem;

impl Session {
    pub(crate) async fn build_initial_context(
        &self,
        turn_context: &TurnContext,
    ) -> Vec<ResponseItem> {
        let mut developer_sections = Vec::<String>::with_capacity(8);
        let mut contextual_user_sections = Vec::<String>::with_capacity(2);
        let shell = self.user_shell();
        let (
            reference_context_item,
            previous_turn_settings,
            collaboration_mode,
            base_instructions,
            session_source,
        ) = {
            let state = self.state.lock().await;
            (
                state.reference_context_item(),
                state.previous_turn_settings(),
                state.session_configuration.collaboration_mode.clone(),
                state.session_configuration.base_instructions.clone(),
                state.session_configuration.session_source.clone(),
            )
        };
        if let Some(model_switch_message) =
            crate::context_manager::updates::build_model_instructions_update_item(
                previous_turn_settings.as_ref(),
                turn_context,
            )
        {
            developer_sections.push(model_switch_message.into_text());
        }
        if turn_context.config.include_permissions_instructions {
            developer_sections.push(
                DeveloperInstructions::from_policy(
                    turn_context.sandbox_policy.get(),
                    turn_context.approval_policy.value(),
                    turn_context.config.approvals_reviewer,
                    self.services.exec_policy.current().as_ref(),
                    &turn_context.cwd,
                    turn_context
                        .features
                        .enabled(Feature::ExecPermissionApprovals),
                    turn_context
                        .features
                        .enabled(Feature::RequestPermissionsTool),
                )
                .into_text(),
            );
        }
        let separate_guardian_developer_message =
            crate::guardian::is_guardian_reviewer_source(&session_source);
        if !separate_guardian_developer_message
            && let Some(developer_instructions) = turn_context.developer_instructions.as_deref()
            && !developer_instructions.is_empty()
        {
            developer_sections.push(developer_instructions.to_string());
        }
        if turn_context.features.enabled(Feature::MemoryTool)
            && turn_context.config.memories.use_memories
            && let Some(memory_prompt) =
                build_memory_tool_developer_instructions(&turn_context.config.codex_home).await
        {
            developer_sections.push(memory_prompt);
        }
        if let Some(collab_instructions) =
            DeveloperInstructions::from_collaboration_mode(&collaboration_mode)
        {
            developer_sections.push(collab_instructions.into_text());
        }
        if let Some(realtime_update) = crate::context_manager::updates::build_initial_realtime_item(
            reference_context_item.as_ref(),
            previous_turn_settings.as_ref(),
            turn_context,
        ) {
            developer_sections.push(realtime_update.into_text());
        }
        if self.features.enabled(Feature::Personality)
            && let Some(personality) = turn_context.personality
        {
            let model_info = turn_context.model_info.clone();
            let has_baked_personality = model_info.supports_personality()
                && base_instructions == model_info.get_model_instructions(Some(personality));
            if !has_baked_personality
                && let Some(personality_message) =
                    crate::context_manager::updates::personality_message_for(
                        &model_info,
                        personality,
                    )
            {
                developer_sections.push(
                    DeveloperInstructions::personality_spec_message(personality_message)
                        .into_text(),
                );
            }
        }
        if turn_context.config.include_apps_instructions && turn_context.apps_enabled() {
            let mcp_connection_manager = self.services.mcp_connection_manager.read().await;
            let accessible_and_enabled_connectors =
                connectors::list_accessible_and_enabled_connectors_from_manager(
                    &mcp_connection_manager,
                    &turn_context.config,
                )
                .await;
            if let Some(apps_section) = render_apps_section(&accessible_and_enabled_connectors) {
                developer_sections.push(apps_section);
            }
        }
        let implicit_skills = turn_context
            .turn_skills
            .outcome
            .allowed_skills_for_implicit_invocation();
        if let Some(skills_section) = render_skills_section(&implicit_skills) {
            developer_sections.push(skills_section);
        }
        let loaded_plugins = self
            .services
            .plugins_manager
            .plugins_for_config(&turn_context.config)
            .await;
        if let Some(plugin_section) = render_plugins_section(loaded_plugins.capability_summaries())
        {
            developer_sections.push(plugin_section);
        }
        if turn_context.features.enabled(Feature::CodexGitCommit)
            && let Some(commit_message_instruction) = commit_message_trailer_instruction(
                turn_context.config.commit_attribution.as_deref(),
            )
        {
            developer_sections.push(commit_message_instruction);
        }
        if let Some(user_instructions) = turn_context.user_instructions.as_deref() {
            contextual_user_sections.push(
                UserInstructions {
                    text: user_instructions.to_string(),
                    directory: turn_context.cwd.to_string_lossy().into_owned(),
                }
                .serialize_to_text(),
            );
        }
        if turn_context.config.include_environment_context {
            let subagents = self
                .services
                .agent_control
                .format_environment_context_subagents(self.conversation_id)
                .await;
            contextual_user_sections.push(
                EnvironmentContext::from_turn_context(turn_context, shell.as_ref())
                    .with_subagents(subagents)
                    .serialize_to_xml(),
            );
        }

        let mut items = Vec::with_capacity(3);
        if let Some(developer_message) =
            crate::context_manager::updates::build_developer_update_item(developer_sections)
        {
            items.push(developer_message);
        }
        if let Some(contextual_user_message) =
            crate::context_manager::updates::build_contextual_user_message(contextual_user_sections)
        {
            items.push(contextual_user_message);
        }
        if separate_guardian_developer_message
            && let Some(developer_instructions) = turn_context.developer_instructions.as_deref()
            && !developer_instructions.is_empty()
            && let Some(guardian_developer_message) =
                crate::context_manager::updates::build_developer_update_item(vec![
                    developer_instructions.to_string(),
                ])
        {
            items.push(guardian_developer_message);
        }
        items
    }
}
