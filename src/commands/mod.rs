use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use crate::approval::{Approval, ApprovalDecision};
use crate::cli::{
    ApprovalCommand, ApprovalRefArgs, ApprovalResolveArgs, Command, FsCommand, ModelsCommand,
    ResumeArgs, RunArgs, SessionCommand, SessionRefArgs, ThreadsCommand, TurnsCommand,
};
use crate::client;
use crate::config::ResolvedConfig;
use crate::error::AppError;
use crate::pending_approval::{
    list_pending_approvals, load_pending_approval, remove_pending_approval, save_pending_approval,
};
use crate::policy::{
    ApprovalPolicyEvaluation, YoloOverride, YoloState, latest_approval_evaluation,
};
use crate::protocol::events::ProtocolEventEnvelope;
use crate::session::{
    SessionDescriptor, SessionDraft, SessionHistoryMode, SessionIndex, SessionReference,
    WorkspaceBinding, select_default_session,
};

#[derive(Debug, Clone)]
pub struct CommandOutput {
    pub command: &'static str,
    pub session: Option<Value>,
    pub data: Value,
    pub meta: Value,
}

pub enum CommandExecution {
    Final(CommandOutput),
    Watch(Box<EventStream>),
}

pub struct EventStream {
    pub command: &'static str,
    pub connection: client::Connection,
    pub buffered_events: Vec<ProtocolEventEnvelope>,
}

#[derive(Debug, Clone)]
struct FlowRequest {
    command: &'static str,
    session_reference: Option<String>,
    input: Option<String>,
    pending_approval: Option<Approval>,
    ephemeral: bool,
    yolo_override: Option<YoloOverride>,
    watch: bool,
    cwd: Option<PathBuf>,
    model: Option<String>,
    approval_policy: Option<String>,
    sandbox: Option<String>,
}

#[derive(Debug, Clone)]
struct PreparedSession {
    session: SessionDescriptor,
    thread_response: Value,
    thread_events: Vec<ProtocolEventEnvelope>,
    selection_reason: String,
    lifecycle: &'static str,
    yolo_state: YoloState,
}

pub async fn execute(
    command: Command,
    config: ResolvedConfig,
) -> Result<CommandExecution, AppError> {
    match command {
        Command::Run(args) => run(args, config).await,
        Command::Resume(args) => resume(args, config).await,
        Command::Approve(args) => approve(args, config).await,
        Command::Deny(args) => deny(args, config).await,
        Command::Health => health(&config).await.map(CommandExecution::Final),
        Command::Models(ModelsCommand::List) => models_list(config).await,
        Command::Session(SessionCommand::List) => session_list(config).await,
        Command::Session(SessionCommand::Show(args)) => session_show(args, config).await,
        Command::Session(SessionCommand::Fork(args)) => session_fork(args, config).await,
        Command::Approval(ApprovalCommand::List) => {
            Ok(CommandExecution::Final(approval_list(&config)?))
        }
        Command::Approval(ApprovalCommand::Show(args)) => {
            Ok(CommandExecution::Final(approval_show(args, &config)?))
        }
        Command::Approval(ApprovalCommand::Approve(args)) => approve(args, config).await,
        Command::Approval(ApprovalCommand::Deny(args)) => deny(args, config).await,
        Command::Threads(ThreadsCommand::List) => session_list_legacy(config).await,
        Command::Threads(ThreadsCommand::Start(args)) => {
            Ok(CommandExecution::Final(CommandOutput {
                command: "threads start",
                session: None,
                data: json!({"status": "not_implemented", "scaffold": {"cwd": args.cwd.or(config.session.cwd.clone())}}),
                meta: scaffold_meta(&config),
            }))
        }
        Command::Threads(ThreadsCommand::Resume(args)) => {
            Ok(CommandExecution::Final(CommandOutput {
                command: "threads resume",
                session: None,
                data: json!({"status": "not_implemented", "scaffold": {"thread_id": args.thread_id}}),
                meta: scaffold_meta(&config),
            }))
        }
        Command::Threads(ThreadsCommand::Read(args)) => {
            Ok(CommandExecution::Final(CommandOutput {
                command: "threads read",
                session: None,
                data: json!({"status": "not_implemented", "scaffold": {"thread_id": args.thread_id}}),
                meta: scaffold_meta(&config),
            }))
        }
        Command::Turns(TurnsCommand::Start(args)) => start_explicit_turn(args, config).await,
        Command::Turns(TurnsCommand::Interrupt(args)) => {
            Ok(CommandExecution::Final(CommandOutput {
                command: "turns interrupt",
                session: None,
                data: json!({"status": "not_implemented", "scaffold": {"thread_id": args.thread_id}}),
                meta: scaffold_meta(&config),
            }))
        }
        Command::Fs(FsCommand::Ls(args)) => fs_ls(args.path, config).await,
        Command::Fs(FsCommand::Cat(args)) => fs_cat(args.path, config).await,
    }
}

