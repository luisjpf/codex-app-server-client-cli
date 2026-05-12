use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{self, BufRead, Write};

use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::json;
use tokio::time::{Duration, Instant, timeout_at};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::http::header::AUTHORIZATION;

use super::rpc::JsonRpcRouter;
use crate::approval::{
    Approval, ApprovalDecision, ApprovalRequiredEnvelope, ApprovalResult,
    approval_response_payload, current_stdio_is_interactive, latest_session_id,
    prompt_for_approval,
};
use crate::cli::Transport;
use crate::config::ResolvedConfig;
use crate::error::AppError;
use crate::policy::{YoloOverride, YoloState, evaluate_approval};
use crate::protocol::events::ProtocolEventEnvelope;
use crate::protocol::messages::{
    JsonRpcIncomingMessage, JsonRpcNotification, JsonRpcRequest, JsonRpcResultMessage, RequestId,
};

const JSONRPC_VERSION: &str = "2.0";
const CLIENT_TITLE: &str = "Codex app-server client CLI";

type WsStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerMetadata {
    pub codex_home: String,
    pub platform_family: String,
    pub platform_os: String,
    pub user_agent: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConnectionState {
    pub transport_open: bool,
    pub handshake_complete: bool,
    pub next_request_id: u64,
    pub server_metadata: Option<ServerMetadata>,
}

impl Default for ConnectionState {
    fn default() -> Self {
        Self {
            transport_open: false,
            handshake_complete: false,
            next_request_id: 1,
            server_metadata: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RequestOutcome<T> {
    pub request_id: RequestId,
    pub result: T,
    pub events: Vec<ProtocolEventEnvelope>,
}

pub struct Connection {
    socket: Option<WsStream>,
    state: ConnectionState,
    request_timeout: Duration,
    router: JsonRpcRouter,
    pending_events: VecDeque<ProtocolEventEnvelope>,
    pending_responses: HashMap<RequestId, BufferedResponse>,
    recoverable_response_ids: HashSet<RequestId>,
}

enum ApprovalMode<'a> {
    Capture {
        command: String,
        yolo_state: YoloState,
    },
    Resume {
        approval: Box<Approval>,
        decision: ApprovalDecision,
        yolo_state: YoloState,
    },
    Prompt {
        reader: &'a mut dyn BufRead,
        writer: &'a mut dyn Write,
        yolo_state: YoloState,
    },
}

impl Connection {
    pub fn state(&self) -> &ConnectionState {
        &self.state
    }

    pub fn into_state(self) -> ConnectionState {
        self.state
    }

    pub async fn close(&mut self) -> Result<(), AppError> {
        if let Some(mut socket) = self.socket.take() {
            self.state.transport_open = false;
            socket
                .close(None)
                .await
                .map_err(|err| AppError::connection("close", err.to_string()))?;
        } else {
            self.state.transport_open = false;
        }
        Ok(())
    }

    pub async fn request<P: Serialize, R: DeserializeOwned>(
        &mut self,
        method: impl Into<String>,
        params: &P,
    ) -> Result<RequestOutcome<R>, AppError> {
        let method = method.into();
        self.request_for_command(&method, method.clone(), params)
            .await
    }

    pub async fn request_for_command_with_yolo<P: Serialize, R: DeserializeOwned>(
        &mut self,
        command: &str,
        method: impl Into<String>,
        params: &P,
        yolo_state: YoloState,
    ) -> Result<RequestOutcome<R>, AppError> {
        let method = method.into();
        let result = if current_stdio_is_interactive() {
            let stdin = io::stdin();
            let stdout = io::stdout();
            let mut reader = stdin.lock();
            let mut writer = stdout.lock();
            self.request_with_approval_mode(
                method,
                params,
                ApprovalMode::Prompt {
                    reader: &mut reader,
                    writer: &mut writer,
                    yolo_state,
                },
            )
            .await
        } else {
            self.request_with_approval_mode(
                method,
                params,
                ApprovalMode::Capture {
                    command: command.to_owned(),
                    yolo_state,
                },
            )
            .await
        };

        if matches!(result, Err(AppError::ApprovalRequired { .. })) && self.state.transport_open {
            let _ = self.close().await;
        }

        result
    }

    pub async fn request_for_command<P: Serialize, R: DeserializeOwned>(
        &mut self,
        command: &str,
        method: impl Into<String>,
        params: &P,
    ) -> Result<RequestOutcome<R>, AppError> {
        self.request_for_command_with_yolo(command, method, params, YoloState::default())
            .await
    }

    pub async fn request_resuming_approval_with_yolo<P: Serialize, R: DeserializeOwned>(
        &mut self,
        _command: &str,
        method: impl Into<String>,
        params: &P,
        approval: Approval,
        decision: ApprovalDecision,
        yolo_state: YoloState,
    ) -> Result<RequestOutcome<R>, AppError> {
        let method = method.into();
        self.request_with_approval_mode(
            method,
            params,
            ApprovalMode::Resume {
                approval: Box::new(approval),
                decision,
                yolo_state,
            },
        )
        .await
    }

    pub async fn request_resuming_approval<P: Serialize, R: DeserializeOwned>(
        &mut self,
        command: &str,
        method: impl Into<String>,
        params: &P,
        approval: Approval,
    ) -> Result<RequestOutcome<R>, AppError> {
        self.request_resuming_approval_with_yolo(
            command,
            method,
            params,
            approval,
            ApprovalDecision::approve_and_resume(),
            YoloState::default(),
        )
        .await
    }

    async fn request_with_approval_mode<P: Serialize, R: DeserializeOwned>(
        &mut self,
        method: String,
        params: &P,
        mut approval_mode: ApprovalMode<'_>,
    ) -> Result<RequestOutcome<R>, AppError> {
        if !self.state.transport_open {
            return Err(AppError::connection(
                "request",
                "cannot send JSON-RPC request on a closed transport",
            ));
        }

        let prepared = self.router.prepare_request(method, params)?;
        self.state.next_request_id = self.router.next_request_id();
        let request_id = prepared.request.id.clone();
        let method_name = prepared.method.clone();

        let socket = self.socket.as_mut().ok_or_else(|| {
            AppError::connection("request", "connection lost before request dispatch")
        })?;
        send_text_message(socket, &prepared.request, "request").await?;

        let response = self
            .read_response_with_approval(request_id.clone(), &method_name, &mut approval_mode)
            .await?;
        let result = serde_json::from_value(response.result).map_err(|err| {
            AppError::protocol(
                "request",
                format!("{method_name}: invalid response payload: {err}"),
            )
        })?;

        Ok(RequestOutcome {
            request_id,
            result,
            events: response.events,
        })
    }

    pub async fn next_event(&mut self) -> Result<ProtocolEventEnvelope, AppError> {
        if !self.state.transport_open {
            return Err(AppError::connection(
                "event",
                "cannot read server events from a closed transport",
            ));
        }

        loop {
            if let Some(event) = self.pending_events.pop_front() {
                return Ok(event);
            }

            match self.read_next_incoming(None, "event").await? {
                JsonRpcIncomingMessage::Notification(notification) => {
                    return Ok(self
                        .router
                        .normalize_notification(notification.method, notification.params));
                }
                JsonRpcIncomingMessage::ServerRequest(request) => {
                    return Ok(self.router.normalize_server_request(request));
                }
                JsonRpcIncomingMessage::Success(response) => {
                    self.buffer_recoverable_response("event", BufferedResponse::Success(response))?;
                }
                JsonRpcIncomingMessage::Error(response) => {
                    self.buffer_recoverable_response("event", BufferedResponse::Error(response))?;
                }
            }
        }
    }

    async fn read_response_with_approval(
        &mut self,
        request_id: RequestId,
        method_name: &str,
        approval_mode: &mut ApprovalMode<'_>,
    ) -> Result<PendingResponse, AppError> {
        if let Some(response) = self.pending_responses.remove(&request_id) {
            return response.into_pending_response("request", method_name);
        }

        let deadline = Instant::now() + self.request_timeout;
        let mut events = Vec::new();

        loop {
            let incoming = match self.read_next_incoming(Some(deadline), "request").await {
                Ok(incoming) => incoming,
                Err(err) => {
                    if is_server_message_timeout(&err, "request") {
                        self.recoverable_response_ids.insert(request_id.clone());
                    }
                    return Err(err);
                }
            };

            match incoming {
                JsonRpcIncomingMessage::Success(response) => {
                    if response.id == request_id {
                        return Ok(PendingResponse {
                            result: response.result,
                            events,
                        });
                    }
                    self.buffer_recoverable_response(
                        "request",
                        BufferedResponse::Success(response),
                    )?;
                }
                JsonRpcIncomingMessage::Error(response) => {
                    if response.id == request_id {
                        return Err(AppError::protocol(
                            "request",
                            format!(
                                "{method_name}: server returned JSON-RPC error {}: {}",
                                response.error.code, response.error.message
                            ),
                        ));
                    }
                    self.buffer_recoverable_response("request", BufferedResponse::Error(response))?;
                }
                JsonRpcIncomingMessage::Notification(notification) => {
                    events.push(
                        self.router
                            .normalize_notification(notification.method, notification.params),
                    );
                }
                JsonRpcIncomingMessage::ServerRequest(request) => {
                    let event = self.router.normalize_server_request(request);
                    if let Some(approval) = Approval::from_event(&event, latest_session_id(&events))
                    {
                        self.handle_approval_request(method_name, approval, approval_mode)
                            .await?;
                    }
                    events.push(event);
                }
            }
        }
    }

    async fn handle_approval_request(
        &mut self,
        method_name: &str,
        approval: Approval,
        approval_mode: &mut ApprovalMode<'_>,
    ) -> Result<(), AppError> {
        match approval_mode {
            ApprovalMode::Capture {
                command,
                yolo_state,
            } => {
                if evaluate_approval(&approval, yolo_state).allows_auto_approve() {
                    let result = ApprovalDecision::approve_and_resume().into_result(approval);
                    self.send_approval_response(&result).await
                } else {
                    let approval = Self::approval_with_yolo_context(approval, *yolo_state);
                    Err(AppError::approval_required(ApprovalRequiredEnvelope::new(
                        command.clone(),
                        format!(
                            "{command}: server requested approval before execution can continue"
                        ),
                        approval,
                    )))
                }
            }
            ApprovalMode::Resume {
                approval: stored,
                decision,
                yolo_state,
            } => {
                let result = if approval.request_id == stored.request_id {
                    decision.clone().into_result((**stored).clone())
                } else if evaluate_approval(&approval, yolo_state).allows_auto_approve() {
                    ApprovalDecision::approve_and_resume().into_result(approval)
                } else {
                    let approval = Self::approval_with_yolo_context(approval, *yolo_state);
                    return Err(AppError::approval_required(ApprovalRequiredEnvelope::new(
                        method_name.to_owned(),
                        format!(
                            "{method_name}: server requested another approval before execution can continue"
                        ),
                        approval,
                    )));
                };
                self.send_approval_response(&result).await?;
                if result.decision.approved {
                    Ok(())
                } else {
                    Err(AppError::protocol(
                        "approval",
                        format!("{method_name}: operator denied approval request"),
                    ))
                }
            }
            ApprovalMode::Prompt {
                reader,
                writer,
                yolo_state,
            } => {
                let decision = if evaluate_approval(&approval, yolo_state).allows_auto_approve() {
                    ApprovalDecision::approve_and_resume()
                } else {
                    prompt_for_approval(&approval, reader, writer).map_err(|err| {
                        AppError::connection(
                            "approval",
                            format!("failed to collect interactive approval response: {err}"),
                        )
                    })?
                };
                let result = decision.into_result(approval);
                self.send_approval_response(&result).await?;
                if result.decision.approved {
                    Ok(())
                } else {
                    Err(AppError::protocol(
                        "approval",
                        format!("{method_name}: operator denied approval request"),
                    ))
                }
            }
        }
    }

    fn approval_with_yolo_context(mut approval: Approval, yolo_state: YoloState) -> Approval {
        if let Some(object) = approval.data.as_object_mut() {
            object.insert("yoloMode".to_owned(), json!(yolo_state.session_enabled));
            if let Some(command_override) = yolo_state.command_override {
                let value = match command_override {
                    YoloOverride::Enable => "enable",
                    YoloOverride::Disable => "disable",
                    YoloOverride::None => return approval,
                };
                object.insert("yoloOverride".to_owned(), json!(value));
            }
        }
        approval
    }

    async fn send_approval_response(&mut self, result: &ApprovalResult) -> Result<(), AppError> {
        let socket = self.socket.as_mut().ok_or_else(|| {
            AppError::connection(
                "approval",
                "connection lost before approval response dispatch",
            )
        })?;
        let response = JsonRpcResultMessage {
            jsonrpc: JSONRPC_VERSION.to_owned(),
            id: result.approval.request_id.clone(),
            result: approval_response_payload(&result.approval, &result.decision),
        };
        send_text_message(socket, &response, "approval").await
    }

    async fn read_next_incoming(
        &mut self,
        deadline: Option<Instant>,
        phase: &'static str,
    ) -> Result<JsonRpcIncomingMessage, AppError> {
        loop {
            let socket = self
                .socket
                .as_mut()
                .ok_or_else(|| AppError::connection(phase, "transport closed unexpectedly"))?;

            let next = match deadline {
                Some(deadline) => timeout_at(deadline, socket.next())
                    .await
                    .map_err(|_| timed_out_waiting_for_server_message(phase))?,
                None => socket.next().await,
            };

            let Some(frame) = next else {
                self.state.transport_open = false;
                return Err(AppError::connection(
                    phase,
                    "websocket closed before server message arrived",
                ));
            };

            match frame.map_err(|err| AppError::connection(phase, err.to_string()))? {
                Message::Text(text) => return decode_server_message(phase, &text),
                Message::Binary(_) => {
                    return Err(AppError::protocol(
                        phase,
                        "expected text JSON-RPC frame but received binary data",
                    ));
                }
                Message::Close(frame) => {
                    self.state.transport_open = false;
                    return Err(AppError::connection(
                        phase,
                        format!("websocket closed while reading server message: {frame:?}"),
                    ));
                }
                Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => continue,
            }
        }
    }

    fn buffer_recoverable_response(
        &mut self,
        phase: &'static str,
        response: BufferedResponse,
    ) -> Result<(), AppError> {
        let response_id = response.id().clone();
        if self.recoverable_response_ids.remove(&response_id) {
            self.pending_responses.insert(response_id, response);
            Ok(())
        } else {
            Err(AppError::protocol(
                phase,
                format!(
                    "received unsolicited JSON-RPC response for unknown request id: {:?}",
                    response_id
                ),
            ))
        }
    }
}

#[derive(Debug, Serialize)]
struct InitializeParams {
    #[serde(rename = "clientInfo")]
    client_info: ClientInfo,
    capabilities: InitializeCapabilities,
}

#[derive(Debug, Serialize)]
struct ClientInfo {
    name: &'static str,
    version: &'static str,
    title: &'static str,
}

#[derive(Debug, Serialize)]
struct InitializeCapabilities {
    #[serde(rename = "experimentalApi")]
    experimental_api: bool,
    #[serde(rename = "optOutNotificationMethods")]
    opt_out_notification_methods: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct InitializeResponse {
    #[serde(rename = "codexHome")]
    codex_home: String,
    #[serde(rename = "platformFamily")]
    platform_family: String,
    #[serde(rename = "platformOs")]
    platform_os: String,
    #[serde(rename = "userAgent")]
    user_agent: String,
}

#[derive(Debug)]
struct PendingResponse {
    result: serde_json::Value,
    events: Vec<ProtocolEventEnvelope>,
}

#[derive(Debug)]
enum BufferedResponse {
    Success(crate::protocol::messages::JsonRpcSuccessResponse),
    Error(crate::protocol::messages::JsonRpcErrorResponse),
}

impl BufferedResponse {
    fn id(&self) -> &RequestId {
        match self {
            Self::Success(response) => &response.id,
            Self::Error(response) => &response.id,
        }
    }

    fn into_pending_response(
        self,
        phase: &'static str,
        method_name: &str,
    ) -> Result<PendingResponse, AppError> {
        match self {
            Self::Success(response) => Ok(PendingResponse {
                result: response.result,
                events: Vec::new(),
            }),
            Self::Error(response) => Err(AppError::protocol(
                phase,
                format!(
                    "{method_name}: server returned JSON-RPC error {}: {}",
                    response.error.code, response.error.message
                ),
            )),
        }
    }
}

pub async fn connect(config: &ResolvedConfig) -> Result<Connection, AppError> {
    match config.connection.transport {
        Transport::Ws => connect_ws(config).await,
        ref transport => Err(AppError::unsupported_transport(
            *transport,
            "only websocket transport is implemented in v1",
        )),
    }
}

pub async fn handshake(config: &ResolvedConfig) -> Result<ConnectionState, AppError> {
    let mut connection = connect(config).await?;
    let state = connection.state().clone();
    connection.close().await?;
    Ok(state)
}

async fn connect_ws(config: &ResolvedConfig) -> Result<Connection, AppError> {
    let mut request = config
        .connection
        .url
        .as_str()
        .into_client_request()
        .map_err(|err| AppError::connection("connect", err.to_string()))?;
    if let Some(token) = config.connection.bearer_token.as_ref() {
        request.headers_mut().insert(
            AUTHORIZATION,
            format!("Bearer {token}")
                .parse::<HeaderValue>()
                .map_err(|err| AppError::authentication("connect", err.to_string()))?,
        );
    }

    let (socket, _) = connect_async(request)
        .await
        .map_err(|err| AppError::connection("connect", err.to_string()))?;

    let mut connection = Connection {
        socket: Some(socket),
        state: ConnectionState {
            transport_open: true,
            handshake_complete: false,
            next_request_id: 1,
            server_metadata: None,
        },
        request_timeout: Duration::from_millis(config.connection.request_timeout_ms),
        router: JsonRpcRouter::new(1),
        pending_events: VecDeque::new(),
        pending_responses: HashMap::new(),
        recoverable_response_ids: HashSet::new(),
    };

    let initialize = JsonRpcRequest {
        jsonrpc: JSONRPC_VERSION.to_owned(),
        id: RequestId::from(1),
        method: "initialize".to_owned(),
        params: serde_json::to_value(InitializeParams {
            client_info: ClientInfo {
                name: env!("CARGO_PKG_NAME"),
                version: env!("CARGO_PKG_VERSION"),
                title: CLIENT_TITLE,
            },
            capabilities: InitializeCapabilities {
                experimental_api: true,
                opt_out_notification_methods: Vec::new(),
            },
        })
        .map_err(AppError::json)?,
    };

    {
        let socket = connection.socket.as_mut().ok_or_else(|| {
            AppError::connection("initialize", "connection lost before initialize dispatch")
        })?;
        send_text_message(socket, &initialize, "initialize").await?;
    }

    let response = connection
        .read_next_incoming(
            Some(Instant::now() + connection.request_timeout),
            "initialize",
        )
        .await?;
    let init_response = match response {
        JsonRpcIncomingMessage::Success(success) if success.id == RequestId::from(1) => {
            serde_json::from_value::<InitializeResponse>(success.result).map_err(|err| {
                AppError::protocol("initialize", format!("invalid initialize payload: {err}"))
            })?
        }
        JsonRpcIncomingMessage::Error(err) if err.id == RequestId::from(1) => {
            return Err(AppError::protocol(
                "initialize",
                format!(
                    "server returned JSON-RPC error {}: {}",
                    err.error.code, err.error.message
                ),
            ));
        }
        other => {
            return Err(AppError::protocol(
                "initialize",
                format!("unexpected message during initialize: {other:?}"),
            ));
        }
    };

    connection.state.handshake_complete = true;
    connection.state.next_request_id = 2;
    connection.router = JsonRpcRouter::new(2);
    connection.state.server_metadata = Some(ServerMetadata {
        codex_home: init_response.codex_home,
        platform_family: init_response.platform_family,
        platform_os: init_response.platform_os,
        user_agent: init_response.user_agent,
    });

    let initialized = JsonRpcNotification {
        jsonrpc: JSONRPC_VERSION.to_owned(),
        method: "initialized".to_owned(),
        params: None,
    };
    let socket = connection.socket.as_mut().ok_or_else(|| {
        AppError::connection(
            "initialize",
            "connection lost before initialized notification",
        )
    })?;
    send_text_message(socket, &initialized, "initialize").await?;

    Ok(connection)
}

async fn send_text_message<T: Serialize>(
    socket: &mut WsStream,
    value: &T,
    phase: &'static str,
) -> Result<(), AppError> {
    let text = serde_json::to_string(value).map_err(AppError::json)?;
    socket
        .send(Message::Text(text))
        .await
        .map_err(|err| AppError::connection(phase, err.to_string()))
}

fn decode_server_message(
    phase: &'static str,
    text: &str,
) -> Result<JsonRpcIncomingMessage, AppError> {
    serde_json::from_str::<JsonRpcIncomingMessage>(text)
        .map_err(|err| AppError::protocol(phase, format!("invalid JSON-RPC message: {err}")))
}

fn timed_out_waiting_for_server_message(phase: &'static str) -> AppError {
    AppError::connection(phase, "timed out waiting for server message")
}

fn is_server_message_timeout(err: &AppError, phase: &'static str) -> bool {
    matches!(
        err,
        AppError::Connection { phase: err_phase, detail }
            if *err_phase == phase && detail.contains("timed out waiting for server message")
    )
}
