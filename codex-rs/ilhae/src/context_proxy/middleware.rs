use std::sync::Arc;

use crate::SharedState;

/// A middleware pipeline component for the ContextProxy.
///
/// Implementing structs encapsulate specific routing rules and handlers
/// (such as authentication or prompt injection) and append them
/// to the SACP Proxy Builder.
pub trait ContextMiddleware {
    /// Applies this middleware's routes and handlers to the given builder.
    fn apply(
        self,
        builder: sacp::Builder<sacp::Proxy>,
        state: Arc<SharedState>,
    ) -> sacp::Builder<sacp::Proxy>;
}