async fn run(args: RunArgs, config: ResolvedConfig) -> Result<CommandExecution, AppError> {
    if args.ephemeral && args.session.is_some() {
        return Err(AppError::protocol(
            "run",
            "--session and --ephemeral cannot be combined",
        ));
    }

    execute_flow(
        FlowRequest {
            command: "run",
            session_reference: args.session,
            input: Some(args.input),
            pending_approval: None,
            ephemeral: args.ephemeral,
            yolo_override: yolo_override(args.yolo, args.no_yolo),
            watch: args.watch,
            cwd: args.cwd,
            model: args.model,
            approval_policy: args.approval_policy,
            sandbox: args.sandbox,
        },
        config,
    )
    .await
}

async fn resume(args: ResumeArgs, config: ResolvedConfig) -> Result<CommandExecution, AppError> {
    let pending_approval = load_pending_approval(&args.session)?;
    let session_reference = match pending_approval.as_ref() {
        Some(approval) => Some(approval.session_id.clone().ok_or_else(|| {
            AppError::protocol(
                "approval",
                format!(
                    "pending approval {} did not include a resumable session identifier",
                    approval.resume_token
                ),
            )
        })?),
        None => Some(args.session.clone()),
    };

    if pending_approval.is_none() && args.input.is_none() {
        return Err(AppError::protocol(
            "resume",
            "resume requires INPUT unless SESSION is a pending approval resume token",
        ));
    }

    execute_flow(
        FlowRequest {
            command: "resume",
            session_reference,
            input: args.input,
            pending_approval,
            ephemeral: false,
            yolo_override: yolo_override(args.yolo, args.no_yolo),
            watch: args.watch,
            cwd: args.cwd,
            model: args.model,
            approval_policy: args.approval_policy,
            sandbox: args.sandbox,
        },
        config,
    )
    .await
}

async fn approve(
    args: ApprovalResolveArgs,
    config: ResolvedConfig,
) -> Result<CommandExecution, AppError> {
    let approval = resolve_approval_reference(&args.reference)?;
    if args.no_resume {
        let mut approval = approval;
        approval.mark_approved();
        save_pending_approval(&approval)?;
        return Ok(CommandExecution::Final(CommandOutput {
            command: "approve",
            session: approval.session_id.as_ref().map(|id| json!({"id": id})),
            data: json!({"approval": approval, "resumed": false, "status": "approved"}),
            meta: scaffold_meta(&config),
        }));
    }

    resolve_pending_approval_via_resume(
        "approve",
        approval,
        ApprovalDecision::approve_and_resume(),
        config,
    )
    .await
}

async fn deny(args: ApprovalRefArgs, config: ResolvedConfig) -> Result<CommandExecution, AppError> {
    let approval = resolve_approval_reference(&args)?;
    resolve_pending_approval_via_resume("deny", approval, ApprovalDecision::deny(), config).await
}

async fn health(config: &ResolvedConfig) -> Result<CommandOutput, AppError> {
    let state = client::handshake(config).await?;
    Ok(CommandOutput {
        command: "health",
        session: None,
        data: json!({"status": "ok"}),
        meta: json!({
            "resolved_config": config,
            "server": {
                "transport_open": state.transport_open,
                "handshake_complete": state.handshake_complete,
                "next_request_id": state.next_request_id,
                "server_metadata": state.server_metadata,
            }
        }),
    })
}

async fn models_list(config: ResolvedConfig) -> Result<CommandExecution, AppError> {
    let mut connection = client::connect(&config).await?;
    let response: client::RequestOutcome<Value> = connection
        .request_for_command("models list", "model/list", &json!({}))
        .await?;
    let connection_state = connection.state().clone();
    connection.close().await?;

    Ok(CommandExecution::Final(CommandOutput {
        command: "models list",
        session: None,
        data: json!({
            "models": extract_array(&response.result, &["models", "items"]),
            "response": response.result,
        }),
        meta: connection_meta(&config, &connection_state),
    }))
}

async fn session_list(config: ResolvedConfig) -> Result<CommandExecution, AppError> {
    let mut connection = client::connect(&config).await?;
    let sessions = load_sessions(&mut connection, "session list").await?;
    let connection_state = connection.state().clone();
    connection.close().await?;

    Ok(CommandExecution::Final(CommandOutput {
        command: "session list",
        session: None,
        data: json!({
            "sessions": sessions.iter().map(session_envelope).collect::<Vec<_>>()
        }),
        meta: connection_meta(&config, &connection_state),
    }))
}

