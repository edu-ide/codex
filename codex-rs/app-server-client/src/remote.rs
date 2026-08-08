/*
This module implements the websocket-backed app-server client transport.

It owns the remote connection lifecycle, including the initialize/initialized
handshake, JSON-RPC request/response routing, server-request resolution, and
notification streaming. The rest of the crate uses the same `AppServerEvent`
surface for both in-process and remote transports, so callers such as the TUI
can switch between them without changing their higher-level session logic.
*/

use std::collections::HashMap;
use std::collections::VecDeque;
use std::io::Error as IoError;
use std::io::ErrorKind;
use std::io::Result as IoResult;
use std::time::Duration;

use crate::AppServerEvent;
use crate::RequestResult;
use crate::SHUTDOWN_TIMEOUT;
use crate::TypedRequestError;
use crate::request_method_name;
use codex_app_server_protocol::ClientInfo;
use codex_app_server_protocol::ClientNotification;
use codex_app_server_protocol::ClientRequest;
use codex_app_server_protocol::InitializeCapabilities;
use codex_app_server_protocol::InitializeParams;
use codex_app_server_protocol::InitializeResponse;
use codex_app_server_protocol::JSONRPCError;
use codex_app_server_protocol::JSONRPCErrorError;
use codex_app_server_protocol::JSONRPCMessage;
use codex_app_server_protocol::JSONRPCNotification;
use codex_app_server_protocol::JSONRPCRequest;
use codex_app_server_protocol::JSONRPCResponse;
use codex_app_server_protocol::RequestId;
use codex_app_server_protocol::Result as JsonRpcResult;
use codex_app_server_protocol::ServerNotification;
use codex_app_server_protocol::ServerRequest;
use codex_utils_absolute_path::{AbsolutePathBuf, AbsolutePathBufGuard};
use codex_utils_rustls_provider::ensure_rustls_crypto_provider;
use futures::SinkExt;
use futures::StreamExt;
use serde::de::DeserializeOwned;
use tokio::io::{AsyncRead, AsyncWrite};
#[cfg(unix)]
use tokio::net::UnixStream;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::time::timeout;
use tokio_tungstenite::client_async;
use tokio_tungstenite::MaybeTlsStream;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::http::header::AUTHORIZATION;
use tracing::warn;
use url::Url;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const INITIALIZE_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteAppServerEndpoint {
    WebSocket {
        websocket_url: String,
        auth_token: Option<String>,
    },
    UnixSocket {
        socket_path: AbsolutePathBuf,
    },
}

fn remote_response_base_path() -> &'static std::path::Path {
    #[cfg(windows)]
    {
        std::path::Path::new(r"C:\")
    }
    #[cfg(not(windows))]
    {
        std::path::Path::new("/")
    }
}

fn decode_remote_response<T>(
    result: serde_json::Value,
    method: String,
) -> Result<T, TypedRequestError>
where
    T: DeserializeOwned,
{
    let _guard = AbsolutePathBufGuard::new(remote_response_base_path());
    serde_json::from_value(result)
        .map_err(|source| TypedRequestError::Deserialize { method, source })
}

#[derive(Debug, Clone)]
pub struct RemoteAppServerConnectArgs {
    pub endpoint: RemoteAppServerEndpoint,
    pub client_name: String,
    pub client_version: String,
    pub experimental_api: bool,
    pub mcp_server_openai_form_elicitation: bool,
    pub opt_out_notification_methods: Vec<String>,
    pub channel_capacity: usize,
}

impl RemoteAppServerConnectArgs {
    fn initialize_params(&self) -> InitializeParams {
        let capabilities = InitializeCapabilities {
            experimental_api: self.experimental_api,
            request_attestation: false,
            mcp_server_openai_form_elicitation: self.mcp_server_openai_form_elicitation,
            opt_out_notification_methods: if self.opt_out_notification_methods.is_empty() {
                None
            } else {
                Some(self.opt_out_notification_methods.clone())
            },
        };

        InitializeParams {
            client_info: ClientInfo {
                name: self.client_name.clone(),
                title: None,
                version: self.client_version.clone(),
            },
            capabilities: Some(capabilities),
        }
    }
}

