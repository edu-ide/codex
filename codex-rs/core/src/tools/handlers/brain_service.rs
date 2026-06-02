use crate::function_tool::FunctionCallError;
use brain_rs::BrainService;
use std::sync::{Arc, OnceLock};

static BRAIN_SERVICE: OnceLock<Result<Arc<BrainService>, String>> = OnceLock::new();

pub(crate) fn shared_brain_service() -> Result<Arc<BrainService>, FunctionCallError> {
    BRAIN_SERVICE
        .get_or_init(|| {
            let data_dir = BrainService::resolve_data_dir();
            BrainService::new(&data_dir, None)
                .map(Arc::new)
                .map_err(|err| err.to_string())
        })
        .clone()
        .map_err(|err| {
            FunctionCallError::RespondToModel(format!("failed to initialize Brain service: {err}"))
        })
}