use super::*;

const MCP_RESOURCE_READ_TOOL_NAME: &str = "read_mcp_resource";
const MCP_TOOL_THREAD_ID_META_KEY: &str = "threadId";

struct McpResourceReadLifecycle {
    thread_id: ThreadId,
    turn_id: String,
    item_id: String,
    server: String,
    uri: String,
    started_at: Instant,
}

#[derive(Clone)]
pub(crate) struct McpRequestProcessor {
    auth_manager: Arc<AuthManager>,
    thread_manager: Arc<ThreadManager>,
    outgoing: Arc<OutgoingMessageSender>,
    config_manager: ConfigManager,
    thread_state_manager: ThreadStateManager,
}

impl McpRequestProcessor {
    pub(crate) fn new(
        auth_manager: Arc<AuthManager>,
        thread_manager: Arc<ThreadManager>,
        outgoing: Arc<OutgoingMessageSender>,
        config_manager: ConfigManager,
        thread_state_manager: ThreadStateManager,
    ) -> Self {
        Self {
            auth_manager,
            thread_manager,
            outgoing,
            config_manager,
            thread_state_manager,
        }
    }

    pub(crate) async fn mcp_server_oauth_login(
        &self,
        params: McpServerOauthLoginParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.mcp_server_oauth_login_response(params)
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn mcp_server_refresh(
        &self,
        params: Option<()>,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.mcp_server_refresh_response(params)
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn mcp_server_status_list(
        &self,
        request_id: &ConnectionRequestId,
        params: ListMcpServerStatusParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.list_mcp_server_status(request_id, params)
            .await
            .map(|()| None)
    }

    pub(crate) async fn mcp_resource_read(
        &self,
        request_id: &ConnectionRequestId,
        params: McpResourceReadParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.read_mcp_resource(request_id, params)
            .await
            .map(|()| None)
    }

    pub(crate) async fn mcp_server_tool_call(
        &self,
        request_id: &ConnectionRequestId,
        params: McpServerToolCallParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.call_mcp_server_tool(request_id, params)
            .await
            .map(|()| None)
    }

    async fn mcp_server_refresh_response(
        &self,
        _params: Option<()>,
    ) -> Result<McpServerRefreshResponse, JSONRPCErrorError> {
        crate::mcp_refresh::queue_strict_refresh(&self.thread_manager, &self.config_manager)
            .await
            .map_err(|err| internal_error(format!("failed to refresh MCP servers: {err}")))?;
        Ok(McpServerRefreshResponse {})
    }

    async fn load_latest_config(
        &self,
        fallback_cwd: Option<PathBuf>,
    ) -> Result<Config, JSONRPCErrorError> {
        self.config_manager
            .load_latest_config(fallback_cwd)
            .await
            .map_err(|err| internal_error(format!("failed to reload config: {err}")))
    }

    async fn load_thread(
        &self,
        thread_id: &str,
    ) -> Result<(ThreadId, Arc<CodexThread>), JSONRPCErrorError> {
        let thread_id = ThreadId::from_string(thread_id)
            .map_err(|err| invalid_request(format!("invalid thread id: {err}")))?;

        let thread = self
            .thread_manager
            .get_thread(thread_id)
            .await
            .map_err(|_| invalid_request(format!("thread not found: {thread_id}")))?;

        Ok((thread_id, thread))
    }

    async fn mcp_server_oauth_login_response(
        &self,
        params: McpServerOauthLoginParams,
    ) -> Result<McpServerOauthLoginResponse, JSONRPCErrorError> {
        let config = self.load_latest_config(/*fallback_cwd*/ None).await?;
        let McpServerOauthLoginParams {
            name,
            scopes,
            timeout_secs,
        } = params;

        let configured_servers = self
            .thread_manager
            .mcp_manager()
            .configured_servers(&config)
            .await;
        let Some(server) = configured_servers.get(&name) else {
            return Err(invalid_request(format!(
                "No MCP server named '{name}' found."
            )));
        };

        let (url, http_headers, env_http_headers) = match &server.transport {
            McpServerTransportConfig::StreamableHttp {
                url,
                http_headers,
                env_http_headers,
                ..
            } => (url.clone(), http_headers.clone(), env_http_headers.clone()),
            _ => {
                return Err(invalid_request(
                    "OAuth login is only supported for streamable HTTP servers.",
                ));
            }
        };

        let discovered_scopes = if scopes.is_none() && server.scopes.is_none() {
            discover_supported_scopes(&server.transport).await
        } else {
            None
        };
        let resolved_scopes =
            resolve_oauth_scopes(scopes, server.scopes.clone(), discovered_scopes);

        let handle = perform_oauth_login_return_url(
            &name,
            &url,
            config.mcp_oauth_credentials_store_mode,
            http_headers,
            env_http_headers,
            &resolved_scopes.scopes,
            server.oauth_client_id(),
            server.oauth_resource.as_deref(),
            timeout_secs,
            config.mcp_oauth_callback_port,
            config.mcp_oauth_callback_url.as_deref(),
        )
        .await
        .map_err(|err| internal_error(format!("failed to login to MCP server '{name}': {err}")))?;
        let authorization_url = handle.authorization_url().to_string();
        let notification_name = name.clone();
        let outgoing = Arc::clone(&self.outgoing);

        tokio::spawn(async move {
            let (success, error) = match handle.wait().await {
                Ok(()) => (true, None),
                Err(err) => (false, Some(err.to_string())),
            };

            let notification = ServerNotification::McpServerOauthLoginCompleted(
                McpServerOauthLoginCompletedNotification {
                    name: notification_name,
                    success,
                    error,
                },
            );
            outgoing.send_server_notification(notification).await;
        });

        Ok(McpServerOauthLoginResponse { authorization_url })
    }

    async fn list_mcp_server_status(
        &self,
        request_id: &ConnectionRequestId,
        params: ListMcpServerStatusParams,
    ) -> Result<(), JSONRPCErrorError> {
        let request = request_id.clone();

        let outgoing = Arc::clone(&self.outgoing);
        let config = match params.thread_id.as_deref() {
            Some(thread_id) => {
                let (_, thread) = self.load_thread(thread_id).await?;
                let thread_config = thread.config().await;
                self.config_manager
                    .load_latest_config_for_thread(thread_config.as_ref())
                    .await
                    .map_err(|err| internal_error(format!("failed to reload config: {err}")))?
            }
            None => self.load_latest_config(/*fallback_cwd*/ None).await?,
        };
        let mcp_config = config
            .to_mcp_config(self.thread_manager.plugins_manager().as_ref())
            .await;
        let auth = self.auth_manager.auth().await;
        let environment_manager = self.thread_manager.environment_manager();
        // This status path has no turn-selected environment. Use config cwd
        // as the local stdio fallback; named environment stdio MCPs must
        // declare their own absolute cwd.
        let runtime_context =
            McpRuntimeContext::new(Arc::clone(&environment_manager), config.cwd.to_path_buf());

        tokio::spawn(async move {
            Self::list_mcp_server_status_task(
                outgoing,
                request,
                params,
                mcp_config,
                auth,
                runtime_context,
            )
            .await;
        });
        Ok(())
    }

    async fn list_mcp_server_status_task(
        outgoing: Arc<OutgoingMessageSender>,
        request_id: ConnectionRequestId,
        params: ListMcpServerStatusParams,
        mcp_config: codex_mcp::McpConfig,
        auth: Option<CodexAuth>,
        runtime_context: McpRuntimeContext,
    ) {
        let result = Self::list_mcp_server_status_response(
            request_id.request_id.to_string(),
            params,
            mcp_config,
            auth,
            runtime_context,
        )
        .await;
        outgoing.send_result(request_id, result).await;
    }

    async fn list_mcp_server_status_response(
        request_id: String,
        params: ListMcpServerStatusParams,
        mcp_config: codex_mcp::McpConfig,
        auth: Option<CodexAuth>,
        runtime_context: McpRuntimeContext,
    ) -> Result<ListMcpServerStatusResponse, JSONRPCErrorError> {
        let detail = match params.detail.unwrap_or(McpServerStatusDetail::Full) {
            McpServerStatusDetail::Full => McpSnapshotDetail::Full,
            McpServerStatusDetail::ToolsAndAuthOnly => McpSnapshotDetail::ToolsAndAuthOnly,
        };

        let snapshot = collect_mcp_server_status_snapshot_with_detail(
            &mcp_config,
            auth.as_ref(),
            request_id,
            runtime_context,
            detail,
        )
        .await;

        let McpServerStatusSnapshot {
            server_infos,
            tools_by_server,
            resources,
            resource_templates,
            auth_statuses,
            mut server_names,
        } = snapshot;
        server_names.extend(
            auth_statuses
                .keys()
                .cloned()
                .chain(resources.keys().cloned())
                .chain(resource_templates.keys().cloned()),
        );
        server_names.sort();
        server_names.dedup();

        let total = server_names.len();
        let limit = params.limit.unwrap_or(total as u32).max(1) as usize;
        let effective_limit = limit.min(total);
        let start = match params.cursor {
            Some(cursor) => match cursor.parse::<usize>() {
                Ok(idx) => idx,
                Err(_) => return Err(invalid_request(format!("invalid cursor: {cursor}"))),
            },
            None => 0,
        };

        if start > total {
            return Err(invalid_request(format!(
                "cursor {start} exceeds total MCP servers {total}"
            )));
        }

        let end = start.saturating_add(effective_limit).min(total);

        let data: Vec<McpServerStatus> = server_names[start..end]
            .iter()
            .map(|name| McpServerStatus {
                name: name.clone(),
                server_info: server_infos.get(name).cloned(),
                tools: tools_by_server.get(name).cloned().unwrap_or_default(),
                resources: resources.get(name).cloned().unwrap_or_default(),
                resource_templates: resource_templates.get(name).cloned().unwrap_or_default(),
                auth_status: auth_statuses
                    .get(name)
                    .cloned()
                    .unwrap_or(CoreMcpAuthStatus::Unsupported)
                    .into(),
            })
            .collect();

        let next_cursor = if end < total {
            Some(end.to_string())
        } else {
            None
        };

        Ok(ListMcpServerStatusResponse { data, next_cursor })
    }

    async fn read_mcp_resource(
        &self,
        request_id: &ConnectionRequestId,
        params: McpResourceReadParams,
    ) -> Result<(), JSONRPCErrorError> {
        let outgoing = Arc::clone(&self.outgoing);
        let McpResourceReadParams {
            thread_id,
            server,
            uri,
        } = params;

        if let Some(thread_id) = thread_id {
            let (thread_uuid, thread) = self.load_thread(&thread_id).await?;
            let request_id = request_id.clone();
            let thread_state_manager = self.thread_state_manager.clone();

            tokio::spawn(async move {
                let lifecycle = Self::emit_mcp_resource_read_started(
                    Arc::clone(&outgoing),
                    &thread_state_manager,
                    thread_uuid,
                    server.clone(),
                    uri.clone(),
                )
                .await;
                let result = thread.read_mcp_resource(&server, &uri).await;
                Self::emit_mcp_resource_read_completed(
                    Arc::clone(&outgoing),
                    &thread_state_manager,
                    lifecycle,
                    &result,
                )
                .await;
                Self::send_mcp_resource_read_response(outgoing, request_id, result).await;
            });
            return Ok(());
        }

        let config = self.load_latest_config(/*fallback_cwd*/ None).await?;
        let mcp_config = config
            .to_mcp_config(self.thread_manager.plugins_manager().as_ref())
            .await;
        let auth = self.auth_manager.auth().await;
        let environment_manager = self.thread_manager.environment_manager();
        // This threadless resource-read path has no turn cwd or turn-selected
        // environment. Use config cwd only as the local stdio fallback; named
        // environment stdio MCPs must declare their own absolute cwd.
        let runtime_context =
            McpRuntimeContext::new(Arc::clone(&environment_manager), config.cwd.to_path_buf());
        let request_id = request_id.clone();

        tokio::spawn(async move {
            let result = read_mcp_resource_without_thread(
                &mcp_config,
                auth.as_ref(),
                runtime_context,
                &server,
                &uri,
            )
            .await
            .and_then(|result| serde_json::to_value(result).map_err(anyhow::Error::from));
            Self::send_mcp_resource_read_response(outgoing, request_id, result).await;
        });
        Ok(())
    }

    async fn emit_mcp_resource_read_started(
        outgoing: Arc<OutgoingMessageSender>,
        thread_state_manager: &ThreadStateManager,
        thread_id: ThreadId,
        server: String,
        uri: String,
    ) -> McpResourceReadLifecycle {
        let item_id = format!("mcp-resource-read-{}", Uuid::now_v7());
        let turn_id = {
            let thread_state = thread_state_manager.thread_state(thread_id).await;
            let state = thread_state.lock().await;
            state
                .active_turn_snapshot()
                .map(|turn| turn.id)
                .or_else(|| state.last_terminal_turn_id.clone())
                .unwrap_or_else(|| item_id.clone())
        };
        let lifecycle = McpResourceReadLifecycle {
            thread_id,
            turn_id,
            item_id,
            server,
            uri,
            started_at: Instant::now(),
        };
        let item = ThreadItem::McpToolCall {
            id: lifecycle.item_id.clone(),
            server: lifecycle.server.clone(),
            tool: MCP_RESOURCE_READ_TOOL_NAME.to_string(),
            status: McpToolCallStatus::InProgress,
            arguments: mcp_resource_read_arguments(&lifecycle.uri),
            mcp_app_resource_uri: None,
            plugin_id: None,
            result: None,
            error: None,
            duration_ms: None,
        };
        let subscribed_connection_ids = thread_state_manager
            .subscribed_connection_ids(lifecycle.thread_id)
            .await;
        let outgoing = ThreadScopedOutgoingMessageSender::new(
            outgoing,
            subscribed_connection_ids,
            lifecycle.thread_id,
        );
        outgoing
            .send_server_notification(ServerNotification::ItemStarted(ItemStartedNotification {
                thread_id: lifecycle.thread_id.to_string(),
                turn_id: lifecycle.turn_id.clone(),
                started_at_ms: now_unix_timestamp_ms(),
                item,
            }))
            .await;
        lifecycle
    }

    async fn emit_mcp_resource_read_completed(
        outgoing: Arc<OutgoingMessageSender>,
        thread_state_manager: &ThreadStateManager,
        lifecycle: McpResourceReadLifecycle,
        result: &anyhow::Result<serde_json::Value>,
    ) {
        let duration_ms = i64::try_from(lifecycle.started_at.elapsed().as_millis()).ok();
        let (status, result, error) = match result {
            Ok(value) => match serde_json::from_value::<McpResourceReadResponse>(value.clone()) {
                Ok(response) => (
                    McpToolCallStatus::Completed,
                    Some(Box::new(mcp_resource_read_tool_result(response))),
                    None,
                ),
                Err(error) => (
                    McpToolCallStatus::Failed,
                    None,
                    Some(McpToolCallError {
                        message: format!(
                            "failed to deserialize MCP resource read response: {error}"
                        ),
                    }),
                ),
            },
            Err(error) => (
                McpToolCallStatus::Failed,
                None,
                Some(McpToolCallError {
                    message: format!("{error:#}"),
                }),
            ),
        };
        let item = ThreadItem::McpToolCall {
            id: lifecycle.item_id,
            server: lifecycle.server,
            tool: MCP_RESOURCE_READ_TOOL_NAME.to_string(),
            status,
            arguments: mcp_resource_read_arguments(&lifecycle.uri),
            mcp_app_resource_uri: None,
            plugin_id: None,
            result,
            error,
            duration_ms,
        };
        let subscribed_connection_ids = thread_state_manager
            .subscribed_connection_ids(lifecycle.thread_id)
            .await;
        let outgoing = ThreadScopedOutgoingMessageSender::new(
            outgoing,
            subscribed_connection_ids,
            lifecycle.thread_id,
        );
        outgoing
            .send_server_notification(ServerNotification::ItemCompleted(
                ItemCompletedNotification {
                    thread_id: lifecycle.thread_id.to_string(),
                    turn_id: lifecycle.turn_id,
                    completed_at_ms: now_unix_timestamp_ms(),
                    item,
                },
            ))
            .await;
    }

    async fn send_mcp_resource_read_response(
        outgoing: Arc<OutgoingMessageSender>,
        request_id: ConnectionRequestId,
        result: anyhow::Result<serde_json::Value>,
    ) {
        let result = result
            .map_err(|error| internal_error(format!("{error:#}")))
            .and_then(|result| {
                serde_json::from_value::<McpResourceReadResponse>(result).map_err(|error| {
                    internal_error(format!(
                        "failed to deserialize MCP resource read response: {error}"
                    ))
                })
            });
        outgoing.send_result(request_id, result).await;
    }

    async fn call_mcp_server_tool(
        &self,
        request_id: &ConnectionRequestId,
        params: McpServerToolCallParams,
    ) -> Result<(), JSONRPCErrorError> {
        let outgoing = Arc::clone(&self.outgoing);
        let thread_id = params.thread_id.clone();
        let (_, thread) = self.load_thread(&thread_id).await?;
        let meta = with_mcp_tool_call_thread_id_meta(params.meta, &thread_id);
        let request_id = request_id.clone();

        tokio::spawn(async move {
            let result = thread
                .call_mcp_tool(&params.server, &params.tool, params.arguments, meta)
                .await
                .map(McpServerToolCallResponse::from)
                .map_err(|error| internal_error(format!("{error:#}")));
            outgoing.send_result(request_id, result).await;
        });
        Ok(())
    }
}

fn now_unix_timestamp_ms() -> i64 {
    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(duration) => i64::try_from(duration.as_millis()).unwrap_or(i64::MAX),
        Err(_) => 0,
    }
}

fn mcp_resource_read_arguments(uri: &str) -> serde_json::Value {
    serde_json::json!({ "uri": uri })
}

fn mcp_resource_read_tool_result(response: McpResourceReadResponse) -> McpToolCallResult {
    let summary = format_mcp_resource_read_summary(&response);
    McpToolCallResult {
        content: vec![serde_json::json!({
            "type": "text",
            "text": summary,
        })],
        structured_content: serde_json::to_value(response).ok(),
        meta: None,
    }
}

fn format_mcp_resource_read_summary(response: &McpResourceReadResponse) -> String {
    let count = response.contents.len();
    let uris = response
        .contents
        .iter()
        .map(|content| match content {
            codex_protocol::mcp::ResourceContent::Text { uri, .. }
            | codex_protocol::mcp::ResourceContent::Blob { uri, .. } => uri.as_str(),
        })
        .collect::<Vec<_>>()
        .join(", ");
    if uris.is_empty() {
        format!("Read {count} resource content block(s).")
    } else {
        format!("Read {count} resource content block(s): {uris}")
    }
}

fn with_mcp_tool_call_thread_id_meta(
    meta: Option<serde_json::Value>,
    thread_id: &str,
) -> Option<serde_json::Value> {
    match meta {
        Some(serde_json::Value::Object(mut map)) => {
            map.insert(
                MCP_TOOL_THREAD_ID_META_KEY.to_string(),
                serde_json::Value::String(thread_id.to_string()),
            );
            Some(serde_json::Value::Object(map))
        }
        None => {
            let mut map = serde_json::Map::new();
            map.insert(
                MCP_TOOL_THREAD_ID_META_KEY.to_string(),
                serde_json::Value::String(thread_id.to_string()),
            );
            Some(serde_json::Value::Object(map))
        }
        other => other,
    }
}
