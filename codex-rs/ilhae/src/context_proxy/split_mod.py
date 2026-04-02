import sys, re

mod_path = 'mod.rs'
with open(mod_path, 'r', encoding='utf-8') as f:
    lines = f.readlines()

new_mod_lines = []
skip_ranges = [
    (57, 146),     # Team structs & config -> team_a2a.rs
    (149, 299),    # A2A response parsing -> team_a2a.rs
    (300, 330),    # SSE parsing -> team_webhook.rs
    (331, 477),    # dispatch_a2a_send_once & stream -> team_webhook.rs
    (480, 887),    # spawn_team_event_webhook -> team_webhook.rs
    (889, 937),    # extract_role_sections -> role_parser.rs
    (939, 1138),   # generate_peer_registration_files -> role_parser.rs
    (1139, 1504),  # A2A Server auto-spawn helpers -> team_a2a.rs
    (1506, 1702),  # spawn_acp_sse_observers -> routing.rs
    (1704, 1983),  # run_team_orchestration_live -> routing.rs
    (1985, 2049),  # abort/cancel helpers -> routing.rs
    (2051, 2341),  # role extraction and validation -> role_parser.rs
    (2343, 2445),  # looks_like, infer_team_role, persist helpers -> routing.rs
]

team_a2a_lines = []
team_webhook_lines = []
role_parser_lines = []
routing_lines = []

def add_lines(dest, start, end):
    for i in range(start, end + 1):
        line = lines[i - 1]
        # Make structs and fns public so they can be used across modules
        line = re.sub(r'^fn ', 'pub(crate) fn ', line)
        line = re.sub(r'^async fn ', 'pub(crate) async fn ', line)
        line = re.sub(r'^struct ', 'pub(crate) struct ', line)
        line = re.sub(r'^enum ', 'pub(crate) enum ', line)
        dest.append(line)

# Extract code to various files
add_lines(team_a2a_lines, 57, 146)
add_lines(team_a2a_lines, 149, 299)
add_lines(team_a2a_lines, 1139, 1504)

add_lines(team_webhook_lines, 300, 330)
add_lines(team_webhook_lines, 331, 477)
add_lines(team_webhook_lines, 480, 887)

add_lines(role_parser_lines, 889, 937)
add_lines(role_parser_lines, 939, 1138)
add_lines(role_parser_lines, 2051, 2341)

add_lines(routing_lines, 1506, 1702)
add_lines(routing_lines, 1704, 1983)
add_lines(routing_lines, 1985, 2049)
add_lines(routing_lines, 2343, 2445)

header = """use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use agent_client_protocol_schema::{
    CancelNotification, ContentBlock, PromptRequest, PromptResponse, RequestPermissionOutcome,
    RequestPermissionRequest, RequestPermissionResponse, SelectedPermissionOutcome, StopReason,
    TextContent,
};
use regex::Regex;
use sacp::{Agent, Client, Conductor, ConnectTo, ConnectionTo, Proxy, Responder, UntypedMessage};
use serde::Deserialize;
use serde_json::json;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::approval_manager::{ApprovalEvent, ApprovalOption, ApprovalRequest};
use crate::memory_store;
use crate::relay_server::{self, RelayEvent};
use crate::{
    apply_codex_profile_to_config, build_dynamic_instructions, infer_agent_id_from_command,
    send_synthetic_tool_call, AssistantBuffer, SetSessionConfigOptionRequest,
    SetSessionConfigOptionResponse, CapabilitiesRequest, CapabilitiesResponse,
    ToggleSkillRequest, ToggleSkillResponse, ToggleMcpRequest, ToggleMcpResponse,
};

// Allow unused imports since we just dump all common imports
#![allow(unused_imports)]
use crate::context_proxy::team_a2a::*;
use crate::context_proxy::team_webhook::*;
use crate::context_proxy::role_parser::*;
use crate::context_proxy::routing::*;
use super::*;
"""

def write_file(name, body_lines):
    with open(name, 'w', encoding='utf-8') as f:
        f.write(header)
        f.write("\n")
        f.writelines(body_lines)

write_file('team_a2a.rs', team_a2a_lines)
write_file('team_webhook.rs', team_webhook_lines)
write_file('role_parser.rs', role_parser_lines)
write_file('routing.rs', routing_lines)

# Write out mod.rs omitting those ranges
i = 1
for line in lines:
    skip = False
    for r in skip_ranges:
        if r[0] <= i <= r[1]:
            skip = True
            break
    if not skip:
        # Before we add the line, if we are at line 35 (the constants), inject the mods
        if i == 35:
            new_mod_lines.append("pub mod team_a2a;\n")
            new_mod_lines.append("pub mod team_webhook;\n")
            new_mod_lines.append("pub mod role_parser;\n")
            new_mod_lines.append("pub mod routing;\n")
            new_mod_lines.append("pub use team_a2a::*;\n")
            new_mod_lines.append("pub use team_webhook::*;\n")
            new_mod_lines.append("pub use role_parser::*;\n")
            new_mod_lines.append("pub use routing::*;\n")
        new_mod_lines.append(line)
        
    i += 1

with open(mod_path, 'w', encoding='utf-8') as f:
    f.writelines(new_mod_lines)

print("Split complete.")
