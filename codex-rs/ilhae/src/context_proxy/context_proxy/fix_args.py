import os
import re
import json

base_sys = '/mnt/nvme0n1p2/workspace/monorepo/services/ilhae-agent/ilhae-proxy/src/context_proxy'

def process(filepath, func):
    with open(filepath, 'r') as f: content = f.read()
    content = func(content)
    with open(filepath, 'w') as f: f.write(content)

def fix_routing2(s):
    # Fix store.add_full_message_with_blocks(
    #         &session_id, "assistant",
    #         &assistant_text,
    #         None::<String>, None::<String>, None::<String>, None::<String>,
    #         0,
    #     )
    # To: -> SqlResult<()> { add_full_message_with_blocks(&self, session_id: &str, role: &str, content: &str, parent_message_id: i64, thread_id: i64, thinking: &str, duration_ms: i64) }
    s = re.sub(
        r'store\.add_full_message_with_blocks\(\s*&session_id,\s*"assistant",\s*&assistant_text,\s*.*?\)',
        'store.add_full_message_with_blocks(&session_id, "assistant", &assistant_text, 0, 0, "", 0)',
        s, flags=re.DOTALL
    )
    
    # Fix upsert_agent_message args
    # db.upsert_agent_message(
    #     &sid, "assistant", &prev_content,
    #     &role_name, meta_model, &prev_thinking, &tool_calls_str, &content_blocks_str,
    # )
    # To: upsert_agent_message(&self, session_id: &str, role: &str, content: &str, agent_id: &str, thinking: &str)
    s = re.sub(
        r'db\.upsert_agent_message\(\s*&sid,\s*"assistant",\s*&prev_content,\s*&role_name,\s*[^,]+,\s*&prev_thinking[^)]*\)',
        'db.upsert_agent_message(&sid, "assistant", &prev_content, &role_name, &prev_thinking)',
        s, flags=re.DOTALL
    )
    return s

process(os.path.join(base_sys, 'routing.rs'), fix_routing2)

def fix_a2a_again(s):
    # Fix any failed pub replacements
    s = s.replace("assistant_text: String,", "pub assistant_text: String,")
    s = s.replace("structured: serde_json::Value,", "pub structured: serde_json::Value,")
    # Fix unused imports of Value vs json
    return s

process(os.path.join(base_sys, 'team_a2a.rs'), fix_a2a_again)

# For role_parser.rs: error: use of undeclared type or module `Value` at line 14?
# Wait, `use serde_json::{json, Value};` was declared multiple times?
def fix_role_parser2(s):
    s = s.replace("use serde_json::{json, Value};\nuse serde_json::Value;", "use serde_json::{json, Value};")
    s = s.replace("use serde_json::Value;\nuse serde_json::{json, Value};", "use serde_json::{json, Value};")
    return s
process(os.path.join(base_sys, 'role_parser.rs'), fix_role_parser2)
process(os.path.join(base_sys, 'team_webhook.rs'), fix_role_parser2)
process(os.path.join(base_sys, 'routing.rs'), fix_role_parser2)
process(os.path.join(base_sys, 'team_a2a.rs'), fix_role_parser2)

print("Done")
