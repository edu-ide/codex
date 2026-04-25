use tracing::debug;
use tracing::warn;

use super::Session;
use crate::tools::registry::ToolRegistry;

pub(super) async fn maybe_sync_registry_with_brain(
    sess: &Session,
    registry: &ToolRegistry,
) {
    let should_sync = {
        let state = sess.state.lock().await;
        !state.is_brain_sync_completed()
    };
    debug!(
        should_sync,
        "evaluating plugin registry brain sync during turn build"
    );
    if !should_sync {
        return;
    }

    debug!("plugin registry brain sync start");
    if let Err(err) = registry.sync_with_brain().await {
        warn!("failed to synchronize tool registry with brain: {err}");
        let mut state = sess.state.lock().await;
        state.set_brain_sync_completed(false);
    } else {
        let mut state = sess.state.lock().await;
        state.set_brain_sync_completed(true);
        debug!("plugin registry brain sync completed");
    }
}
