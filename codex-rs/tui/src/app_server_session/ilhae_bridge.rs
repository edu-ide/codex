use agent_client_protocol_schema::PermissionOptionKind;
use agent_client_protocol_schema::RequestPermissionOutcome;
use agent_client_protocol_schema::RequestPermissionRequest;
use agent_client_protocol_schema::RequestPermissionResponse;
use agent_client_protocol_schema::SelectedPermissionOutcome;
use codex_app_server_client::AppServerEvent;
use codex_app_server_protocol::CommandExecutionApprovalDecision;
use codex_app_server_protocol::RequestId;
use codex_app_server_protocol::ServerNotification;
use codex_app_server_protocol::ThreadItem;
use codex_app_server_protocol::TurnStatus;
use codex_ilhae::IlhaeAppSessionEventDto;
use codex_ilhae::IlhaeAppSessionEventNotification;
use codex_ilhae::IlhaeInteractiveOptionDto;
use codex_ilhae::IlhaeInteractiveOptionKind;
use codex_ilhae::IlhaeInteractiveRequestDto;

fn select_acp_permission_option(
    request: &RequestPermissionRequest,
    preferred_kinds: &[PermissionOptionKind],
) -> Option<String> {
    preferred_kinds.iter().find_map(|preferred| {
        request
            .options
            .iter()
            .find(|option| option.kind == *preferred)
            .map(|option| option.option_id.to_string())
    })
}

pub(super) fn ilhae_interactive_option_kind(
    kind: PermissionOptionKind,
) -> IlhaeInteractiveOptionKind {
    match kind {
        PermissionOptionKind::AllowOnce => IlhaeInteractiveOptionKind::ApproveOnce,
        PermissionOptionKind::AllowAlways => IlhaeInteractiveOptionKind::ApproveSession,
        PermissionOptionKind::RejectOnce => IlhaeInteractiveOptionKind::RejectOnce,
        PermissionOptionKind::RejectAlways => IlhaeInteractiveOptionKind::RejectSession,
        _ => IlhaeInteractiveOptionKind::Custom,
    }
}

pub(super) fn acp_permission_response_from_exec_decision(
    request: &RequestPermissionRequest,
    decision: CommandExecutionApprovalDecision,
) -> RequestPermissionResponse {
    let selected = match decision {
        CommandExecutionApprovalDecision::Accept => select_acp_permission_option(
            request,
            &[
                PermissionOptionKind::AllowOnce,
                PermissionOptionKind::AllowAlways,
            ],
        ),
        CommandExecutionApprovalDecision::AcceptForSession
        | CommandExecutionApprovalDecision::AcceptWithExecpolicyAmendment { .. }
        | CommandExecutionApprovalDecision::ApplyNetworkPolicyAmendment { .. } => {
            select_acp_permission_option(
                request,
                &[
                    PermissionOptionKind::AllowAlways,
                    PermissionOptionKind::AllowOnce,
                ],
            )
        }
        CommandExecutionApprovalDecision::Decline => select_acp_permission_option(
            request,
            &[
                PermissionOptionKind::RejectOnce,
                PermissionOptionKind::RejectAlways,
            ],
        ),
        CommandExecutionApprovalDecision::Cancel => None,
    };

    if let Some(option_id) = selected {
        RequestPermissionResponse::new(RequestPermissionOutcome::Selected(
            SelectedPermissionOutcome::new(option_id),
        ))
    } else {
        RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled)
    }
}

fn turn_status_label(status: TurnStatus) -> &'static str {
    match status {
        TurnStatus::Completed => "completed",
        TurnStatus::Interrupted => "interrupted",
        TurnStatus::Failed => "failed",
        TurnStatus::InProgress => "in_progress",
    }
}