pub(crate) fn websocket_url_supports_auth_token(url: &Url) -> bool {
    match (url.scheme(), url.host()) {
        ("wss", Some(_)) => true,
        ("ws", Some(url::Host::Domain(domain))) => domain.eq_ignore_ascii_case("localhost"),
        ("ws", Some(url::Host::Ipv4(addr))) => addr.is_loopback(),
        ("ws", Some(url::Host::Ipv6(addr))) => addr.is_loopback(),
        _ => false,
    }
}

fn websocket_request(
    websocket_url: &str,
    auth_token: Option<&str>,
) -> IoResult<tokio_tungstenite::tungstenite::http::Request<()>> {
    let url = Url::parse(websocket_url).map_err(|err| {
        IoError::new(
            ErrorKind::InvalidInput,
            format!("invalid websocket URL `{websocket_url}`: {err}"),
        )
    })?;
    if auth_token.is_some() && !websocket_url_supports_auth_token(&url) {
        return Err(IoError::new(
            ErrorKind::InvalidInput,
            format!(
                "remote auth tokens require `wss://` or loopback `ws://` URLs; got `{websocket_url}`"
            ),
        ));
    }
    let mut request: tokio_tungstenite::tungstenite::http::Request<()> = url
        .as_str()
        .into_client_request()
        .map_err(|err| {
        IoError::new(
            ErrorKind::InvalidInput,
            format!("invalid websocket URL `{websocket_url}`: {err}"),
        )
    })?;
    if let Some(auth_token) = auth_token {
        let header_value = HeaderValue::from_str(&format!("Bearer {auth_token}")).map_err(|err| {
            IoError::new(
                ErrorKind::InvalidInput,
                format!("invalid remote authorization header value: {err}"),
            )
        })?;
        request.headers_mut().insert(AUTHORIZATION, header_value);
    }
    Ok(request)
}

enum RemoteClientCommand {
    Request {
        request: Box<ClientRequest>,
        response_tx: oneshot::Sender<IoResult<RequestResult>>,
    },
    Notify {
        notification: ClientNotification,
        response_tx: oneshot::Sender<IoResult<()>>,
    },
    ResolveServerRequest {
        request_id: RequestId,
        result: JsonRpcResult,
        response_tx: oneshot::Sender<IoResult<()>>,
    },
    RejectServerRequest {
        request_id: RequestId,
        error: JSONRPCErrorError,
        response_tx: oneshot::Sender<IoResult<()>>,
    },
    Shutdown {
        response_tx: oneshot::Sender<IoResult<()>>,
    },
}

pub struct RemoteAppServerClient {
    command_tx: mpsc::Sender<RemoteClientCommand>,
    event_rx: mpsc::UnboundedReceiver<AppServerEvent>,
    pending_events: VecDeque<AppServerEvent>,
    worker_handle: tokio::task::JoinHandle<()>,
    server_version: Option<String>,
    codex_home: Option<String>,
}

struct RemoteInitializeHandshake {
    pending_events: Vec<AppServerEvent>,
    server_version: Option<String>,
    codex_home: Option<String>,
}

#[derive(Clone)]
pub struct RemoteAppServerRequestHandle {
    command_tx: mpsc::Sender<RemoteClientCommand>,
}

