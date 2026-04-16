import re

with open("src/rmcp_client.rs", "r") as f:
    data = f.read()

# Fix AuthRequiredError -> UnexpectedServerResponse
data = re.sub(
    r"return Err\(StreamableHttpError::AuthRequired.*?\}\)\);",
    'return Err(StreamableHttpError::UnexpectedServerResponse(Cow::Owned(format!("auth required: {}", header))));',
    data, flags=re.DOTALL
)

# Fix PeerRequestOptions
data = re.sub(
    r"rmcp::service::PeerRequestOptions::builder\(\)[\s\n]*\.meta\(meta\)[\s\n]*\.build\(\)",
    '{ let mut o = rmcp::service::PeerRequestOptions::no_options(); o.meta = Some(meta); o }',
    data, flags=re.DOTALL
)

with open("src/rmcp_client.rs", "w") as f:
    f.write(data)
