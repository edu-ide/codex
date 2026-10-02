use crate::SharedState;

pub async fn handle_browser_launch(
    ctx: &SharedState,
    cmd: &crate::relay_server::RelayCommand,
    _client_id: u32,
    maybe_respond: impl Fn(Option<&str>, serde_json::Value, Option<String>),
) {
    let settings = ctx.infra.settings_store.get();
    match ctx.infra.browser_mgr.launch(&settings.browser).await {
        Ok(status) => maybe_respond(cmd.request_id.as_deref(), serde_json::json!(status), None),
        Err(error) => maybe_respond(
            cmd.request_id.as_deref(),
            serde_json::Value::Null,
            Some(error),
        ),
    }
}

pub async fn handle_browser_stop(
    ctx: &SharedState,
    cmd: &crate::relay_server::RelayCommand,
    _client_id: u32,
    maybe_respond: impl Fn(Option<&str>, serde_json::Value, Option<String>),
) {
    match ctx.infra.browser_mgr.stop().await {
        Ok(status) => maybe_respond(cmd.request_id.as_deref(), serde_json::json!(status), None),
        Err(error) => maybe_respond(
            cmd.request_id.as_deref(),
            serde_json::Value::Null,
            Some(error),
        ),
    }
}
