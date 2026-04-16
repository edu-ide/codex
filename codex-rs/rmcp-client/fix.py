import re

with open("src/rmcp_client.rs", "r") as f:
    data = f.read()

data = data.replace(
    "AuthRequiredError::new(\n                header,\n            )",
    "AuthRequiredError { www_authenticate_header: header, ..Default::default() }"
)

data = data.replace(
    "let rmcp_params = CallToolRequestParams::builder(name.into(), arguments).build();",
    "let mut rmcp_params = CallToolRequestParams::new(name.into());\n        rmcp_params.arguments = arguments;"
)

data = data.replace(
    "rmcp::model::CallToolRequest::builder(rmcp_params).build()",
    "rmcp::model::CallToolRequest::new(rmcp_params)"
)

data = data.replace(
    "rmcp::service::PeerRequestOptions::builder()\n                        .meta(meta)\n                        .build()",
    "{ let mut o = rmcp::service::PeerRequestOptions::no_options(); o.meta = meta; o }"
)

with open("src/rmcp_client.rs", "w") as f:
    f.write(data)
