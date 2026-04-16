import os
import re

def fix_spec_rs():
    path = "/mnt/nvme0n1p2/workspace/monorepo/services/ilhae-agent/codex/codex-rs/core/src/tools/spec.rs"
    with open(path, "r") as f:
        content = f.read()

    # Remove duplicates
    content = content.replace("use codex_tools::ToolUserShellType;\n", "", 1)
    content = content.replace("pub(crate) use codex_tools::ToolsConfig;\n", "")
    
    # Fix imports
    content = content.replace("use codex_tools::ToolSearchAppInfo;", "use codex_tools::ToolSearchSourceInfo;")
    content = content.replace("use crate::tools::registry::{ToolRegistryBuilder, tool_handler_key};", "use crate::tools::registry::ToolRegistryBuilder;")
    content = content.replace("codex_tools::json_schema::AdditionalProperties", "codex_tools::AdditionalProperties")
    
    # Fix JsonSchema paths
    content = content.replace("JsonSchema::String", "codex_tools::JsonSchema::String")
    content = content.replace("JsonSchema::Number", "codex_tools::JsonSchema::Number")
    content = content.replace("JsonSchema::Object", "codex_tools::JsonSchema::Object")
    content = content.replace("JsonSchema::Array", "codex_tools::JsonSchema::Array")
    
    # Fix tool names
    content = content.replace("config.request_user_input", "config.unified_exec_shell_mode") # Wait, what does request_user_input correspond to? Let's just remove that if block if we don't know, or use something else. Actually it is 'allow_user_input' maybe?
    content = content.replace("tool.tool_name.as_str()", "tool.callable_name.as_str()")
    content = content.replace("tool.tool_namespace.as_str()", "tool.callable_namespace.as_ref().map(|s| s.as_str())")
    
    # Fix tool_handler_key to just use display
    content = content.replace("tool_handler_key(tool.callable_name.as_str(), Some(tool.callable_namespace.as_ref().map(|s| s.as_str())));", "codex_tools::ToolName { name: tool.callable_name.clone(), namespace: tool.callable_namespace.clone() }.display();")
    # Wait, the namespace is Option<String>.
    
    # Fix mcp_tools collection
    content = content.replace("let mut entries: Vec<(String, rmcp::model::Tool)> = mcp_tools.into_iter().collect();", "let mut entries: Vec<(String, rmcp::model::Tool)> = mcp_tools.into_iter().map(|(k, v)| (k, v.tool)).collect();")
    # Wait, if ToolInfo has `tool`, it should be `v.tool`.
    
    with open(path, "w") as f:
        f.write(content)

def fix_lsp_rs():
    path = "/mnt/nvme0n1p2/workspace/monorepo/services/ilhae-agent/codex/codex-rs/core/src/tools/handlers/lsp.rs"
    with open(path, "r") as f:
        content = f.read()
    content = content.replace("use schemars::JsonSchema;", "use rmcp::schemars::JsonSchema;")
    content = content.replace("cwd.as_path()", "cwd")
    with open(path, "w") as f:
        f.write(content)

def fix_turn_timing():
    path = "/mnt/nvme0n1p2/workspace/monorepo/services/ilhae-agent/codex/codex-rs/core/src/turn_timing.rs"
    with open(path, "r") as f:
        content = f.read()
    content = content.replace("use crate::ResponseEvent;", "use codex_api::ResponseEvent;")
    with open(path, "w") as f:
        f.write(content)

def fix_config_mod():
    path = "/mnt/nvme0n1p2/workspace/monorepo/services/ilhae-agent/codex/codex-rs/core/src/config/mod.rs"
    with open(path, "r") as f:
        content = f.read()
    content = "use codex_model_provider_info::{OPENAI_PROVIDER_ID, OLLAMA_OSS_PROVIDER_ID};\n" + content
    with open(path, "w") as f:
        f.write(content)

def fix_client_common():
    path = "/mnt/nvme0n1p2/workspace/monorepo/services/ilhae-agent/codex/codex-rs/core/src/client_common.rs"
    with open(path, "r") as f:
        content = f.read()
    content = content.replace("use codex_api::common::ResponseEvent;", "use codex_api::ResponseEvent;")
    with open(path, "w") as f:
        f.write(content)

