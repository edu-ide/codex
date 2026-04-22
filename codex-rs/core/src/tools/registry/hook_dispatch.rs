use std::time::Duration;

use crate::function_tool::FunctionCallError;
use crate::sandbox_tags::sandbox_tag;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use codex_hooks::HookEvent;
use codex_hooks::HookEventAfterToolUse;
use codex_hooks::HookPayload;
use codex_hooks::HookResult;
use codex_hooks::HookToolInput;
use codex_hooks::HookToolInputLocalShell;
use codex_hooks::HookToolKind;
use codex_protocol::protocol::SandboxPolicy;
use codex_tools::ToolName;
use tracing::warn;

pub(super) fn unsupported_tool_call_message(payload: &ToolPayload, tool_name: &ToolName) -> String {
    let tool_name = tool_name.display();
    match payload {
        ToolPayload::Custom { .. } => format!("unsupported custom tool call: {tool_name}"),
        _ => format!("unsupported call: {tool_name}"),
    }
}

pub(super) fn sandbox_policy_tag(policy: &SandboxPolicy) -> &'static str {
    match policy {
        SandboxPolicy::ReadOnly { .. } => "read-only",
        SandboxPolicy::WorkspaceWrite { .. } => "workspace-write",
        SandboxPolicy::DangerFullAccess => "danger-full-access",
        SandboxPolicy::ExternalSandbox { .. } => "external-sandbox",
    }
}

// Hooks use a separate wire-facing input type so hook payload JSON stays stable
// and decoupled from core's internal tool runtime representation.
impl From<&ToolPayload> for HookToolInput {
    fn from(payload: &ToolPayload) -> Self {
        match payload {
            ToolPayload::Function { arguments } => HookToolInput::Function {
                arguments: arguments.clone(),
            },
            ToolPayload::ToolSearch { arguments } => HookToolInput::Function {
                arguments: serde_json::json!({
                    "query": arguments.query,
                    "limit": arguments.limit,
                })
                .to_string(),
            },
            ToolPayload::Custom { input } => HookToolInput::Custom {
                input: input.clone(),
            },
            ToolPayload::LocalShell { params } => HookToolInput::LocalShell {
                params: HookToolInputLocalShell {
                    command: params.command.clone(),
                    workdir: params.workdir.clone(),
                    timeout_ms: params.timeout_ms,
                    sandbox_permissions: params.sandbox_permissions,
                    prefix_rule: params.prefix_rule.clone(),
                    justification: params.justification.clone(),
                },
            },
            ToolPayload::Mcp {
                server,
                tool,
                raw_arguments,
            } => HookToolInput::Mcp {
                server: server.clone(),
                tool: tool.clone(),
                arguments: raw_arguments.clone(),
            },
        }
    }
}

fn hook_tool_kind(tool_input: &HookToolInput) -> HookToolKind {
    match tool_input {
        HookToolInput::Function { .. } => HookToolKind::Function,
        HookToolInput::Custom { .. } => HookToolKind::Custom,
        HookToolInput::LocalShell { .. } => HookToolKind::LocalShell,
        HookToolInput::Mcp { .. } => HookToolKind::Mcp,
    }
}

pub(super) async fn dispatch_after_tool_use_hook(
    invocation: &ToolInvocation,
    output_preview: String,
    success: bool,
    executed: bool,
    duration: Duration,
    mutating: bool,
) -> Option<FunctionCallError> {
    let session = invocation.session.as_ref();
    let turn = invocation.turn.as_ref();
    let tool_input = HookToolInput::from(&invocation.payload);
    let hook_outcomes = session
        .hooks()
        .dispatch(HookPayload {
            session_id: session.conversation_id,
            cwd: turn.cwd.clone(),
            client: turn.app_server_client_name.clone(),
            triggered_at: chrono::Utc::now(),
            hook_event: HookEvent::AfterToolUse {
                event: HookEventAfterToolUse {
                    turn_id: turn.sub_id.clone(),
                    call_id: invocation.call_id.clone(),
                    tool_name: invocation.tool_name.display(),
                    tool_kind: hook_tool_kind(&tool_input),
                    tool_input,
                    executed,
                    success,
                    duration_ms: u64::try_from(duration.as_millis()).unwrap_or(u64::MAX),
                    mutating,
                    sandbox: sandbox_tag(&turn.sandbox_policy, turn.windows_sandbox_level)
                        .to_string(),
                    sandbox_policy: sandbox_policy_tag(&turn.sandbox_policy).to_string(),
                    output_preview: output_preview.clone(),
                },
            },
        })
        .await;

    for hook_outcome in hook_outcomes {
        let hook_name = hook_outcome.hook_name;
        match hook_outcome.result {
            HookResult::Success => {}
            HookResult::FailedContinue(error) => {
                warn!(
                    call_id = %invocation.call_id,
                    tool_name = %invocation.tool_name.display(),
                    hook_name = %hook_name,
                    error = %error,
                    "after_tool_use hook failed; continuing"
                );
            }
            HookResult::FailedAbort(error) => {
                warn!(
                    call_id = %invocation.call_id,
                    tool_name = %invocation.tool_name.display(),
                    hook_name = %hook_name,
                    error = %error,
                    "after_tool_use hook failed; aborting operation"
                );
                return Some(FunctionCallError::Fatal(format!(
                    "after_tool_use hook '{hook_name}' failed and aborted operation: {error}"
                )));
            }
        }
    }

    None
}