impl RemoteAppServerClient {
    pub async fn connect(args: RemoteAppServerConnectArgs) -> IoResult<Self> {
        let initialize_params = args.initialize_params();
        let channel_capacity = args.channel_capacity.max(1);
        match args.endpoint {
            RemoteAppServerEndpoint::WebSocket {
                websocket_url,
                auth_token,
            } => {
                let mut request = websocket_request(&websocket_url, auth_token.as_deref())?;
                ensure_rustls_crypto_provider();
                let stream = timeout(CONNECT_TIMEOUT, connect_async(request))
                    .await
                    .map_err(|_| {
                        IoError::new(
                            ErrorKind::TimedOut,
                            format!(
                                "timed out connecting to remote app server at `{websocket_url}`"
                            ),
                        )
                    })?
                    .map(|(stream, _response)| stream)
                    .map_err(|err| {
                        IoError::other(format!(
                            "failed to connect to remote app server at `{websocket_url}`: {err}"
                        ))
                    })?;
                let mut stream = stream;
                let handshake = initialize_remote_connection(
                    &mut stream,
                    &websocket_url,
                    initialize_params,
                    INITIALIZE_TIMEOUT,
                )
                .await?;
                Self::start_stream(
                    stream,
                    websocket_url,
                    channel_capacity,
                    handshake,
                )
                .await
            }
            #[allow(unreachable_patterns)]
            RemoteAppServerEndpoint::UnixSocket { socket_path } => {
                #[cfg(not(unix))]
                {
                    return Err(IoError::new(
                        ErrorKind::Unsupported,
                        format!(
                            "unix socket remote endpoint is not supported on this platform: {}",
                            socket_path.display()
                        ),
                    ));
                }

                #[cfg(unix)]
                {
                    let websocket_url = format!("unix://{}", socket_path.display());
                    let stream = timeout(CONNECT_TIMEOUT, UnixStream::connect(socket_path.as_path()))
                        .await
                        .map_err(|_| {
                            IoError::new(
                                ErrorKind::TimedOut,
                                format!(
                                    "timed out connecting to remote app server unix socket at `{}`",
                                    socket_path.display()
                                ),
                            )
                        })?
                        .map_err(|err| {
                            IoError::other(format!(
                                "failed to connect to remote app server unix socket at `{}`: {err}",
                                socket_path.display()
                            ))
                        })?;
                    let request = websocket_request("ws://localhost/rpc", None)?;
                    let (mut stream, _response) = timeout(
                        CONNECT_TIMEOUT,
                        client_async(request, stream),
                    )
                    .await
                    .map_err(|_| {
                        IoError::new(
                            ErrorKind::TimedOut,
                            "timed out upgrading unix socket to websocket",
                        )
                    })?
                    .map_err(|err| {
                        IoError::other(format!(
                            "failed to upgrade unix socket at `{}` to websocket: {err}",
                            socket_path.display()
                        ))
                    })?;
                    let handshake = initialize_remote_connection(
                        &mut stream,
                        &websocket_url,
                        initialize_params,
                        INITIALIZE_TIMEOUT,
                    )
                    .await?;
                    Self::start_stream(
                        stream,
                        websocket_url,
                        channel_capacity,
                        handshake,
                    )
                    .await
                }
            }
        }
    }