fn ilhae_interactive_options_from_exec_decisions(
    decisions: Option<&Vec<CommandExecutionApprovalDecision>>,
) -> Vec<IlhaeInteractiveOptionDto> {
    decisions
        .cloned()
        .unwrap_or_else(|| {
            vec![
                CommandExecutionApprovalDecision::Accept,
                CommandExecutionApprovalDecision::AcceptForSession,
                CommandExecutionApprovalDecision::Decline,
                CommandExecutionApprovalDecision::Cancel,
            ]
        })
        .into_iter()
        .map(|decision| match decision {
            CommandExecutionApprovalDecision::Accept => IlhaeInteractiveOptionDto {
                id: "approve_once".to_string(),
                label: "Approve once".to_string(),
                kind: IlhaeInteractiveOptionKind::ApproveOnce,
            },
            CommandExecutionApprovalDecision::AcceptForSession => IlhaeInteractiveOptionDto {
                id: "approve_session".to_string(),
                label: "Approve for session".to_string(),
                kind: IlhaeInteractiveOptionKind::ApproveSession,
            },
            CommandExecutionApprovalDecision::Decline => IlhaeInteractiveOptionDto {
                id: "deny_once".to_string(),
                label: "Deny once".to_string(),
                kind: IlhaeInteractiveOptionKind::RejectOnce,
            },
            CommandExecutionApprovalDecision::Cancel => IlhaeInteractiveOptionDto {
                id: "cancel".to_string(),
                label: "Cancel".to_string(),
                kind: IlhaeInteractiveOptionKind::Cancel,
            },
            CommandExecutionApprovalDecision::AcceptWithExecpolicyAmendment { .. } => {
                IlhaeInteractiveOptionDto {
                    id: "approve_exec_policy".to_string(),
                    label: "Approve with exec policy amendment".to_string(),
                    kind: IlhaeInteractiveOptionKind::Custom,
                }
            }
            CommandExecutionApprovalDecision::ApplyNetworkPolicyAmendment { .. } => {
                IlhaeInteractiveOptionDto {
                    id: "approve_network_policy".to_string(),
                    label: "Approve with network policy amendment".to_string(),
                    kind: IlhaeInteractiveOptionKind::Custom,
                }
            }
        })
        .collect()
}

fn canonical_interactive_request_from_server_request(
    request: &codex_app_server_protocol::ServerRequest,
) -> Option<IlhaeInteractiveRequestDto> {
    match request {
        codex_app_server_protocol::ServerRequest::CommandExecutionRequestApproval {
            request_id,
            params,
        } => Some(IlhaeInteractiveRequestDto {
            source: "app_server".to_string(),
            thread_id: params.thread_id.clone(),
            turn_id: params.turn_id.clone(),
            request_id: match request_id {
                RequestId::String(value) => value.clone(),
                RequestId::Integer(value) => value.to_string(),
            },
            title: params
                .command
                .clone()
                .unwrap_or_else(|| "Command approval".to_string()),
            reason: params.reason.clone(),
            requested_permissions: None,
            options: ilhae_interactive_options_from_exec_decisions(
                params.available_decisions.as_ref(),
            ),
        }),
        codex_app_server_protocol::ServerRequest::PermissionsRequestApproval {
            request_id,
            params,
        } => Some(IlhaeInteractiveRequestDto {
            source: "app_server".to_string(),
            thread_id: params.thread_id.clone(),
            turn_id: params.turn_id.clone(),
            request_id: match request_id {
                RequestId::String(value) => value.clone(),
                RequestId::Integer(value) => value.to_string(),
            },
            title: "Permissions approval".to_string(),
            reason: params.reason.clone(),
            requested_permissions: Some(params.permissions.clone().into()),
            options: vec![
                IlhaeInteractiveOptionDto {
                    id: "approve_once".to_string(),
                    label: "Approve once".to_string(),
                    kind: IlhaeInteractiveOptionKind::ApproveOnce,
                },
                IlhaeInteractiveOptionDto {
                    id: "approve_session".to_string(),
                    label: "Approve for session".to_string(),
                    kind: IlhaeInteractiveOptionKind::ApproveSession,
                },
                IlhaeInteractiveOptionDto {
                    id: "cancel".to_string(),
                    label: "Cancel".to_string(),
                    kind: IlhaeInteractiveOptionKind::Cancel,
                },
            ],
        }),
        codex_app_server_protocol::ServerRequest::FileChangeRequestApproval {
            request_id,
            params,
        } => Some(IlhaeInteractiveRequestDto {
            source: "app_server".to_string(),
            thread_id: params.thread_id.clone(),
            turn_id: params.turn_id.clone(),
            request_id: match request_id {
                RequestId::String(value) => value.clone(),
                RequestId::Integer(value) => value.to_string(),
            },
            title: "File change approval".to_string(),
            reason: params.reason.clone(),
            requested_permissions: None,
            options: vec![
                IlhaeInteractiveOptionDto {
                    id: "approve_once".to_string(),
                    label: "Approve once".to_string(),
                    kind: IlhaeInteractiveOptionKind::ApproveOnce,
                },
                IlhaeInteractiveOptionDto {
                    id: "deny_once".to_string(),
                    label: "Deny once".to_string(),
                    kind: IlhaeInteractiveOptionKind::RejectOnce,
                },
                IlhaeInteractiveOptionDto {
                    id: "cancel".to_string(),
                    label: "Cancel".to_string(),
                    kind: IlhaeInteractiveOptionKind::Cancel,
                },
            ],
        }),
        codex_app_server_protocol::ServerRequest::McpServerElicitationRequest {
            request_id,
            params,
        } => Some(IlhaeInteractiveRequestDto {
            source: "app_server".to_string(),
            thread_id: params.thread_id.clone(),
            turn_id: params
                .turn_id
                .clone()
                .unwrap_or_else(|| "elicitation".to_string()),
            request_id: match request_id {
                RequestId::String(value) => value.clone(),
                RequestId::Integer(value) => value.to_string(),
            },
            title: format!("MCP elicitation: {}", params.server_name),
            reason: Some(match &params.request {
                codex_app_server_protocol::McpServerElicitationRequest::Form {
                    message, ..
                }
                | codex_app_server_protocol::McpServerElicitationRequest::Url { message, .. } => {
                    message.clone()
                }
            }),
            requested_permissions: None,
            options: vec![
                IlhaeInteractiveOptionDto {
                    id: "accept".to_string(),
                    label: "Accept".to_string(),
                    kind: IlhaeInteractiveOptionKind::ApproveOnce,
                },
                IlhaeInteractiveOptionDto {
                    id: "decline".to_string(),
                    label: "Decline".to_string(),
                    kind: IlhaeInteractiveOptionKind::RejectOnce,
                },
                IlhaeInteractiveOptionDto {
                    id: "cancel".to_string(),
                    label: "Cancel".to_string(),
                    kind: IlhaeInteractiveOptionKind::Cancel,
                },
            ],
        }),
        _ => None,
    }
}

