// commands

use crate::SharedState;
pub async fn handle_browser_launch(
    ctx: &SharedState,
    _cmd: &crate::relay_server::RelayCommand,
    _client_id: u32,
    _maybe_respond: impl Fn(Option<&str>, serde_json::Value, Option<String>),
) {
    let s = ctx.infra.settings_store.get();
    let _ = ctx.infra.browser_mgr.launch(
        &s.browser.browser_type,
        s.browser.cdp_port,
        s.browser.headless,
        s.browser.persistent,
        &s.browser.server_url,
    );
}

pub async fn handle_browser_stop(
    ctx: &SharedState,
    _cmd: &crate::relay_server::RelayCommand,
    _client_id: u32,
    _maybe_respond: impl Fn(Option<&str>, serde_json::Value, Option<String>),
) {
    let _ = ctx.infra.browser_mgr.stop();
}