    async fn start_stream<S>(
        stream: WebSocketStream<S>,
        websocket_url: String,
        channel_capacity: usize,
        handshake: RemoteInitializeHandshake,
    ) -> IoResult<Self>
    where
        S: AsyncRead + AsyncWrite + Send + Unpin + 'static,
    {
        let mut stream = stream;
        let (command_tx, mut command_rx) = mpsc::channel::<RemoteClientCommand>(channel_capacity);
        let (event_tx, event_rx) = mpsc::unbounded_channel::<AppServerEvent>();

        let RemoteInitializeHandshake {
            pending_events,
            server_version,
            codex_home,
        } = handshake;

        let worker_handle = tokio::spawn(async move {
            let mut pending_requests =
                HashMap::<RequestId, oneshot::Sender<IoResult<RequestResult>>>::new();
            loop {
                tokio::select! {
                    command = command_rx.recv() => {
                        let Some(command) = command else {
                            let _ = stream.close(None).await;
                            break;
                        };
                        match command {
                            RemoteClientCommand::Request { request, response_tx } => {
                                let request_id = request_id_from_client_request(&request);
                                if pending_requests.contains_key(&request_id) {
                                    let _ = response_tx.send(Err(IoError::new(
                                        ErrorKind::InvalidInput,
                                        format!("duplicate remote app-server request id `{request_id}`"),
                                    )));
                                    continue;
                                }
                                pending_requests.insert(request_id.clone(), response_tx);
                                if let Err(err) = write_jsonrpc_message(
                                    &mut stream,
                                    JSONRPCMessage::Request(jsonrpc_request_from_client_request(*request)),
                                    &websocket_url,
                                )
                                .await
                                {
                                    let err_message = err.to_string();
                                    if let Some(response_tx) = pending_requests.remove(&request_id) {
                                        let _ = response_tx.send(Err(err));
                                    }
                                    let _ = deliver_event(
                                        &event_tx,
                                        AppServerEvent::Disconnected {
                                            message: format!(
                                                "remote app server at `{websocket_url}` write failed: {err_message}"
                                            ),
                                        },
                                    );
                                    break;
                                }
                            }
                            RemoteClientCommand::Notify { notification, response_tx } => {
                                let result = write_jsonrpc_message(
                                    &mut stream,
                                    JSONRPCMessage::Notification(
                                        jsonrpc_notification_from_client_notification(notification),
                                    ),
                                    &websocket_url,
                                )
                                .await;
                                let _ = response_tx.send(result);
                            }
                            RemoteClientCommand::ResolveServerRequest {
                                request_id,
                                result,
                                response_tx,
                            } => {
                                let result = write_jsonrpc_message(
                                    &mut stream,
                                    JSONRPCMessage::Response(JSONRPCResponse {
                                        id: request_id,
                                        result,
                                    }),
                                    &websocket_url,
                                )
                                .await;
                                let _ = response_tx.send(result);
                            }
                            RemoteClientCommand::RejectServerRequest {
                                request_id,
                                error,
                                response_tx,
                            } => {
                                let result = write_jsonrpc_message(
                                    &mut stream,
                                    JSONRPCMessage::Error(JSONRPCError {
                                        error,
                                        id: request_id,
                                    }),
                                    &websocket_url,
                                )
                                .await;
                                let _ = response_tx.send(result);
                            }
                            RemoteClientCommand::Shutdown { response_tx } => {
                                let close_result = stream.close(None).await.map_err(|err| {
                                    IoError::other(format!(
                                        "failed to close websocket app server `{websocket_url}`: {err}"
                                    ))
                                });
                                let _ = response_tx.send(close_result);
                                break;
                            }
                        }
                    }
                    message = stream.next() => {
                        match message {
                            Some(Ok(Message::Text(text))) => {
                                match serde_json::from_str::<JSONRPCMessage>(&text) {
                                    Ok(JSONRPCMessage::Response(response)) => {
                                        if let Some(response_tx) = pending_requests.remove(&response.id) {
                                            let _ = response_tx.send(Ok(Ok(response.result)));
                                        }
                                    }
                                    Ok(JSONRPCMessage::Error(error)) => {
                                        if let Some(response_tx) = pending_requests.remove(&error.id) {
                                            let _ = response_tx.send(Ok(Err(error.error)));
                                        }
                                    }
                                    Ok(JSONRPCMessage::Notification(notification)) => {
                                        if let Some(event) =
                                            app_server_event_from_notification(notification)
                                            && let Err(err) = deliver_event(
                                                &event_tx,
                                                event,
                                            )
                                            {
                                                warn!(%err, "failed to deliver remote app-server event");
                                                break;
                                            }
                                    }
                                    Ok(JSONRPCMessage::Request(request)) => {
                                        let request_id = request.id.clone();
                                        let method = request.method.clone();
                                        match ServerRequest::try_from(request) {
                                            Ok(request) => {
                                                if let Err(err) = deliver_event(
                                                    &event_tx,
                                                    AppServerEvent::ServerRequest(request),
                                                )
                                                {
                                                    warn!(%err, "failed to deliver remote app-server server request");
                                                    break;
                                                }
                                            }
                                            Err(err) => {
                                                warn!(%err, method, "rejecting unknown remote app-server request");
                                                if let Err(reject_err) = write_jsonrpc_message(
                                                    &mut stream,
                                                    JSONRPCMessage::Error(JSONRPCError {
                                                        error: JSONRPCErrorError {
                                                            code: -32601,
                                                            message: format!(
                                                                "unsupported remote app-server request `{method}`"
                                                            ),
                                                            data: None,
                                                        },
                                                        id: request_id,
                                                    }),
                                                    &websocket_url,
                                                )
                                                .await
                                                {
                                                    let err_message = reject_err.to_string();
                                                    let _ = deliver_event(
                                                        &event_tx,
                                                        AppServerEvent::Disconnected {
                                                            message: format!(
                                                                "remote app server at `{websocket_url}` write failed: {err_message}"
                                                            ),
                                                        },
                                                    );
                                                    break;
                                                }
                                            }
                                        }
                                    }
                                    Err(err) => {
                                        let _ = deliver_event(
                                            &event_tx,
                                            AppServerEvent::Disconnected {
                                                message: format!(
                                                    "remote app server at `{websocket_url}` sent invalid JSON-RPC: {err}"
                                                ),
                                            },
                                        );
                                        break;
                                    }
                                }
                            }
                            Some(Ok(Message::Close(frame))) => {
                                let reason = frame
                                    .as_ref()
                                    .map(|frame| frame.reason.to_string())
                                    .filter(|reason| !reason.is_empty())
                                    .unwrap_or_else(|| "connection closed".to_string());
                                let _ = deliver_event(
                                    &event_tx,
                                    AppServerEvent::Disconnected {
                                        message: format!(
                                            "remote app server at `{websocket_url}` disconnected: {reason}"
                                        ),
                                    },
                                );
                                break;
                            }
                            Some(Ok(Message::Binary(_)))
                            | Some(Ok(Message::Ping(_)))
                            | Some(Ok(Message::Pong(_)))
                            | Some(Ok(Message::Frame(_))) => {}
                            Some(Err(err)) => {
                                let _ = deliver_event(
                                    &event_tx,
                                    AppServerEvent::Disconnected {
                                        message: format!(
                                            "remote app server at `{websocket_url}` transport failed: {err}"
                                        ),
                                    },
                                );
                                break;
                            }
                            None => {
                                let _ = deliver_event(
                                    &event_tx,
                                    AppServerEvent::Disconnected {
                                        message: format!(
                                            "remote app server at `{websocket_url}` closed the connection"
                                        ),
                                    },
                                );
                                break;
                            }
                        }
                    }
                }
            }

            let err = IoError::new(
                ErrorKind::BrokenPipe,
                "remote app-server worker channel is closed",
            );
            for (_, response_tx) in pending_requests {
                let _ = response_tx.send(Err(IoError::new(err.kind(), err.to_string())));
            }
        });

        Ok(Self {
            command_tx,
            event_rx,
            pending_events: pending_events.into(),
            worker_handle,
            server_version,
            codex_home,
        })
    }