async fn session_list_legacy(config: ResolvedConfig) -> Result<CommandExecution, AppError> {
    let output = expect_final_output(session_list(config).await?);
    Ok(CommandExecution::Final(CommandOutput {
        command: "threads list",
        session: output.session,
        data: output.data,
        meta: output.meta,
    }))
}

async fn session_show(
    args: SessionRefArgs,
    config: ResolvedConfig,
) -> Result<CommandExecution, AppError> {
    let binding = resolve_workspace_binding(None, &config)?;
    let reference = resolve_session_reference_arg(&args)?;
    let mut connection = client::connect(&config).await?;
    let sessions = load_sessions(&mut connection, "session show").await?;
    let session = resolve_explicit_session(&reference, &binding, &sessions)?;
    let response: client::RequestOutcome<Value> = connection
        .request_for_command(
            "session show",
            "thread/read",
            &json!({"threadId": session.id}),
        )
        .await?;
    let connection_state = connection.state().clone();
    connection.close().await?;

    let session_descriptor = session_descriptor_from_thread_response(
        &response.result,
        session.alias.clone(),
        &binding,
        session.ephemeral,
        session.yolo,
    )?;

    Ok(CommandExecution::Final(CommandOutput {
        command: "session show",
        session: Some(session_envelope(&session_descriptor)),
        data: json!({"thread": response.result}),
        meta: connection_meta(&config, &connection_state),
    }))
}

async fn session_fork(
    args: SessionRefArgs,
    config: ResolvedConfig,
) -> Result<CommandExecution, AppError> {
    let binding = resolve_workspace_binding(None, &config)?;
    let reference = resolve_session_reference_arg(&args)?;
    let mut connection = client::connect(&config).await?;
    let sessions = load_sessions(&mut connection, "session fork").await?;
    let session = resolve_explicit_session(&reference, &binding, &sessions)?;
    let response: client::RequestOutcome<Value> = connection
        .request_for_command(
            "session fork",
            "thread/fork",
            &json!({"threadId": session.id, "excludeTurns": true}),
        )
        .await?;
    let connection_state = connection.state().clone();
    connection.close().await?;

    let forked_session = session_descriptor_from_thread_response(
        &response.result,
        extract_string(&response.result, &["alias", "name", "title"]),
        &binding,
        extract_bool(&response.result, &["ephemeral"]),
        extract_bool(&response.result, &["yoloMode", "yolo_mode"]),
    )?;

    Ok(CommandExecution::Final(CommandOutput {
        command: "session fork",
        session: Some(session_envelope(&forked_session)),
        data: json!({"thread": response.result}),
        meta: connection_meta(&config, &connection_state),
    }))
}

fn approval_list(config: &ResolvedConfig) -> Result<CommandOutput, AppError> {
    Ok(CommandOutput {
        command: "approval list",
        session: None,
        data: json!({"approvals": list_pending_approvals()?}),
        meta: scaffold_meta(config),
    })
}

fn approval_show(
    args: ApprovalRefArgs,
    config: &ResolvedConfig,
) -> Result<CommandOutput, AppError> {
    let approval = resolve_approval_reference(&args)?;
    Ok(CommandOutput {
        command: "approval show",
        session: approval.session_id.as_ref().map(|id| json!({"id": id})),
        data: json!({"approval": approval}),
        meta: scaffold_meta(config),
    })
}

async fn fs_ls(path: PathBuf, config: ResolvedConfig) -> Result<CommandExecution, AppError> {
    let mut connection = client::connect(&config).await?;
    let response: client::RequestOutcome<Value> = connection
        .request_for_command("fs ls", "fs/readDirectory", &json!({"path": path}))
        .await?;
    let connection_state = connection.state().clone();
    connection.close().await?;

    Ok(CommandExecution::Final(CommandOutput {
        command: "fs ls",
        session: None,
        data: json!({
            "path": path,
            "entries": extract_array(&response.result, &["entries", "children", "items"]),
            "response": response.result,
        }),
        meta: connection_meta(&config, &connection_state),
    }))
}

async fn fs_cat(path: PathBuf, config: ResolvedConfig) -> Result<CommandExecution, AppError> {
    let mut connection = client::connect(&config).await?;
    let response: client::RequestOutcome<Value> = connection
        .request_for_command("fs cat", "fs/readFile", &json!({"path": path}))
        .await?;
    let connection_state = connection.state().clone();
    connection.close().await?;

    Ok(CommandExecution::Final(CommandOutput {
        command: "fs cat",
        session: None,
        data: json!({
            "path": path,
            "content": extract_string(&response.result, &["content", "text", "base64", "data"]),
            "response": response.result,
        }),
        meta: connection_meta(&config, &connection_state),
    }))
}

