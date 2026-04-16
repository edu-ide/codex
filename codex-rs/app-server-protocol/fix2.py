import re

with open("src/protocol/v2.rs", "r") as f:
    data = f.read()

data = data.replace(
    "            meta: value.meta,\n        }\n    }\n}\n\nimpl From<rmcp::model::CreateElicitationResult> for McpServerElicitationRequestResponse {\n    fn from(value: rmcp::model::CreateElicitationResult) -> Self {\n        Self {\n            action: value.action.into(),\n            content: value.content,\n            meta: value.meta,\n        }\n    }\n}",
    """            meta: value.meta.and_then(|m| serde_json::from_value(m).ok()),
        }
    }
}

impl From<rmcp::model::CreateElicitationResult> for McpServerElicitationRequestResponse {
    fn from(value: rmcp::model::CreateElicitationResult) -> Self {
        Self {
            action: value.action.into(),
            content: value.content,
            meta: value.meta.and_then(|m| serde_json::to_value(m).ok()),
        }
    }
}"""
)

with open("src/protocol/v2.rs", "w") as f:
    f.write(data)