    pub fn server_version(&self) -> Option<&str> {
        self.server_version.as_deref()
    }

    pub fn codex_home(&self) -> Option<&str> {
        self.codex_home.as_deref()
    }

    pub fn request_handle(&self) -> RemoteAppServerRequestHandle {
        RemoteAppServerRequestHandle {
            command_tx: self.command_tx.clone(),
        }
    }

    pub async fn request(&self, request: ClientRequest) -> IoResult<RequestResult> {
        let (response_tx, response_rx) = oneshot::channel();
        self.command_tx
            .send(RemoteClientCommand::Request {
                request: Box::new(request),
                response_tx,
            })
            .await
            .map_err(|_| {
                IoError::new(
                    ErrorKind::BrokenPipe,
                    "remote app-server worker channel is closed",
                )
            })?;
        response_rx.await.map_err(|_| {
            IoError::new(
                ErrorKind::BrokenPipe,
                "remote app-server request channel is closed",
            )
        })?
    }

    pub async fn request_typed<T>(&self, request: ClientRequest) -> Result<T, TypedRequestError>
    where
        T: DeserializeOwned,
    {
        let method = request_method_name(&request);
        let response =
            self.request(request)
                .await
                .map_err(|source| TypedRequestError::Transport {
                    method: method.clone(),
                    source,
                })?;
        let result = response.map_err(|source| TypedRequestError::Server {
            method: method.clone(),
            source,
        })?;
        decode_remote_response(result, method)
    }

    pub async fn notify(&self, notification: ClientNotification) -> IoResult<()> {
        let (response_tx, response_rx) = oneshot::channel();
        self.command_tx
            .send(RemoteClientCommand::Notify {
                notification,
                response_tx,
            })
            .await
            .map_err(|_| {
                IoError::new(
                    ErrorKind::BrokenPipe,
                    "remote app-server worker channel is closed",
                )
            })?;
        response_rx.await.map_err(|_| {
            IoError::new(
                ErrorKind::BrokenPipe,
                "remote app-server notify channel is closed",
            )
        })?
    }