async fn resolve_pending_approval_via_resume(
    command_name: &'static str,
    approval: Approval,
    decision: ApprovalDecision,
    config: ResolvedConfig,
) -> Result<CommandExecution, AppError> {
    let session_id = approval.session_id.clone().ok_or_else(|| {
        AppError::protocol(
            "approval",
            format!(
                "pending approval {} did not include a resumable session identifier",
                approval.resume_token
            ),
        )
    })?;
    let binding = resolve_workspace_binding(None, &config)?;
    let mut connection = client::connect(&config).await?;
    let initial_yolo_state = yolo_state_from_stored_approval(&approval);
    let mut params = Map::new();
    params.insert("threadId".to_owned(), json!(session_id));
    params.insert("excludeTurns".to_owned(), json!(true));
    params.insert("cwd".to_owned(), json!(resolved_cwd_for_requests(&binding)));
    params.insert(
        "approvalPolicy".to_owned(),
        json!(config.session.approval_policy),
    );
    params.insert("sandboxPolicy".to_owned(), json!(config.session.sandbox));
    if let Some(value) = initial_yolo_state.command_override {
        params.insert(
            "yoloMode".to_owned(),
            json!(matches!(value, YoloOverride::Enable)),
        );
    }

    let result = connection
        .request_resuming_approval_with_yolo(
            command_name,
            "thread/resume",
            &params,
            approval.clone(),
            decision.clone(),
            initial_yolo_state,
        )
        .await;
    let connection_state = connection.state().clone();
    let _ = connection.close().await;

    match result {
        Ok(response) => {
            let final_yolo_state = effective_yolo_state_from_resume(
                &response.result,
                initial_yolo_state.command_override,
            );
            remove_pending_approval(&approval.resume_token)?;
            Ok(CommandExecution::Final(CommandOutput {
                command: command_name,
                session: Some(json!({"id": session_id})),
                data: final_turn_data(&response.result, &response.events),
                meta: json!({
                    "resolved_config": config,
                    "workspace_binding": binding,
                    "policy": policy_meta(&final_yolo_state, &response.events),
                    "server": {
                        "transport_open": connection_state.transport_open,
                        "handshake_complete": connection_state.handshake_complete,
                        "next_request_id": connection_state.next_request_id,
                        "server_metadata": connection_state.server_metadata,
                    },
                }),
            }))
        }
        Err(err) if !decision.approved => {
            let mut denied = approval.clone();
            denied.mark_denied();
            remove_pending_approval(&approval.resume_token)?;
            Ok(CommandExecution::Final(CommandOutput {
                command: command_name,
                session: Some(json!({"id": session_id})),
                data: json!({
                    "approval": denied,
                    "resumed": false,
                    "status": "denied",
                    "error": err.to_string(),
                }),
                meta: connection_meta(&config, &connection_state),
            }))
        }
        Err(err) => Err(err),
    }
}

