use crate::app_server_session::AppServerSession;
use codex_ilhae::BootstrappedIlhaeRuntime;
use codex_ilhae::native_runtime_context;
use codex_ilhae::notify_engine_state;
use codex_ilhae::types::NOTIF_APP_SESSION_EVENT;
use color_eyre::eyre::Result;

use super::App;

impl App {
    pub(super) fn sync_backend_capabilities(&mut self) {}

    fn report_ilhae_runtime_control_unsupported(&mut self, command: &str) {
        let backend = if self.remote_app_server_url.is_some() {
            "remote runtime"
        } else {
            "current backend"
        };
        self.chat_widget.add_error_message(format!(
            "`/{command}` is only supported in the native embedded ilhae runtime; the {backend} cannot mutate runtime state."
        ));
    }

    fn native_ilhae_runtime_for_control(
        &mut self,
        command: &str,
    ) -> Option<BootstrappedIlhaeRuntime> {
        let runtime = native_runtime_context();
        if runtime.is_none() {
            self.report_ilhae_runtime_control_unsupported(command);
        }
        runtime
    }

    async fn finish_ilhae_runtime_mutation(&mut self, runtime: &BootstrappedIlhaeRuntime) {
        notify_engine_state(&runtime.cx_cache, &runtime.settings_store).await;
        self.sync_backend_capabilities();
        self.refresh_status_line();
    }

    async fn mutate_native_ilhae_runtime<F>(&mut self, command: &'static str, mutator: F)
    where
        F: FnOnce(&BootstrappedIlhaeRuntime) -> Result<(), String>,
    {
        let Some(runtime) = self.native_ilhae_runtime_for_control(command) else {
            return;
        };

        if let Err(err) = mutator(&runtime) {
            self.chat_widget
                .add_error_message(format!("Failed to apply `/{command}`: {err}"));
            return;
        }

        self.finish_ilhae_runtime_mutation(&runtime).await;
    }

    async fn mutate_native_ilhae_active_profile<F>(&mut self, command: &'static str, mutator: F)
    where
        F: FnOnce(&mut codex_ilhae::IlhaeAppProfileDto) -> Result<(), String>,
    {
        let Some(runtime) = self.native_ilhae_runtime_for_control(command) else {
            return;
        };

        let (active_profile_id, profile) = codex_ilhae::config::get_ilhae_profile(None);
        let mut profile = profile.unwrap_or_else(|| codex_ilhae::IlhaeAppProfileDto {
            id: active_profile_id.unwrap_or_else(|| "default".to_string()),
            ..Default::default()
        });

        if let Err(err) = mutator(&mut profile) {
            self.chat_widget
                .add_error_message(format!("Failed to prepare `/{command}`: {err}"));
            return;
        }

        let persisted = match codex_ilhae::config::upsert_ilhae_profile(profile, true) {
            Ok((_, profile)) => profile,
            Err(err) => {
                self.chat_widget
                    .add_error_message(format!("Failed to apply `/{command}`: {err}"));
                return;
            }
        };

        if let Err(err) =
            codex_ilhae::config::apply_ilhae_profile_projection(&runtime.settings_store, &persisted)
        {
            self.chat_widget
                .add_error_message(format!("Failed to project `/{command}`: {err}"));
            return;
        }

        self.finish_ilhae_runtime_mutation(&runtime).await;
    }

    pub(super) async fn set_ilhae_runtime_profile(&mut self, profile_id: String) {
        self.mutate_native_ilhae_runtime("profile", move |runtime| {
            let profile = codex_ilhae::config::set_active_ilhae_profile(&profile_id)?;
            codex_ilhae::config::apply_ilhae_profile_projection(&runtime.settings_store, &profile)
        })
        .await;
    }

    pub(super) async fn set_ilhae_advisor_mode(
        &mut self,
        enabled: Option<bool>,
        preset: Option<String>,
    ) {
        self.mutate_native_ilhae_active_profile("advisor", move |profile| {
            let next_enabled = enabled.unwrap_or(!profile.agent.advisor);
            profile.agent.advisor = next_enabled;

            let next_preset = preset
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
                .or_else(|| {
                    (next_enabled && profile.agent.advisor_preset.trim().is_empty())
                        .then(codex_ilhae::settings_types::default_advisor_preset)
                });
            if let Some(next_preset) = next_preset {
                profile.agent.advisor_preset = next_preset;
            }
            Ok(())
        })
        .await;
    }

    pub(super) async fn set_ilhae_auto_mode(&mut self, enabled: Option<bool>) {
        self.mutate_native_ilhae_active_profile("auto", move |profile| {
            profile.agent.auto_mode = enabled.unwrap_or(!profile.agent.auto_mode);
            Ok(())
        })
        .await;
    }

    pub(super) async fn set_ilhae_team_mode(&mut self, enabled: Option<bool>) {
        self.mutate_native_ilhae_active_profile("team", move |profile| {
            profile.agent.team_mode = enabled.unwrap_or(!profile.agent.team_mode);
            Ok(())
        })
        .await;
    }

    pub(super) async fn set_ilhae_dream_mode(&mut self, enabled: Option<bool>) {
        self.mutate_native_ilhae_active_profile("dream", move |profile| {
            profile.agent.dream_mode = enabled.unwrap_or(!profile.agent.dream_mode);
            Ok(())
        })
        .await;
    }

    pub(super) async fn set_ilhae_embed_mode(&mut self, enabled: Option<bool>) {
        self.mutate_native_ilhae_active_profile("embed", move |profile| {
            profile.agent.embed_mode = enabled.unwrap_or(!profile.agent.embed_mode);
            Ok(())
        })
        .await;
    }

    pub(super) async fn set_ilhae_kairos_mode(&mut self, enabled: Option<bool>) {
        self.mutate_native_ilhae_active_profile("kairos", move |profile| {
            profile.agent.kairos = enabled.unwrap_or(!profile.agent.kairos);
            Ok(())
        })
        .await;
    }

    pub(super) async fn set_ilhae_improve_mode(&mut self, enabled: Option<bool>) {
        self.mutate_native_ilhae_active_profile("improve", move |profile| {
            profile.agent.self_improvement = enabled.unwrap_or(!profile.agent.self_improvement);
            Ok(())
        })
        .await;
    }

    pub(super) async fn flush_ilhae_runtime_events(&mut self, app_server: &mut AppServerSession) {
        let Some(runtime) = native_runtime_context() else {
            return;
        };
        while let Some(event) = app_server.next_ilhae_event().await {
            runtime
                .cx_cache
                .notify_desktop_for_session(
                    NOTIF_APP_SESSION_EVENT,
                    serde_json::to_value(&event).unwrap_or(serde_json::Value::Null),
                    event.session_id(),
                )
                .await;
        }
    }
}