    pub async fn resolve_server_request(
        &self,
        request_id: RequestId,
        result: JsonRpcResult,
    ) -> IoResult<()> {
        let (response_tx, response_rx) = oneshot::channel();
        self.command_tx
            .send(RemoteClientCommand::ResolveServerRequest {
                request_id,
                result,
                response_tx,
            })
            .await
            .map_err(|_| {
                IoError::new(
                    ErrorKind::BrokenPipe,
                    "remote app-server worker channel is closed",
                )
            })?;
        response_rx.await.map_err(|_| {
            IoError::new(
                ErrorKind::BrokenPipe,
                "remote app-server resolve channel is closed",
            )
        })?
    }

    pub async fn reject_server_request(
        &self,
        request_id: RequestId,
        error: JSONRPCErrorError,
    ) -> IoResult<()> {
        let (response_tx, response_rx) = oneshot::channel();
        self.command_tx
            .send(RemoteClientCommand::RejectServerRequest {
                request_id,
                error,
                response_tx,
            })
            .await
            .map_err(|_| {
                IoError::new(
                    ErrorKind::BrokenPipe,
                    "remote app-server worker channel is closed",
                )
            })?;
        response_rx.await.map_err(|_| {
            IoError::new(
                ErrorKind::BrokenPipe,
                "remote app-server reject channel is closed",
            )
        })?
    }

    pub async fn next_event(&mut self) -> Option<AppServerEvent> {
        if let Some(event) = self.pending_events.pop_front() {
            return Some(event);
        }
        self.event_rx.recv().await
    }

    pub async fn shutdown(self) -> IoResult<()> {
        let Self {
            command_tx,
            event_rx,
            pending_events: _pending_events,
            server_version: _server_version,
            codex_home: _codex_home,
            worker_handle,
        } = self;
        let mut worker_handle = worker_handle;
        drop(event_rx);
        let (response_tx, response_rx) = oneshot::channel();
        if command_tx
            .send(RemoteClientCommand::Shutdown { response_tx })
            .await
            .is_ok()
            && let Ok(Ok(close_result)) = timeout(SHUTDOWN_TIMEOUT, response_rx).await
        {
            close_result?;
        }

        if let Err(_elapsed) = timeout(SHUTDOWN_TIMEOUT, &mut worker_handle).await {
            worker_handle.abort();
            let _ = worker_handle.await;
        }
        Ok(())
    }
}

impl RemoteAppServerRequestHandle {
    pub async fn request(&self, request: ClientRequest) -> IoResult<RequestResult> {
        let (response_tx, response_rx) = oneshot::channel();
        self.command_tx
            .send(RemoteClientCommand::Request {
                request: Box::new(request),
                response_tx,
            })
            .await
            .map_err(|_| {
                IoError::new(
                    ErrorKind::BrokenPipe,
                    "remote app-server worker channel is closed",
                )
            })?;
        response_rx.await.map_err(|_| {
            IoError::new(
                ErrorKind::BrokenPipe,
                "remote app-server request channel is closed",
            )
        })?
    }

    pub async fn request_typed<T>(&self, request: ClientRequest) -> Result<T, TypedRequestError>
    where
        T: DeserializeOwned,
    {
        let method = request_method_name(&request);
        let response =
            self.request(request)
                .await
                .map_err(|source| TypedRequestError::Transport {
                    method: method.clone(),
                    source,
                })?;
        let result = response.map_err(|source| TypedRequestError::Server {
            method: method.clone(),
            source,
        })?;
        decode_remote_response(result, method)
    }

    pub async fn request_json_rpc(&self, request: JSONRPCRequest) -> IoResult<RequestResult> {
        let request_value = serde_json::to_value(request).map_err(|err| {
            IoError::new(
                ErrorKind::InvalidInput,
                format!("invalid JSON-RPC request for app-server: {err}"),
            )
        })?;
        let mut request_value = match request_value {
            serde_json::Value::Object(value) => value,
            _ => {
                return Err(IoError::new(
                    ErrorKind::InvalidData,
                    "invalid JSON-RPC request payload",
                ))
            }
        };
        request_value.remove("trace");
        let request = serde_json::from_value::<ClientRequest>(serde_json::Value::Object(
            request_value,
        ))
        .map_err(|err| {
            IoError::new(
                ErrorKind::InvalidInput,
                format!("unsupported remote app-server request: {err}"),
            )
        })?;

        self.request(request).await
    }
}