pub(super) fn canonical_ilhae_event_from_app_server_event(
    engine_id: &str,
    event: &AppServerEvent,
) -> Option<IlhaeAppSessionEventNotification> {
    let event = match event {
        AppServerEvent::ServerRequest(request) => {
            canonical_interactive_request_from_server_request(request)
                .map(|request| IlhaeAppSessionEventDto::InteractiveRequest { request })
        }
        AppServerEvent::ServerNotification(notification) => match notification {
            ServerNotification::TurnStarted(notif) => Some(IlhaeAppSessionEventDto::TurnStarted {
                thread_id: notif.thread_id.clone(),
                turn_id: notif.turn.id.clone(),
            }),
            ServerNotification::TurnCompleted(notif) => {
                Some(IlhaeAppSessionEventDto::TurnCompleted {
                    thread_id: notif.thread_id.clone(),
                    turn_id: notif.turn.id.clone(),
                    status: turn_status_label(notif.turn.status.clone()).to_string(),
                })
            }
            ServerNotification::AgentMessageDelta(notif) => {
                Some(IlhaeAppSessionEventDto::MessageDelta {
                    thread_id: notif.thread_id.clone(),
                    turn_id: notif.turn_id.clone(),
                    item_id: notif.item_id.clone(),
                    channel: "assistant".to_string(),
                    delta: notif.delta.clone(),
                })
            }
            ServerNotification::ReasoningTextDelta(notif) => {
                Some(IlhaeAppSessionEventDto::MessageDelta {
                    thread_id: notif.thread_id.clone(),
                    turn_id: notif.turn_id.clone(),
                    item_id: notif.item_id.clone(),
                    channel: "reasoning".to_string(),
                    delta: notif.delta.clone(),
                })
            }
            ServerNotification::ItemStarted(notif) => match &notif.item {
                ThreadItem::DynamicToolCall {
                    id,
                    tool,
                    arguments,
                    ..
                } => Some(IlhaeAppSessionEventDto::ToolCallStarted {
                    thread_id: notif.thread_id.clone(),
                    turn_id: notif.turn_id.clone(),
                    call_id: id.clone(),
                    tool: tool.clone(),
                    arguments: arguments.clone(),
                }),
                _ => None,
            },
            ServerNotification::ItemCompleted(notif) => match &notif.item {
                ThreadItem::DynamicToolCall {
                    id,
                    tool,
                    success,
                    content_items,
                    ..
                } => Some(IlhaeAppSessionEventDto::ToolCallCompleted {
                    thread_id: notif.thread_id.clone(),
                    turn_id: notif.turn_id.clone(),
                    call_id: id.clone(),
                    tool: tool.clone(),
                    success: success.unwrap_or(false),
                    output_text: content_items
                        .clone()
                        .unwrap_or_default()
                        .into_iter()
                        .filter_map(|item| {
                            match item {
                            codex_app_server_protocol::DynamicToolCallOutputContentItem::InputText {
                                text,
                            } => Some(text),
                            _ => None,
                        }
                        })
                        .collect(),
                }),
                _ => None,
            },
            _ => None,
        },
        AppServerEvent::Lagged { .. } | AppServerEvent::Disconnected { .. } => None,
    }?;

    Some(IlhaeAppSessionEventNotification {
        engine: engine_id.to_string(),
        event,
    })
}