fn yolo_state_from_stored_approval(approval: &Approval) -> YoloState {
    let command_override = match approval
        .data
        .get("yoloOverride")
        .or_else(|| approval.data.get("yolo_override"))
        .and_then(Value::as_str)
    {
        Some("enable") => Some(YoloOverride::Enable),
        Some("disable") => Some(YoloOverride::Disable),
        _ => None,
    };
    let session_enabled = approval
        .data
        .get("yoloMode")
        .or_else(|| approval.data.get("yolo_mode"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    YoloState::from_session(session_enabled, command_override)
}

fn effective_yolo_state_from_resume(
    result: &Value,
    command_override: Option<YoloOverride>,
) -> YoloState {
    let session_enabled = extract_bool(result, &["yoloMode", "yolo_mode", "yolo"]);
    YoloState::from_session(session_enabled, command_override)
}

fn resolve_session_reference_arg(args: &SessionRefArgs) -> Result<String, AppError> {
    args.id
        .clone()
        .or(args.alias.clone())
        .ok_or_else(|| AppError::protocol("session", "expected exactly one of --id or --alias"))
}

fn resolve_approval_reference(args: &ApprovalRefArgs) -> Result<Approval, AppError> {
    let reference =
        args.id.clone().or(args.token.clone()).ok_or_else(|| {
            AppError::protocol("approval", "expected exactly one of --id or --token")
        })?;
    load_pending_approval(&reference)?.ok_or_else(|| {
        AppError::protocol(
            "approval",
            format!("pending approval not found for reference {reference}"),
        )
    })
}

fn expect_final_output(execution: CommandExecution) -> CommandOutput {
    match execution {
        CommandExecution::Final(output) => output,
        CommandExecution::Watch(_) => panic!("expected final command output"),
    }
}

async fn start_explicit_turn(
    args: crate::cli::TurnStartArgs,
    config: ResolvedConfig,
) -> Result<CommandExecution, AppError> {
    let binding = resolve_workspace_binding(None, &config)?;
    let mut connection = client::connect(&config).await?;
    let flow = FlowRequest {
        command: "turns start",
        session_reference: Some(args.thread_id.clone()),
        input: Some(args.prompt),
        pending_approval: None,
        ephemeral: false,
        yolo_override: None,
        watch: args.watch,
        cwd: None,
        model: None,
        approval_policy: None,
        sandbox: None,
    };
    let turn = match start_turn(
        &mut connection,
        &flow,
        &config,
        &binding,
        &args.thread_id,
        &YoloState::default(),
    )
    .await
    {
        Ok(turn) => turn,
        Err(err) => {
            let _ = connection.close().await;
            return Err(err);
        }
    };
    let connection_state = connection.state().clone();

    if args.watch {
        return Ok(CommandExecution::Watch(Box::new(EventStream {
            command: "turns start",
            connection,
            buffered_events: turn.events,
        })));
    }

    connection.close().await?;

    Ok(CommandExecution::Final(CommandOutput {
        command: "turns start",
        session: Some(json!({"id": args.thread_id})),
        data: final_turn_data(&turn.result, &turn.events),
        meta: json!({
            "resolved_config": config,
            "workspace_binding": binding,
            "policy": policy_meta(&YoloState::default(), &turn.events),
            "server": {
                "transport_open": connection_state.transport_open,
                "handshake_complete": connection_state.handshake_complete,
                "next_request_id": connection_state.next_request_id,
                "server_metadata": connection_state.server_metadata,
            },
        }),
    }))
}

async fn execute_flow(
    flow: FlowRequest,
    config: ResolvedConfig,
) -> Result<CommandExecution, AppError> {
    let binding = resolve_workspace_binding(flow.cwd.as_deref(), &config)?;
    let mut connection = client::connect(&config).await?;

    let listed_sessions = if flow.pending_approval.is_some() {
        Vec::new()
    } else {
        match load_sessions(&mut connection, flow.command).await {
            Ok(listed_sessions) => listed_sessions,
            Err(err) => {
                let _ = connection.close().await;
                return Err(err);
            }
        }
    };

    let prepared =
        match prepare_session(&mut connection, &flow, &config, &binding, &listed_sessions).await {
            Ok(prepared) => prepared,
            Err(err) => {
                let _ = connection.close().await;
                return Err(err);
            }
        };
    let connection_state = connection.state().clone();

    if let Some(pending_approval) = flow.pending_approval.as_ref() {
        connection.close().await?;
        remove_pending_approval(&pending_approval.resume_token)?;
        return Ok(CommandExecution::Final(CommandOutput {
            command: flow.command,
            session: Some(session_envelope(&prepared.session)),
            data: final_turn_data(&prepared.thread_response, &prepared.thread_events),
            meta: json!({
                "resolved_config": config,
                "workspace_binding": binding,
                "session_selection": {
                    "reason": prepared.selection_reason,
                    "lifecycle": prepared.lifecycle,
                },
                "thread": prepared.thread_response,
                "policy": policy_meta(&prepared.yolo_state, &prepared.thread_events),
                "server": {
                    "transport_open": connection_state.transport_open,
                    "handshake_complete": connection_state.handshake_complete,
                    "next_request_id": connection_state.next_request_id,
                    "server_metadata": connection_state.server_metadata,
                },
            }),
        }));
    }

    let turn = match start_turn(
        &mut connection,
        &flow,
        &config,
        &binding,
        &prepared.session.id,
        &prepared.yolo_state,
    )
    .await
    {
        Ok(turn) => turn,
        Err(err) => {
            let _ = connection.close().await;
            return Err(err);
        }
    };

    if flow.watch {
        return Ok(CommandExecution::Watch(Box::new(EventStream {
            command: flow.command,
            connection,
            buffered_events: turn.events,
        })));
    }

    connection.close().await?;

    Ok(CommandExecution::Final(CommandOutput {
        command: flow.command,
        session: Some(session_envelope(&prepared.session)),
        data: final_turn_data(&turn.result, &turn.events),
        meta: json!({
            "resolved_config": config,
            "workspace_binding": binding,
            "session_selection": {
                "reason": prepared.selection_reason,
                "lifecycle": prepared.lifecycle,
            },
            "thread": prepared.thread_response,
            "policy": policy_meta(&prepared.yolo_state, &turn.events),
            "server": {
                "transport_open": connection_state.transport_open,
                "handshake_complete": connection_state.handshake_complete,
                "next_request_id": connection_state.next_request_id,
                "server_metadata": connection_state.server_metadata,
            },
        }),
    }))
}

async fn load_sessions(
    connection: &mut client::Connection,
    command: &str,
) -> Result<Vec<SessionDescriptor>, AppError> {
    let listed: client::RequestOutcome<Value> = connection
        .request_for_command(command, "thread/list", &json!({}))
        .await?;
    Ok(extract_thread_items(&listed.result)
        .into_iter()
        .filter_map(session_descriptor_from_list_item)
        .collect())
}

async fn prepare_session(
    connection: &mut client::Connection,
    flow: &FlowRequest,
    config: &ResolvedConfig,
    binding: &WorkspaceBinding,
    listed_sessions: &[SessionDescriptor],
) -> Result<PreparedSession, AppError> {
    if let Some(reference) = flow.session_reference.as_ref() {
        if let Some(approval) = flow.pending_approval.as_ref() {
            let initial = yolo_state_from_stored_approval(approval);
            let response =
                resume_thread(connection, flow, config, binding, reference, &initial).await?;
            let session = session_descriptor_from_thread_response(
                &response.result,
                Some(reference.clone()),
                binding,
                false,
                initial.effective,
            )?;
            let yolo_state =
                effective_yolo_state_from_resume(&response.result, initial.command_override);
            return Ok(PreparedSession {
                session,
                thread_response: response.result,
                thread_events: response.events,
                selection_reason: "approval_resume_token".to_owned(),
                lifecycle: "reused",
                yolo_state,
            });
        }

        let session = resolve_explicit_session(reference, binding, listed_sessions)?;
        let yolo_state = YoloState::from_session(session.yolo, flow.yolo_override);
        let response =
            resume_thread(connection, flow, config, binding, &session.id, &yolo_state).await?;
        let session = session_descriptor_from_thread_response(
            &response.result,
            Some(session.alias.clone().unwrap_or_else(|| reference.clone())),
            binding,
            session.ephemeral,
            yolo_state.effective,
        )?;
        return Ok(PreparedSession {
            session,
            thread_response: response.result,
            thread_events: response.events,
            selection_reason: "explicit_reference".to_owned(),
            lifecycle: "reused",
            yolo_state,
        });
    }

    match select_default_session(binding, listed_sessions, flow.ephemeral) {
        crate::session::SessionSelection::Reuse { session, reason } => {
            let yolo_state = YoloState::from_session(session.yolo, flow.yolo_override);
            let response =
                resume_thread(connection, flow, config, binding, &session.id, &yolo_state).await?;
            let session = session_descriptor_from_thread_response(
                &response.result,
                session.alias.clone(),
                binding,
                session.ephemeral,
                yolo_state.effective,
            )?;
            Ok(PreparedSession {
                session,
                thread_response: response.result,
                thread_events: response.events,
                selection_reason: reason_key(&reason).to_owned(),
                lifecycle: "reused",
                yolo_state,
            })
        }
        crate::session::SessionSelection::Create { draft, reason } => {
            let yolo_state = YoloState::for_new_session(flow.yolo_override);
            let response =
                start_thread(connection, flow, config, binding, &draft, &yolo_state).await?;
            let session = session_descriptor_from_thread_response(
                &response.result,
                None,
                binding,
                draft.ephemeral,
                yolo_state.effective,
            )?;
            Ok(PreparedSession {
                session,
                thread_response: response.result,
                thread_events: response.events,
                selection_reason: reason_key(&reason).to_owned(),
                lifecycle: "created",
                yolo_state,
            })
        }
    }
}

async fn start_thread(
    connection: &mut client::Connection,
    flow: &FlowRequest,
    config: &ResolvedConfig,
    binding: &WorkspaceBinding,
    draft: &SessionDraft,
    yolo_state: &YoloState,
) -> Result<client::RequestOutcome<Value>, AppError> {
    connection
        .request_for_command_with_yolo(
            flow.command,
            "thread/start",
            &json!({
                "cwd": resolved_cwd_for_requests(binding),
                "workspaceRoot": draft.workspace_root,
                "approvalPolicy": flow
                    .approval_policy
                    .clone()
                    .unwrap_or_else(|| config.session.approval_policy.clone()),
                "sandboxPolicy": flow
                    .sandbox
                    .clone()
                    .unwrap_or_else(|| config.session.sandbox.clone()),
                "model": flow.model,
                "ephemeral": draft.ephemeral,
                "yoloMode": yolo_state.effective,
                "historyMode": history_mode_key(&draft.history_mode),
            }),
            *yolo_state,
        )
        .await
}

async fn resume_thread(
    connection: &mut client::Connection,
    flow: &FlowRequest,
    config: &ResolvedConfig,
    binding: &WorkspaceBinding,
    session_id: &str,
    yolo_state: &YoloState,
) -> Result<client::RequestOutcome<Value>, AppError> {
    let params = json!({
        "threadId": session_id,
        "excludeTurns": true,
        "cwd": resolved_cwd_for_requests(binding),
        "approvalPolicy": flow
            .approval_policy
            .clone()
            .unwrap_or_else(|| config.session.approval_policy.clone()),
        "sandboxPolicy": flow
            .sandbox
            .clone()
            .unwrap_or_else(|| config.session.sandbox.clone()),
        "model": flow.model,
        "yoloMode": yolo_state.effective,
    });

    if let Some(approval) = flow.pending_approval.clone() {
        connection
            .request_resuming_approval_with_yolo(
                flow.command,
                "thread/resume",
                &params,
                approval,
                ApprovalDecision::approve_and_resume(),
                *yolo_state,
            )
            .await
    } else {
        connection
            .request_for_command_with_yolo(flow.command, "thread/resume", &params, *yolo_state)
            .await
    }
}

async fn start_turn(
    connection: &mut client::Connection,
    flow: &FlowRequest,
    _config: &ResolvedConfig,
    _binding: &WorkspaceBinding,
    thread_id: &str,
    yolo_state: &YoloState,
) -> Result<client::RequestOutcome<Value>, AppError> {
    let input = flow
        .input
        .clone()
        .ok_or_else(|| AppError::protocol(flow.command, "missing turn input"))?;
    connection
        .request_for_command_with_yolo(
            flow.command,
            "turn/start",
            &json!({
                "threadId": thread_id,
                "input": input,
                "yoloMode": yolo_state.effective,
            }),
            *yolo_state,
        )
        .await
}

fn resolve_workspace_binding(
    cwd_override: Option<&Path>,
    config: &ResolvedConfig,
) -> Result<WorkspaceBinding, AppError> {
    let cwd = cwd_override
        .map(PathBuf::from)
        .or_else(|| config.session.cwd.clone())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    WorkspaceBinding::discover(&cwd)
        .map_err(|err| AppError::protocol("session", format!("workspace binding failed: {err}")))
}

fn resolve_explicit_session(
    reference: &str,
    binding: &WorkspaceBinding,
    sessions: &[SessionDescriptor],
) -> Result<SessionDescriptor, AppError> {
    let index = SessionIndex::new(sessions);
    index
        .resolve(&SessionReference::parse(reference), Some(binding))
        .cloned()
        .map_err(|err| AppError::protocol("session", err.to_string()))
}

fn final_turn_data(result: &Value, events: &[ProtocolEventEnvelope]) -> Value {
    let turn_id = extract_string(result, &["turnId", "turn_id"]).or_else(|| {
        events.iter().rev().find_map(|event| match &event.event {
            crate::protocol::events::ProtocolEvent::TurnStarted { turn_id, .. }
            | crate::protocol::events::ProtocolEvent::TurnCompleted { turn_id, .. } => {
                turn_id.clone()
            }
            _ => None,
        })
    });
    let output = result
        .get("output")
        .cloned()
        .unwrap_or_else(|| result.clone());
    let mut obj = serde_json::Map::new();
    if let Some(turn_id) = turn_id {
        obj.insert("turn_id".to_owned(), json!(turn_id));
    }
    obj.insert("output".to_owned(), output);
    if !events.is_empty() {
        obj.insert("events".to_owned(), json!(events));
    }
    Value::Object(obj)
}

fn session_envelope(session: &SessionDescriptor) -> Value {
    json!({
        "id": session.id,
        "alias": session.alias,
        "workspace_root": session.workspace_root,
        "repo_root": session.repo_root,
        "ephemeral": session.ephemeral,
        "yolo": session.yolo,
        "last_active_at": session.last_active_at,
    })
}

fn connection_meta(config: &ResolvedConfig, connection_state: &client::ConnectionState) -> Value {
    json!({
        "resolved_config": config,
        "server": {
            "transport_open": connection_state.transport_open,
            "handshake_complete": connection_state.handshake_complete,
            "next_request_id": connection_state.next_request_id,
            "server_metadata": connection_state.server_metadata,
        },
    })
}

fn scaffold_meta(config: &ResolvedConfig) -> Value {
    json!({"resolved_config": config})
}

fn policy_meta(yolo_state: &YoloState, events: &[ProtocolEventEnvelope]) -> Value {
    let last_approval: Option<ApprovalPolicyEvaluation> =
        latest_approval_evaluation(events, yolo_state);
    json!({
        "yolo": yolo_state,
        "last_approval": last_approval,
    })
}

fn extract_thread_items(value: &Value) -> Vec<Value> {
    extract_array(value, &["threads", "items", "sessions"])
}

fn session_descriptor_from_list_item(value: Value) -> Option<SessionDescriptor> {
    let id = extract_string(&value, &["id", "threadId", "thread_id"])?;
    let workspace_root = normalize_path(PathBuf::from(extract_string(
        &value,
        &["cwd", "workspaceRoot", "workspace_root"],
    )?));
    let repo_root = extract_string(&value, &["repoRoot", "repo_root"])
        .map(PathBuf::from)
        .map(normalize_path);
    Some(SessionDescriptor {
        id,
        alias: extract_string(&value, &["name", "alias", "title"]),
        workspace_root,
        repo_root,
        ephemeral: extract_bool(&value, &["ephemeral"]),
        yolo: extract_bool(&value, &["yoloMode", "yolo_mode", "yolo"]),
        last_active_at: extract_string(&value, &["updatedAt", "lastActiveAt", "updated_at"]),
    })
}

fn session_descriptor_from_thread_response(
    value: &Value,
    alias_hint: Option<String>,
    binding: &WorkspaceBinding,
    ephemeral_default: bool,
    yolo_default: bool,
) -> Result<SessionDescriptor, AppError> {
    let id = extract_string(value, &["id", "threadId", "thread_id"])
        .ok_or_else(|| AppError::protocol("session", "thread response missing id/threadId"))?;
    let workspace_root = extract_string(value, &["cwd", "workspaceRoot", "workspace_root"])
        .map(PathBuf::from)
        .map(normalize_path)
        .unwrap_or_else(|| binding.workspace_root.clone());
    let repo_root = extract_string(value, &["repoRoot", "repo_root"])
        .map(PathBuf::from)
        .map(normalize_path)
        .or_else(|| binding.repo_root.clone());
    Ok(SessionDescriptor {
        id,
        alias: extract_string(value, &["name", "alias", "title"]).or(alias_hint),
        workspace_root,
        repo_root,
        ephemeral: value
            .get("ephemeral")
            .and_then(Value::as_bool)
            .unwrap_or(ephemeral_default),
        yolo: value
            .get("yoloMode")
            .or_else(|| value.get("yolo_mode"))
            .and_then(Value::as_bool)
            .unwrap_or(yolo_default),
        last_active_at: extract_string(value, &["updatedAt", "lastActiveAt", "updated_at"]),
    })
}

fn normalize_path(path: PathBuf) -> PathBuf {
    fs::canonicalize(&path).unwrap_or(path)
}

fn reason_key(reason: &crate::session::SessionSelectionReason) -> &'static str {
    match reason {
        crate::session::SessionSelectionReason::ExplicitEphemeral => "explicit_ephemeral",
        crate::session::SessionSelectionReason::WorkspaceScopedDefault => {
            "workspace_scoped_default"
        }
        crate::session::SessionSelectionReason::NoWorkspaceMatch => "no_workspace_match",
    }
}

fn history_mode_key(mode: &SessionHistoryMode) -> &'static str {
    match mode {
        SessionHistoryMode::ResumePrior => "resume_prior",
        SessionHistoryMode::CleanWorkspaceIdentity => "clean_workspace_identity",
    }
}

fn resolved_cwd_for_requests(binding: &WorkspaceBinding) -> &Path {
    binding.resolved_cwd.as_path()
}

fn yolo_override(yolo: bool, no_yolo: bool) -> Option<YoloOverride> {
    if yolo {
        Some(YoloOverride::Enable)
    } else if no_yolo {
        Some(YoloOverride::Disable)
    } else {
        None
    }
}

fn extract_array(value: &Value, keys: &[&str]) -> Vec<Value> {
    if let Some(array) = keys
        .iter()
        .find_map(|key| value.get(*key).and_then(Value::as_array))
    {
        return array.clone();
    }
    Vec::new()
}

fn extract_string(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        value
            .get(*key)
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
    })
}

fn extract_bool(value: &Value, keys: &[&str]) -> bool {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_bool))
        .unwrap_or(false)
}
