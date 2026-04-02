pub mod artifact;
pub mod memory;
pub mod misc;
pub mod session;
pub mod task;
pub mod team;

#[macro_export]
macro_rules! check_tool_enabled {
    ($settings_store:expr, $tool_name:expr) => {{
        if let Some(plugin_id) = $crate::tool_to_plugin_id($tool_name) {
            let cfg = $settings_store.get();
            let default_enabled = plugin_id != "browser";
            if !cfg
                .plugins
                .get(plugin_id)
                .copied()
                .unwrap_or(default_enabled)
            {
                return Err(sacp::Error::invalid_request().data(format!(
                    "Plugin '{}' is disabled. Enable it in Settings → Plugins.",
                    plugin_id
                )));
            }
            // Per-tool disable: plugins.<id>.<tool> = false
            let tool_key = format!("{}.{}", plugin_id, $tool_name);
            if !cfg.plugins.get(&tool_key).copied().unwrap_or(true) {
                return Err(sacp::Error::invalid_request()
                    .data(format!("Tool '{}' is disabled.", $tool_name)));
            }
        }
    }};
}
