use codex_tools::ToolSpec;
use super::{brain_artifact_ops_spec::create_brain_artifact_ops_tool, brain_memory_ops_spec::create_brain_memory_ops_tool};

pub fn create_all_brain_tools() -> Vec<ToolSpec> {
    vec![create_brain_memory_ops_tool(), create_brain_artifact_ops_tool()]
}
pub fn create_all_browser_tools() -> Vec<ToolSpec> {
    Vec::new()
}
pub fn create_all_computer_tools() -> Vec<ToolSpec> {
    Vec::new()
}