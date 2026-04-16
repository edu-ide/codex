import re

with open("src/mcp_connection_manager.rs", "r") as f:
    data = f.read()

data = data.replace(
    "let mut elicitation_opts = rmcp::model::ClientElicitationCapabilities::default();",
    "let mut elicitation_opts = rmcp::model::ElicitationCapability::default();"
)

data = data.replace(
    """let mut params = rmcp::model::InitializeRequestParams::new(
        rmcp::model::ProtocolVersion::V2024_11_05,
        caps,
        info,
    );""",
    """let mut params = rmcp::model::InitializeRequestParams::new(
        caps,
        info,
    );"""
)

with open("src/mcp_connection_manager.rs", "w") as f:
    f.write(data)