async fn initialize_remote_connection(
    stream: &mut WebSocketStream<impl AsyncRead + AsyncWrite + Unpin>,
    websocket_url: &str,
    params: InitializeParams,
    initialize_timeout: Duration,
) -> IoResult<RemoteInitializeHandshake> {
    let initialize_request_id = RequestId::String("initialize".to_string());
    let mut pending_events = Vec::new();
    let mut server_version = None;
    let mut codex_home = None;
    write_jsonrpc_message(
        stream,
        JSONRPCMessage::Request(jsonrpc_request_from_client_request(
            ClientRequest::Initialize {
                request_id: initialize_request_id.clone(),
                params,
            },
        )),
        websocket_url,
    )
    .await?;

    timeout(initialize_timeout, async {
        loop {
            match stream.next().await {
                Some(Ok(Message::Text(text))) => {
                    let message = serde_json::from_str::<JSONRPCMessage>(&text).map_err(|err| {
                        IoError::other(format!(
                            "remote app server at `{websocket_url}` sent invalid initialize response: {err}"
                        ))
                    })?;
                    match message {
                        JSONRPCMessage::Response(response) if response.id == initialize_request_id => {
                            let response = serde_json::from_value::<InitializeResponse>(response.result)
                                .map_err(|err| {
                                    IoError::other(format!(
                                        "remote app server at `{websocket_url}` returned malformed initialize response: {err}"
                                    ))
                                })?;
                            server_version = parse_server_version_from_user_agent(&response.user_agent);
                            codex_home = Some(response.codex_home.to_string_lossy().into_owned());
                            break Ok(());
                        }
                        JSONRPCMessage::Error(error) if error.id == initialize_request_id => {
                            break Err(IoError::other(format!(
                                "remote app server at `{websocket_url}` rejected initialize: {}",
                                error.error.message
                            )));
                        }
                        JSONRPCMessage::Notification(notification) => {
                            if let Some(event) = app_server_event_from_notification(notification) {
                                pending_events.push(event);
                            }
                        }
                        JSONRPCMessage::Request(request) => {
                            let request_id = request.id.clone();
                            let method = request.method.clone();
                            match ServerRequest::try_from(request) {
                                Ok(request) => {
                                    pending_events.push(AppServerEvent::ServerRequest(request));
                                }
                                Err(err) => {
                                    warn!(%err, method, "rejecting unknown remote app-server request during initialize");
                                    write_jsonrpc_message(
                                        stream,
                                        JSONRPCMessage::Error(JSONRPCError {
                                            error: JSONRPCErrorError {
                                                code: -32601,
                                                message: format!(
                                                    "unsupported remote app-server request `{method}`"
                                                ),
                                                data: None,
                                            },
                                            id: request_id,
                                        }),
                                        websocket_url,
                                    )
                                    .await?;
                                }
                            }
                        }
                        JSONRPCMessage::Response(_) | JSONRPCMessage::Error(_) => {}
                    }
                }
                Some(Ok(Message::Binary(_)))
                | Some(Ok(Message::Ping(_)))
                | Some(Ok(Message::Pong(_)))
                | Some(Ok(Message::Frame(_))) => {}
                Some(Ok(Message::Close(frame))) => {
                    let reason = frame
                        .as_ref()
                        .map(|frame| frame.reason.to_string())
                        .filter(|reason| !reason.is_empty())
                        .unwrap_or_else(|| "connection closed during initialize".to_string());
                    break Err(IoError::new(
                        ErrorKind::ConnectionAborted,
                        format!(
                            "remote app server at `{websocket_url}` closed during initialize: {reason}"
                        ),
                    ));
                }
                Some(Err(err)) => {
                    break Err(IoError::other(format!(
                        "remote app server at `{websocket_url}` transport failed during initialize: {err}"
                    )));
                }
                None => {
                    break Err(IoError::new(
                        ErrorKind::UnexpectedEof,
                        format!("remote app server at `{websocket_url}` closed during initialize"),
                    ));
                }
            }
        }
    })
    .await
    .map_err(|_| {
        IoError::new(
            ErrorKind::TimedOut,
            format!("timed out waiting for initialize response from `{websocket_url}`"),
        )
    })??;

    write_jsonrpc_message(
        stream,
        JSONRPCMessage::Notification(jsonrpc_notification_from_client_notification(
            ClientNotification::Initialized,
        )),
        websocket_url,
    )
    .await?;

    Ok(RemoteInitializeHandshake {
        pending_events,
        server_version,
        codex_home,
    })
}