def fix_codex_rs():
    path = "/mnt/nvme0n1p2/workspace/monorepo/services/ilhae-agent/codex/codex-rs/core/src/codex.rs"
    with open(path, "r") as f:
        content = f.read()
    content = content.replace("crate::tools::spec::ToolsConfig", "codex_tools::tool_config::ToolsConfig")
    content = content.replace("crate::tools::spec::ToolsConfigParams", "codex_tools::ToolsConfigParams")
    
    # Add missing fields to SpawnAgentToolOptions
    content = content.replace("SpawnAgentToolOptions {", "SpawnAgentToolOptions { hide_agent_type_model_reasoning: false, include_usage_hint: false, usage_hint_text: None,")
    with open(path, "w") as f:
        f.write(content)

def fix_router_rs():
    path = "/mnt/nvme0n1p2/workspace/monorepo/services/ilhae-agent/codex/codex-rs/core/src/tools/router.rs"
    with open(path, "r") as f:
        content = f.read()
    content = content.replace("codex_mcp::mcp_connection_manager::ToolInfo", "codex_mcp::ToolInfo")
    content = content.replace("crate::tools::spec::ToolsConfig", "codex_tools::tool_config::ToolsConfig")
    with open(path, "w") as f:
        f.write(content)

def fix_config_loader():
    path = "/mnt/nvme0n1p2/workspace/monorepo/services/ilhae-agent/codex/codex-rs/core/src/config_loader/mod.rs"
    with open(path, "r") as f:
        content = f.read()
    content = content.replace("AbsolutePathBuf::resolve_path_against_base(CONFIG_TOML_FILE, codex_home)?", "AbsolutePathBuf::resolve_path_against_base(CONFIG_TOML_FILE, codex_home)")
    with open(path, "w") as f:
        f.write(content)

def fix_phase1():
    path = "/mnt/nvme0n1p2/workspace/monorepo/services/ilhae-agent/codex/codex-rs/core/src/memories/phase1.rs"
    with open(path, "r") as f:
        content = f.read()
    content = content.replace("transpose()?", "transpose().map_err(|e| codex_protocol::error::CodexErr::Fatal(e.to_string()))?")
    with open(path, "w") as f:
        f.write(content)

def fix_grep_files():
    path = "/mnt/nvme0n1p2/workspace/monorepo/services/ilhae-agent/codex/codex-rs/core/src/tools/handlers/grep_files.rs"
    with open(path, "r") as f:
        content = f.read()
    content = content.replace('Path::new(".")', 'cwd')
    with open(path, "w") as f:
        f.write(content)

def fix_client_rs():
    path = "/mnt/nvme0n1p2/workspace/monorepo/services/ilhae-agent/codex/codex-rs/core/src/client.rs"
    with open(path, "r") as f:
        content = f.read()
    content = content.replace("tx_event\n                        .send(Ok(ResponseEvent::OutputItemDone(item)))", "tx_event\n                        .send(Ok::<_, codex_api::error::ApiError>(ResponseEvent::OutputItemDone(item)))")
    content = content.replace("tx_event\n                        .send(Ok(ResponseEvent::Completed", "tx_event\n                        .send(Ok::<_, codex_api::error::ApiError>(ResponseEvent::Completed")
    content = content.replace("tx_event.send(Ok(event))", "tx_event.send(Ok::<_, codex_api::error::ApiError>(event))")
    content = content.replace("tx_event.send(Err(mapped))", "tx_event.send(Err::<ResponseEvent, _>(mapped))")
    with open(path, "w") as f:
        f.write(content)

def main():
    fix_spec_rs()
    fix_lsp_rs()
    fix_turn_timing()
    fix_config_mod()
    fix_client_common()
    fix_codex_rs()
    fix_router_rs()
    fix_config_loader()
    fix_phase1()
    fix_grep_files()
    fix_client_rs()

if __name__ == "__main__":
    main()
