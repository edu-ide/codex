import re

with open("src/mcp_connection_manager.rs", "r") as f:
    data = f.read()

# Fix PaginatedRequestParams
data = re.sub(
    r"PaginatedRequestParams \{\s*meta: None,\s*cursor: Some\(next\.clone\(\)\),\s*\}",
    "{ let mut p = rmcp::model::PaginatedRequestParams::default(); p.cursor = Some(next.clone()); p }",
    data
)

data = re.sub(
    r"let params = InitializeRequestParams \{[\s\S]*?\.\.Default::default\(\)\n\s*\};",
    """let mut elicitation_opts = rmcp::model::ClientElicitationCapabilities::default();
    elicitation_opts.supported = Some(true);
    let mut caps = rmcp::model::ClientCapabilities::default();
    caps.elicitation = Some(elicitation_opts);
    
    let mut info = rmcp::model::Implementation::new("codex-mcp-client".to_owned(), env!("CARGO_PKG_VERSION").to_owned());
    info.title = Some("Codex".into());
    
    let mut params = rmcp::model::InitializeRequestParams::new(
        rmcp::model::ProtocolVersion::V2024_11_05,
        caps,
        info,
    );""",
    data
)

with open("src/mcp_connection_manager.rs", "w") as f:
    f.write(data)