fn parse_server_version_from_user_agent(user_agent: &str) -> Option<String> {
    user_agent
        .split('/')
        .nth(1)
        .and_then(|version| version.split_whitespace().next())
        .map(|version| version.to_string())
}

fn app_server_event_from_notification(notification: JSONRPCNotification) -> Option<AppServerEvent> {
    match ServerNotification::try_from(notification) {
        Ok(notification) => Some(AppServerEvent::ServerNotification(notification)),
        Err(_) => None,
    }
}

fn deliver_event(
    event_tx: &mpsc::UnboundedSender<AppServerEvent>,
    event: AppServerEvent,
) -> IoResult<()> {
    event_tx.send(event).map_err(|_| {
        IoError::new(
            ErrorKind::BrokenPipe,
            "remote app-server event consumer channel is closed",
        )
    })
}

fn request_id_from_client_request(request: &ClientRequest) -> RequestId {
    jsonrpc_request_from_client_request(request.clone()).id
}

fn jsonrpc_request_from_client_request(request: ClientRequest) -> JSONRPCRequest {
    let value = match serde_json::to_value(request) {
        Ok(value) => value,
        Err(err) => panic!("client request should serialize: {err}"),
    };
    match serde_json::from_value(value) {
        Ok(request) => request,
        Err(err) => panic!("client request should encode as JSON-RPC request: {err}"),
    }
}

fn jsonrpc_notification_from_client_notification(
    notification: ClientNotification,
) -> JSONRPCNotification {
    let value = match serde_json::to_value(notification) {
        Ok(value) => value,
        Err(err) => panic!("client notification should serialize: {err}"),
    };
    match serde_json::from_value(value) {
        Ok(notification) => notification,
        Err(err) => panic!("client notification should encode as JSON-RPC notification: {err}"),
    }
}

async fn write_jsonrpc_message(
    stream: &mut WebSocketStream<impl AsyncRead + AsyncWrite + Unpin>,
    message: JSONRPCMessage,
    websocket_url: &str,
) -> IoResult<()> {
    let payload = serde_json::to_string(&message).map_err(IoError::other)?;
    stream
        .send(Message::Text(payload.into()))
        .await
        .map_err(|err| {
            IoError::other(format!(
                "failed to write websocket message to `{websocket_url}`: {err}"
            ))
        })
}
#[cfg(test)]
mod tests {
    use super::*;
    use codex_app_server_protocol::SkillsListResponse;
    use serde_json::json;

    #[tokio::test]
    async fn shutdown_tolerates_worker_exit_after_command_is_queued() {
        let (command_tx, mut command_rx) = mpsc::channel(1);
        let (_event_tx, event_rx) = mpsc::unbounded_channel::<AppServerEvent>();
        let worker_handle = tokio::spawn(async move {
            let _ = command_rx.recv().await;
        });
        let client = RemoteAppServerClient {
            command_tx,
            event_rx,
            pending_events: VecDeque::new(),
            worker_handle,
            server_version: None,
            codex_home: None,
        };

        client
            .shutdown()
            .await
            .expect("shutdown should complete when worker exits first");
    }

    #[test]
    fn decode_remote_response_accepts_unix_absolute_paths() {
        let response = json!({
            "data": [{
                "cwd": "/home/yth",
                "skills": [{
                    "name": "demo",
                    "description": "demo skill",
                    "path": "/home/yth/.ilhae/codex-home/skills/demo/SKILL.md",
                    "scope": "repo",
                    "enabled": true
                }],
                "errors": []
            }]
        });

        let decoded: SkillsListResponse =
            decode_remote_response(response, "skills/list".to_string())
                .expect("remote unix paths should decode");

        assert_eq!(decoded.data.len(), 1);
        assert_eq!(decoded.data[0].skills.len(), 1);
        assert!(
            decoded.data[0].skills[0]
                .path
                .to_string_lossy()
                .contains("home")
        );
    }
}
