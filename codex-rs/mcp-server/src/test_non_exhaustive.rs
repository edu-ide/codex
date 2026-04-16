use rmcp::model::*;
pub fn test() {
    let mut info = Implementation::default();
    info.name = "test".to_string();
    
    let mut cap = ServerCapabilities::default();
    let mut tc = ToolsCapability::default();
    tc.list_changed = Some(true);
    cap.tools = Some(tc);
    
    let mut ir = InitializeResult::default();
    ir.capabilities = cap;
    ir.server_info = info;
    
    let mut tool = Tool::default();
    tool.name = "codex".to_string().into();
}
