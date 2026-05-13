use clap::Parser;
use codex_app_server_client_cli::cli::Cli;
use codex_app_server_client_cli::commands::{CommandExecution, CommandOutput, execute};
use codex_app_server_client_cli::config::ResolvedConfig;
use codex_app_server_client_cli::protocol::messages::{JsonRpcNotification, JsonRpcRequest};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;
use std::process::Command as ProcessCommand;
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::net::TcpListener;
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::Message;

const JSONRPC_VERSION: &str = "2.0";
static ENV_LOCK: Mutex<()> = Mutex::new(());

#[tokio::test]
async fn run_command_reuses_repo_default_session_and_returns_session_envelope() {
    let _config_dir = TestConfigDir::install(
        "run_command_reuses_repo_default_session_and_returns_session_envelope",
    );
    let repo_root = temp_repo("run_reuse_repo_default");
    let nested = repo_root.join("src/bin");
    fs::create_dir_all(&nested).expect("nested dir should exist");
    let other_root = temp_repo("run_reuse_other_repo");

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind listener");
    let addr = listener.local_addr().expect("listener addr");
    let repo_root_for_server = repo_root.clone();
    let other_root_for_server = other_root.clone();

    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept connection");
        let mut socket = accept_async(stream).await.expect("accept websocket");
        handshake(&mut socket).await;

        let list = next_request(&mut socket).await;
        assert_eq!(list.method, "thread/list");
        send_result(
            &mut socket,
            list.id,
            json!({
                "threads": [
                    {
                        "id": "sess_other",
                        "name": "feature-auth",
                        "cwd": other_root_for_server,
                        "updatedAt": "2026-05-11T10:00:00Z"
                    },
                    {
                        "id": "sess_repo",
                        "name": "feature-auth",
                        "cwd": repo_root_for_server,
                        "updatedAt": "2026-05-11T11:00:00Z"
                    }
                ]
            }),
        )
        .await;

        let resume = next_request(&mut socket).await;
        assert_eq!(resume.method, "thread/resume");
        assert_eq!(resume.params.get("threadId"), Some(&json!("sess_repo")));
        send_result(
            &mut socket,
            resume.id,
            json!({
                "id": "sess_repo",
                "name": "feature-auth",
                "cwd": repo_root_for_server,
                "ephemeral": false,
                "yoloMode": false
            }),
        )
        .await;

        let turn = next_request(&mut socket).await;
        assert_eq!(turn.method, "turn/start");
        assert_eq!(turn.params.get("threadId"), Some(&json!("sess_repo")));
        assert_eq!(
            turn.params.get("input"),
            Some(&json!("summarize the workspace"))
        );
        send_notification(
            &mut socket,
            json!({
                "jsonrpc": JSONRPC_VERSION,
                "method": "turn/started",
                "params": {
                    "threadId": "sess_repo",
                    "turnId": "turn_repo"
                }
            }),
        )
        .await;
        send_result(
            &mut socket,
            turn.id,
            json!({
                "turnId": "turn_repo",
                "output": {"summary": "done"}
            }),
        )
        .await;

        expect_close(&mut socket).await;
    });

    let cli = Cli::parse_from([
        "bin",
        "--url",
        &format!("ws://{addr}"),
        "--cwd",
        nested.to_str().expect("nested path utf8"),
        "run",
        "summarize the workspace",
    ]);
    let config = ResolvedConfig::load(&cli).expect("config should load");
    let output = expect_final(
        execute(cli.command, config)
            .await
            .expect("run should succeed"),
    );

    assert_eq!(output.command, "run");
    assert_eq!(
        output
            .session
            .as_ref()
            .and_then(|session| session.get("id")),
        Some(&json!("sess_repo"))
    );
    assert_eq!(output.data.get("turn_id"), Some(&json!("turn_repo")));
    assert_eq!(output.data.pointer("/output/summary"), Some(&json!("done")));
    assert!(output.data.get("response").is_none());
    assert_eq!(
        output.meta.pointer("/session_selection/reason"),
        Some(&json!("workspace_scoped_default"))
    );

    server.await.expect("server task should finish");
}

#[tokio::test]
async fn run_command_creates_session_when_workspace_has_no_match() {
    let _config_dir =
        TestConfigDir::install("run_command_creates_session_when_workspace_has_no_match");
    let repo_root = temp_repo("run_create_session");
    let nested = repo_root.join("src");
    fs::create_dir_all(&nested).expect("nested dir should exist");

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind listener");
    let addr = listener.local_addr().expect("listener addr");
    let repo_root_for_server = repo_root.clone();

    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept connection");
        let mut socket = accept_async(stream).await.expect("accept websocket");
        handshake(&mut socket).await;

        let list = next_request(&mut socket).await;
        assert_eq!(list.method, "thread/list");
        send_result(&mut socket, list.id, json!({"threads": []})).await;

        let start = next_request(&mut socket).await;
        assert_eq!(start.method, "thread/start");
        send_result(
            &mut socket,
            start.id,
            json!({
                "id": "sess_new",
                "cwd": repo_root_for_server,
                "ephemeral": false,
                "yoloMode": false
            }),
        )
        .await;

        let turn = next_request(&mut socket).await;
        assert_eq!(turn.method, "turn/start");
        assert_eq!(turn.params.get("threadId"), Some(&json!("sess_new")));
        send_result(
            &mut socket,
            turn.id,
            json!({
                "turnId": "turn_new",
                "output": {"summary": "created"}
            }),
        )
        .await;

        expect_close(&mut socket).await;
    });

    let cli = Cli::parse_from([
        "bin",
        "--url",
        &format!("ws://{addr}"),
        "--cwd",
        nested.to_str().expect("nested path utf8"),
        "run",
        "start fresh",
    ]);
    let config = ResolvedConfig::load(&cli).expect("config should load");
    let output = expect_final(
        execute(cli.command, config)
            .await
            .expect("run should succeed"),
    );

    assert_eq!(output.command, "run");
    assert_eq!(
        output
            .session
            .as_ref()
            .and_then(|session| session.get("id")),
        Some(&json!("sess_new"))
    );
    assert_eq!(output.data.get("turn_id"), Some(&json!("turn_new")));
    assert_eq!(
        output.data.pointer("/output/summary"),
        Some(&json!("created"))
    );
    assert!(output.data.get("response").is_none());
    assert_eq!(
        output.meta.pointer("/session_selection/lifecycle"),
        Some(&json!("created"))
    );

    server.await.expect("server task should finish");
}

#[tokio::test]
async fn resume_command_accepts_alias_reference_and_starts_turn() {
    let _config_dir =
        TestConfigDir::install("resume_command_accepts_alias_reference_and_starts_turn");
    let repo_root = temp_repo("resume_alias_reference");

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind listener");
    let addr = listener.local_addr().expect("listener addr");
    let repo_root_for_server = repo_root.clone();

    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept connection");
        let mut socket = accept_async(stream).await.expect("accept websocket");
        handshake(&mut socket).await;

        let list = next_request(&mut socket).await;
        assert_eq!(list.method, "thread/list");
        send_result(
            &mut socket,
            list.id,
            json!({
                "threads": [
                    {
                        "id": "sess_feature_auth",
                        "name": "feature-auth",
                        "cwd": repo_root_for_server,
                        "updatedAt": "2026-05-11T11:30:00Z"
                    }
                ]
            }),
        )
        .await;

        let resume = next_request(&mut socket).await;
        assert_eq!(resume.method, "thread/resume");
        assert_eq!(
            resume.params.get("threadId"),
            Some(&json!("sess_feature_auth"))
        );
        send_result(
            &mut socket,
            resume.id,
            json!({
                "id": "sess_feature_auth",
                "name": "feature-auth",
                "cwd": repo_root_for_server
            }),
        )
        .await;

        let turn = next_request(&mut socket).await;
        assert_eq!(turn.method, "turn/start");
        assert_eq!(
            turn.params.get("threadId"),
            Some(&json!("sess_feature_auth"))
        );
        assert_eq!(turn.params.get("input"), Some(&json!("continue the work")));
        send_result(
            &mut socket,
            turn.id,
            json!({
                "turnId": "turn_resume",
                "output": {"summary": "continued"}
            }),
        )
        .await;

        expect_close(&mut socket).await;
    });

    let cli = Cli::parse_from([
        "bin",
        "--url",
        &format!("ws://{addr}"),
        "--cwd",
        repo_root.to_str().expect("repo path utf8"),
        "resume",
        "feature-auth",
        "continue the work",
    ]);
    let config = ResolvedConfig::load(&cli).expect("config should load");
    let output = expect_final(
        execute(cli.command, config)
            .await
            .expect("resume should succeed"),
    );

    assert_eq!(output.command, "resume");
    assert_eq!(
        output
            .session
            .as_ref()
            .and_then(|session| session.get("id")),
        Some(&json!("sess_feature_auth"))
    );
    assert_eq!(output.data.get("turn_id"), Some(&json!("turn_resume")));
    assert_eq!(
        output.meta.pointer("/session_selection/reason"),
        Some(&json!("explicit_reference"))
    );

    server.await.expect("server task should finish");
}

#[tokio::test(flavor = "multi_thread")]
async fn run_command_non_interactive_approval_returns_run_envelope_and_exit_code_7() {
    let _config_dir = TestConfigDir::install(
        "run_command_non_interactive_approval_returns_run_envelope_and_exit_code_7",
    );
    let repo_root = temp_repo("run_non_interactive_approval");

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind listener");
    let addr = listener.local_addr().expect("listener addr");
    let repo_root_for_server = repo_root.clone();

    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept connection");
        let mut socket = accept_async(stream).await.expect("accept websocket");
        handshake(&mut socket).await;

        let list = next_request(&mut socket).await;
        assert_eq!(list.method, "thread/list");
        send_result(&mut socket, list.id, json!({"threads": []})).await;

        let start = next_request(&mut socket).await;
        assert_eq!(start.method, "thread/start");
        send_result(
            &mut socket,
            start.id,
            json!({
                "id": "sess_new",
                "cwd": repo_root_for_server,
                "ephemeral": false,
                "yoloMode": false
            }),
        )
        .await;

        let turn = next_request(&mut socket).await;
        assert_eq!(turn.method, "turn/start");
        send_notification(
            &mut socket,
            json!({
                "jsonrpc": JSONRPC_VERSION,
                "method": "turn/started",
                "params": {
                    "threadId": "sess_new",
                    "turnId": "turn_approval"
                }
            }),
        )
        .await;
        send_notification(
            &mut socket,
            json!({
                "jsonrpc": JSONRPC_VERSION,
                "id": "approval-1",
                "method": "item/commandExecution/requestApproval",
                "params": {
                    "itemId": "item-approval",
                    "summary": "Run npm test",
                    "command": ["npm", "test"],
                    "requestedAt": "2026-05-11T20:15:00Z"
                }
            }),
        )
        .await;

        expect_close(&mut socket).await;
    });

    let output = tokio::task::spawn_blocking(move || {
        ProcessCommand::new(env!("CARGO_BIN_EXE_codex-app-server-client-cli"))
            .args([
                "--url",
                &format!("ws://{addr}"),
                "--cwd",
                repo_root.to_str().expect("repo path utf8"),
                "run",
                "needs approval",
            ])
            .output()
            .expect("run binary")
    })
    .await
    .expect("join binary task");

    assert_eq!(output.status.code(), Some(7));
    assert!(String::from_utf8_lossy(&output.stderr).trim().is_empty());

    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    let envelope: Value = serde_json::from_str(stdout.trim()).expect("valid approval envelope");
    assert_eq!(envelope["ok"], json!(false));
    assert_eq!(envelope["command"], json!("run"));
    assert_eq!(envelope["error"]["code"], json!("approval_required"));
    assert_eq!(envelope["approval"]["approval_id"], json!("approval-1"));
    assert_eq!(
        envelope["approval"]["resume_token"],
        json!("sess_new:approval-1")
    );
    assert_eq!(envelope["approval"]["session_id"], json!("sess_new"));
    assert_eq!(envelope["approval"]["scope"], json!("command_execution"));

    server.await.expect("server task should finish");
}

#[tokio::test(flavor = "multi_thread")]
async fn resume_command_with_approval_token_resumes_blocked_step() {
    let _config_dir =
        TestConfigDir::install("resume_command_with_approval_token_resumes_blocked_step");
    let repo_root = temp_repo("resume_pending_approval");

    let approval_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind approval listener");
    let approval_addr = approval_listener
        .local_addr()
        .expect("approval listener addr");
    let repo_root_for_approval_server = repo_root.clone();

    let approval_server = tokio::spawn(async move {
        let (stream, _) = approval_listener
            .accept()
            .await
            .expect("accept approval connection");
        let mut socket = accept_async(stream).await.expect("accept websocket");
        handshake(&mut socket).await;

        let list = next_request(&mut socket).await;
        assert_eq!(list.method, "thread/list");
        send_result(&mut socket, list.id, json!({"threads": []})).await;

        let start = next_request(&mut socket).await;
        assert_eq!(start.method, "thread/start");
        send_result(
            &mut socket,
            start.id,
            json!({
                "id": "sess_new",
                "cwd": repo_root_for_approval_server,
                "ephemeral": false,
                "yoloMode": false
            }),
        )
        .await;

        let turn = next_request(&mut socket).await;
        assert_eq!(turn.method, "turn/start");
        send_notification(
            &mut socket,
            json!({
                "jsonrpc": JSONRPC_VERSION,
                "method": "turn/started",
                "params": {
                    "threadId": "sess_new",
                    "turnId": "turn_pending_approval"
                }
            }),
        )
        .await;
        send_notification(
            &mut socket,
            json!({
                "jsonrpc": JSONRPC_VERSION,
                "id": "approval-1",
                "method": "item/commandExecution/requestApproval",
                "params": {
                    "itemId": "item-approval",
                    "summary": "Run npm test",
                    "command": ["npm", "test"],
                    "requestedAt": "2026-05-11T20:15:00Z"
                }
            }),
        )
        .await;

        expect_close(&mut socket).await;
    });

    let initial_output = tokio::task::spawn_blocking({
        let repo_root = repo_root.clone();
        move || {
            ProcessCommand::new(env!("CARGO_BIN_EXE_codex-app-server-client-cli"))
                .args([
                    "--url",
                    &format!("ws://{approval_addr}"),
                    "--cwd",
                    repo_root.to_str().expect("repo path utf8"),
                    "run",
                    "needs approval",
                ])
                .output()
                .expect("run binary")
        }
    })
    .await
    .expect("join initial binary task");

    assert_eq!(initial_output.status.code(), Some(7));
    let initial_stdout = String::from_utf8(initial_output.stdout).expect("stdout utf8");
    let initial_envelope: Value =
        serde_json::from_str(initial_stdout.trim()).expect("valid approval envelope");
    let resume_token = initial_envelope["approval"]["resume_token"]
        .as_str()
        .expect("resume token string")
        .to_owned();
    assert_eq!(resume_token, "sess_new:approval-1");

    approval_server
        .await
        .expect("approval server should finish");

    let resume_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind resume listener");
    let resume_addr = resume_listener.local_addr().expect("resume listener addr");
    let repo_root_for_resume_server = repo_root.clone();

    let resume_server = tokio::spawn(async move {
        let (stream, _) = resume_listener
            .accept()
            .await
            .expect("accept resume connection");
        let mut socket = accept_async(stream).await.expect("accept websocket");
        handshake(&mut socket).await;

        let resume = next_request(&mut socket).await;
        assert_eq!(resume.method, "thread/resume");
        assert_eq!(resume.params.get("threadId"), Some(&json!("sess_new")));

        send_notification(
            &mut socket,
            json!({
                "jsonrpc": JSONRPC_VERSION,
                "method": "turn/started",
                "params": {
                    "threadId": "sess_new",
                    "turnId": "turn_resumed"
                }
            }),
        )
        .await;
        send_notification(
            &mut socket,
            json!({
                "jsonrpc": JSONRPC_VERSION,
                "id": "approval-1",
                "method": "item/commandExecution/requestApproval",
                "params": {
                    "itemId": "item-approval",
                    "summary": "Run npm test",
                    "command": ["npm", "test"],
                    "requestedAt": "2026-05-11T20:15:00Z"
                }
            }),
        )
        .await;

        let approval_response = socket
            .next()
            .await
            .expect("approval response frame present")
            .expect("approval response frame ok")
            .into_text()
            .expect("approval response text");
        let approval_response: Value =
            serde_json::from_str(&approval_response).expect("parse approval response");
        assert_eq!(approval_response["id"], json!("approval-1"));
        assert_eq!(approval_response["result"]["approved"], json!(true));
        assert_eq!(approval_response["result"]["resume"], json!(true));
        assert_eq!(
            approval_response["result"]["approvalId"],
            json!("approval-1")
        );
        assert_eq!(
            approval_response["result"]["resumeToken"],
            json!("sess_new:approval-1")
        );

        send_result(
            &mut socket,
            resume.id,
            json!({
                "threadId": "sess_new",
                "turnId": "turn_resumed",
                "output": {"summary": "approval resumed"},
                "cwd": repo_root_for_resume_server,
                "ephemeral": false,
                "yoloMode": false
            }),
        )
        .await;

        expect_close(&mut socket).await;
    });

    let resumed_output = tokio::task::spawn_blocking(move || {
        ProcessCommand::new(env!("CARGO_BIN_EXE_codex-app-server-client-cli"))
            .args([
                "--url",
                &format!("ws://{resume_addr}"),
                "--cwd",
                repo_root.to_str().expect("repo path utf8"),
                "resume",
                &resume_token,
            ])
            .output()
            .expect("resume binary")
    })
    .await
    .expect("join resumed binary task");

    assert!(resumed_output.status.success());
    assert!(
        String::from_utf8_lossy(&resumed_output.stderr)
            .trim()
            .is_empty()
    );

    let resumed_stdout = String::from_utf8(resumed_output.stdout).expect("stdout utf8");
    let resumed_envelope: Value =
        serde_json::from_str(resumed_stdout.trim()).expect("valid resumed envelope");
    assert_eq!(resumed_envelope["ok"], json!(true));
    assert_eq!(resumed_envelope["command"], json!("resume"));
    assert_eq!(resumed_envelope["session"]["id"], json!("sess_new"));
    assert_eq!(resumed_envelope["data"]["turn_id"], json!("turn_resumed"));
    assert_eq!(
        resumed_envelope["data"]["output"]["summary"],
        json!("approval resumed")
    );

    resume_server.await.expect("resume server should finish");
}

#[tokio::test]
async fn run_command_uses_session_scoped_yolo_by_default() {
    let _config_dir = TestConfigDir::install("run_command_uses_session_scoped_yolo_by_default");
    let repo_root = temp_repo("run_session_yolo_default");

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind listener");
    let addr = listener.local_addr().expect("listener addr");
    let repo_root_for_server = repo_root.clone();

    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept connection");
        let mut socket = accept_async(stream).await.expect("accept websocket");
        handshake(&mut socket).await;

        let list = next_request(&mut socket).await;
        assert_eq!(list.method, "thread/list");
        send_result(
            &mut socket,
            list.id,
            json!({
                "threads": [{
                    "id": "sess_yolo",
                    "name": "feature-yolo",
                    "cwd": repo_root_for_server,
                    "updatedAt": "2026-05-12T00:00:00Z",
                    "yoloMode": true
                }]
            }),
        )
        .await;

        let resume = next_request(&mut socket).await;
        assert_eq!(resume.method, "thread/resume");
        assert_eq!(resume.params.get("threadId"), Some(&json!("sess_yolo")));
        assert_eq!(resume.params.get("yoloMode"), Some(&json!(true)));
        send_result(
            &mut socket,
            resume.id,
            json!({
                "id": "sess_yolo",
                "name": "feature-yolo",
                "cwd": repo_root_for_server,
                "ephemeral": false,
                "yoloMode": true
            }),
        )
        .await;

        let turn = next_request(&mut socket).await;
        assert_eq!(turn.method, "turn/start");
        assert_eq!(turn.params.get("threadId"), Some(&json!("sess_yolo")));
        assert_eq!(turn.params.get("yoloMode"), Some(&json!(true)));
        send_result(
            &mut socket,
            turn.id,
            json!({
                "turnId": "turn_yolo",
                "output": {"summary": "yolo session reused"}
            }),
        )
        .await;

        expect_close(&mut socket).await;
    });

    let cli = Cli::parse_from([
        "bin",
        "--url",
        &format!("ws://{addr}"),
        "--cwd",
        repo_root.to_str().expect("repo path utf8"),
        "run",
        "continue with session defaults",
    ]);
    let config = ResolvedConfig::load(&cli).expect("config should load");
    let output = expect_final(
        execute(cli.command, config)
            .await
            .expect("run should succeed"),
    );

    assert_eq!(
        output
            .session
            .as_ref()
            .and_then(|session| session.get("yolo")),
        Some(&json!(true))
    );
    assert_eq!(
        output.meta.pointer("/policy/yolo/effective"),
        Some(&json!(true))
    );
    assert_eq!(
        output.meta.pointer("/policy/yolo/source"),
        Some(&json!("session"))
    );

    server.await.expect("server task should finish");
}

#[tokio::test]
async fn run_command_no_yolo_disables_session_scoped_yolo() {
    let _config_dir = TestConfigDir::install("run_command_no_yolo_disables_session_scoped_yolo");
    let repo_root = temp_repo("run_session_yolo_disable");

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind listener");
    let addr = listener.local_addr().expect("listener addr");
    let repo_root_for_server = repo_root.clone();

    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept connection");
        let mut socket = accept_async(stream).await.expect("accept websocket");
        handshake(&mut socket).await;

        let list = next_request(&mut socket).await;
        assert_eq!(list.method, "thread/list");
        send_result(
            &mut socket,
            list.id,
            json!({
                "threads": [{
                    "id": "sess_yolo",
                    "name": "feature-yolo",
                    "cwd": repo_root_for_server,
                    "updatedAt": "2026-05-12T00:00:00Z",
                    "yoloMode": true
                }]
            }),
        )
        .await;

        let resume = next_request(&mut socket).await;
        assert_eq!(resume.method, "thread/resume");
        assert_eq!(resume.params.get("yoloMode"), Some(&json!(false)));
        send_result(
            &mut socket,
            resume.id,
            json!({
                "id": "sess_yolo",
                "name": "feature-yolo",
                "cwd": repo_root_for_server,
                "ephemeral": false,
                "yoloMode": false
            }),
        )
        .await;

        let turn = next_request(&mut socket).await;
        assert_eq!(turn.method, "turn/start");
        assert_eq!(turn.params.get("yoloMode"), Some(&json!(false)));
        send_result(
            &mut socket,
            turn.id,
            json!({
                "turnId": "turn_yolo_disabled",
                "output": {"summary": "yolo disabled for this command"}
            }),
        )
        .await;

        expect_close(&mut socket).await;
    });

    let cli = Cli::parse_from([
        "bin",
        "--url",
        &format!("ws://{addr}"),
        "--cwd",
        repo_root.to_str().expect("repo path utf8"),
        "run",
        "disable yolo for this turn",
        "--no-yolo",
    ]);
    let config = ResolvedConfig::load(&cli).expect("config should load");
    let output = expect_final(
        execute(cli.command, config)
            .await
            .expect("run should succeed"),
    );

    assert_eq!(
        output
            .session
            .as_ref()
            .and_then(|session| session.get("yolo")),
        Some(&json!(false))
    );
    assert_eq!(
        output.meta.pointer("/policy/yolo/effective"),
        Some(&json!(false))
    );
    assert_eq!(
        output.meta.pointer("/policy/yolo/source"),
        Some(&json!("command_override_disable"))
    );

    server.await.expect("server task should finish");
}

#[tokio::test]
async fn run_command_with_yolo_auto_approves_command_execution_requests() {
    let _config_dir =
        TestConfigDir::install("run_command_with_yolo_auto_approves_command_execution_requests");
    let repo_root = temp_repo("run_yolo_auto_approve");

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind listener");
    let addr = listener.local_addr().expect("listener addr");
    let repo_root_for_server = repo_root.clone();

    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept connection");
        let mut socket = accept_async(stream).await.expect("accept websocket");
        handshake(&mut socket).await;

        let list = next_request(&mut socket).await;
        assert_eq!(list.method, "thread/list");
        send_result(&mut socket, list.id, json!({"threads": []})).await;

        let start = next_request(&mut socket).await;
        assert_eq!(start.method, "thread/start");
        assert_eq!(start.params.get("yoloMode"), Some(&json!(true)));
        send_result(
            &mut socket,
            start.id,
            json!({
                "id": "sess_yolo_new",
                "cwd": repo_root_for_server,
                "ephemeral": false,
                "yoloMode": true
            }),
        )
        .await;

        let turn = next_request(&mut socket).await;
        assert_eq!(turn.method, "turn/start");
        assert_eq!(turn.params.get("yoloMode"), Some(&json!(true)));
        send_notification(
            &mut socket,
            json!({
                "jsonrpc": JSONRPC_VERSION,
                "method": "turn/started",
                "params": {
                    "threadId": "sess_yolo_new",
                    "turnId": "turn_yolo_new"
                }
            }),
        )
        .await;
        send_notification(
            &mut socket,
            json!({
                "jsonrpc": JSONRPC_VERSION,
                "id": "approval-yolo-1",
                "method": "item/commandExecution/requestApproval",
                "params": {
                    "itemId": "item-yolo-approval",
                    "summary": "Run npm test",
                    "command": ["npm", "test"],
                    "requestedAt": "2026-05-12T00:15:00Z"
                }
            }),
        )
        .await;

        let approval_response = socket
            .next()
            .await
            .expect("approval response frame present")
            .expect("approval response frame ok")
            .into_text()
            .expect("approval response text");
        let approval_response: Value =
            serde_json::from_str(&approval_response).expect("parse approval response");
        assert_eq!(approval_response["id"], json!("approval-yolo-1"));
        assert_eq!(approval_response["result"]["approved"], json!(true));
        assert_eq!(approval_response["result"]["resume"], json!(true));

        send_result(
            &mut socket,
            turn.id,
            json!({
                "turnId": "turn_yolo_new",
                "output": {"summary": "yolo auto-approved command"}
            }),
        )
        .await;

        expect_close(&mut socket).await;
    });

    let cli = Cli::parse_from([
        "bin",
        "--url",
        &format!("ws://{addr}"),
        "--cwd",
        repo_root.to_str().expect("repo path utf8"),
        "run",
        "ship it",
        "--yolo",
    ]);
    let config = ResolvedConfig::load(&cli).expect("config should load");
    let output = expect_final(
        execute(cli.command, config)
            .await
            .expect("run should succeed"),
    );

    assert_eq!(
        output.data.pointer("/output/summary"),
        Some(&json!("yolo auto-approved command"))
    );
    assert_eq!(
        output.meta.pointer("/policy/last_approval/category"),
        Some(&json!("command_execution"))
    );
    assert_eq!(
        output.meta.pointer("/policy/last_approval/risk_traits"),
        Some(&json!(["shell_exec"]))
    );
    assert_eq!(
        output.meta.pointer("/policy/last_approval/decision"),
        Some(&json!("auto_approve_yolo"))
    );

    server.await.expect("server task should finish");
}

#[tokio::test]
async fn resume_command_with_pending_approval_threads_yolo_to_follow_on_approvals() {
    let _config_dir = TestConfigDir::install(
        "resume_command_with_pending_approval_threads_yolo_to_follow_on_approvals",
    );
    let repo_root = temp_repo("resume_pending_approval_yolo");

    let approval_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind approval listener");
    let approval_addr = approval_listener
        .local_addr()
        .expect("approval listener addr");
    let repo_root_for_approval_server = repo_root.clone();

    let approval_server = tokio::spawn(async move {
        let (stream, _) = approval_listener
            .accept()
            .await
            .expect("accept approval connection");
        let mut socket = accept_async(stream).await.expect("accept websocket");
        handshake(&mut socket).await;

        let list = next_request(&mut socket).await;
        assert_eq!(list.method, "thread/list");
        send_result(&mut socket, list.id, json!({"threads": []})).await;

        let start = next_request(&mut socket).await;
        assert_eq!(start.method, "thread/start");
        send_result(
            &mut socket,
            start.id,
            json!({
                "id": "sess_resume_yolo",
                "cwd": repo_root_for_approval_server,
                "ephemeral": false,
                "yoloMode": false
            }),
        )
        .await;

        let turn = next_request(&mut socket).await;
        assert_eq!(turn.method, "turn/start");
        assert_eq!(turn.params.get("yoloMode"), Some(&json!(true)));
        send_notification(
            &mut socket,
            json!({
                "jsonrpc": JSONRPC_VERSION,
                "method": "turn/started",
                "params": {
                    "threadId": "sess_resume_yolo",
                    "turnId": "turn_resume_yolo"
                }
            }),
        )
        .await;
        send_notification(
            &mut socket,
            json!({
                "jsonrpc": JSONRPC_VERSION,
                "id": "approval-resume-yolo-1",
                "method": "item/custom/requestApproval",
                "params": {
                    "itemId": "item-yolo-approval",
                    "summary": "Confirm risky external action",
                    "requestedAction": "confirm external action",
                    "requestedAt": "2026-05-12T00:15:00Z"
                }
            }),
        )
        .await;

        expect_close(&mut socket).await;
    });

    let initial_output = tokio::task::spawn_blocking({
        let repo_root = repo_root.clone();
        move || {
            ProcessCommand::new(env!("CARGO_BIN_EXE_codex-app-server-client-cli"))
                .args([
                    "--url",
                    &format!("ws://{approval_addr}"),
                    "--cwd",
                    repo_root.to_str().expect("repo path utf8"),
                    "run",
                    "needs yolo follow-on approval",
                    "--yolo",
                ])
                .output()
                .expect("run binary")
        }
    })
    .await
    .expect("join initial binary task");

    assert_eq!(initial_output.status.code(), Some(7));
    let initial_stdout = String::from_utf8(initial_output.stdout).expect("stdout utf8");
    let initial_envelope: Value =
        serde_json::from_str(initial_stdout.trim()).expect("valid approval envelope");
    let resume_token = initial_envelope["approval"]["resume_token"]
        .as_str()
        .expect("resume token string")
        .to_owned();
    assert_eq!(resume_token, "sess_resume_yolo:approval-resume-yolo-1");
    assert_eq!(
        initial_envelope["approval"]["data"]["yoloOverride"],
        json!("enable")
    );
    assert_eq!(
        initial_envelope["approval"]["data"]["yoloMode"],
        json!(false)
    );

    approval_server
        .await
        .expect("approval server should finish");

    let resume_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind resume listener");
    let resume_addr = resume_listener.local_addr().expect("resume listener addr");
    let repo_root_for_resume_server = repo_root.clone();

    let resume_server = tokio::spawn(async move {
        let (stream, _) = resume_listener
            .accept()
            .await
            .expect("accept resume connection");
        let mut socket = accept_async(stream).await.expect("accept websocket");
        handshake(&mut socket).await;

        let resume = next_request(&mut socket).await;
        assert_eq!(resume.method, "thread/resume");
        assert_eq!(
            resume.params.get("threadId"),
            Some(&json!("sess_resume_yolo"))
        );
        assert_eq!(resume.params.get("yoloMode"), Some(&json!(true)));

        send_notification(
            &mut socket,
            json!({
                "jsonrpc": JSONRPC_VERSION,
                "method": "turn/started",
                "params": {
                    "threadId": "sess_resume_yolo",
                    "turnId": "turn_resumed_yolo"
                }
            }),
        )
        .await;
        send_notification(
            &mut socket,
            json!({
                "jsonrpc": JSONRPC_VERSION,
                "id": "approval-resume-yolo-1",
                "method": "item/commandExecution/requestApproval",
                "params": {
                    "itemId": "item-yolo-approval",
                    "summary": "Run npm test",
                    "command": ["npm", "test"],
                    "requestedAt": "2026-05-12T00:15:00Z"
                }
            }),
        )
        .await;

        let approval_response = socket
            .next()
            .await
            .expect("stored approval response frame present")
            .expect("stored approval response frame ok")
            .into_text()
            .expect("stored approval response text");
        let approval_response: Value =
            serde_json::from_str(&approval_response).expect("parse stored approval response");
        assert_eq!(approval_response["id"], json!("approval-resume-yolo-1"));
        assert_eq!(approval_response["result"]["approved"], json!(true));
        assert_eq!(approval_response["result"]["resume"], json!(true));

        send_notification(
            &mut socket,
            json!({
                "jsonrpc": JSONRPC_VERSION,
                "id": "approval-resume-yolo-2",
                "method": "item/commandExecution/requestApproval",
                "params": {
                    "itemId": "item-yolo-approval-2",
                    "summary": "Deploy build artifact",
                    "command": ["deploy", "artifact"],
                    "requestedAt": "2026-05-12T00:16:00Z"
                }
            }),
        )
        .await;

        let follow_on_response = socket
            .next()
            .await
            .expect("follow-on approval response frame present")
            .expect("follow-on approval response frame ok")
            .into_text()
            .expect("follow-on approval response text");
        let follow_on_response: Value =
            serde_json::from_str(&follow_on_response).expect("parse follow-on approval response");
        assert_eq!(follow_on_response["id"], json!("approval-resume-yolo-2"));
        assert_eq!(follow_on_response["result"]["approved"], json!(true));
        assert_eq!(follow_on_response["result"]["resume"], json!(true));

        send_result(
            &mut socket,
            resume.id,
            json!({
                "threadId": "sess_resume_yolo",
                "turnId": "turn_resumed_yolo",
                "output": {"summary": "resume token auto-approved follow-on command"},
                "cwd": repo_root_for_resume_server,
                "ephemeral": false,
                "yoloMode": true
            }),
        )
        .await;

        expect_close(&mut socket).await;
    });

    let resumed_output = tokio::task::spawn_blocking(move || {
        ProcessCommand::new(env!("CARGO_BIN_EXE_codex-app-server-client-cli"))
            .args([
                "--url",
                &format!("ws://{resume_addr}"),
                "--cwd",
                repo_root.to_str().expect("repo path utf8"),
                "resume",
                &resume_token,
            ])
            .output()
            .expect("resume binary")
    })
    .await
    .expect("join resumed binary task");

    assert!(resumed_output.status.success());
    let resumed_stdout = String::from_utf8(resumed_output.stdout).expect("stdout utf8");
    let resumed_envelope: Value =
        serde_json::from_str(resumed_stdout.trim()).expect("valid resumed envelope");
    assert_eq!(
        resumed_envelope["data"]["output"]["summary"],
        json!("resume token auto-approved follow-on command")
    );
    assert_eq!(
        resumed_envelope["meta"].pointer("/policy/yolo/effective"),
        Some(&json!(true))
    );
    assert_eq!(
        resumed_envelope["meta"].pointer("/policy/yolo/source"),
        Some(&json!("command_override_enable"))
    );
    assert_eq!(
        resumed_envelope["meta"].pointer("/policy/last_approval/decision"),
        Some(&json!("auto_approve_yolo"))
    );

    resume_server.await.expect("resume server should finish");
}

async fn handshake(socket: &mut tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>) {
    let initialize = next_request(socket).await;
    assert_eq!(initialize.method, "initialize");
    send_result(
        socket,
        initialize.id,
        json!({
            "codexHome": "/tmp/codex-home",
            "platformFamily": "unix",
            "platformOs": "macos",
            "userAgent": "codex-app-server/0.test"
        }),
    )
    .await;

    let initialized = socket
        .next()
        .await
        .expect("initialized frame present")
        .expect("initialized frame ok")
        .into_text()
        .expect("initialized text");
    let notification: JsonRpcNotification =
        serde_json::from_str(&initialized).expect("parse initialized notification");
    assert_eq!(notification.method, "initialized");
}

async fn next_request(
    socket: &mut tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
) -> JsonRpcRequest {
    let frame = socket
        .next()
        .await
        .expect("request frame present")
        .expect("request frame ok")
        .into_text()
        .expect("request text");
    serde_json::from_str(&frame).expect("parse request")
}

async fn send_result(
    socket: &mut tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
    id: codex_app_server_client_cli::protocol::messages::RequestId,
    result: Value,
) {
    socket
        .send(Message::Text(
            json!({
                "jsonrpc": JSONRPC_VERSION,
                "id": id,
                "result": result,
            })
            .to_string()
            .into(),
        ))
        .await
        .expect("send result");
}

async fn send_notification(
    socket: &mut tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
    payload: Value,
) {
    socket
        .send(Message::Text(payload.to_string().into()))
        .await
        .expect("send notification");
}

async fn expect_close(socket: &mut tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>) {
    let close = socket.next().await.expect("close frame present");
    assert!(matches!(close, Ok(Message::Close(_))));
}

fn expect_final(execution: CommandExecution) -> CommandOutput {
    match execution {
        CommandExecution::Final(output) => output,
        CommandExecution::Watch(_) => panic!("expected final command output, got watch stream"),
    }
}

fn temp_repo(label: &str) -> PathBuf {
    let root = temp_dir(label);
    fs::create_dir_all(root.join(".git")).expect("git dir should exist");
    root
}

fn temp_dir(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should move forward")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("codex_cli_{label}_{nonce}"));
    fs::create_dir_all(&root).expect("temp dir should exist");
    root
}

struct TestConfigDir {
    _lock: MutexGuard<'static, ()>,
    old_home: Option<OsString>,
    old_xdg_config_home: Option<OsString>,
}

impl TestConfigDir {
    fn install(label: &str) -> Self {
        let lock = ENV_LOCK.lock().expect("lock env");
        let root = temp_dir(label);
        let xdg = root.join("xdg-config");
        fs::create_dir_all(&xdg).expect("create xdg config dir");

        let old_home = env::var_os("HOME");
        let old_xdg_config_home = env::var_os("XDG_CONFIG_HOME");

        unsafe {
            env::set_var("HOME", &root);
            env::set_var("XDG_CONFIG_HOME", &xdg);
        }

        Self {
            _lock: lock,
            old_home,
            old_xdg_config_home,
        }
    }
}

impl Drop for TestConfigDir {
    fn drop(&mut self) {
        unsafe {
            match &self.old_home {
                Some(value) => env::set_var("HOME", value),
                None => env::remove_var("HOME"),
            }
            match &self.old_xdg_config_home {
                Some(value) => env::set_var("XDG_CONFIG_HOME", value),
                None => env::remove_var("XDG_CONFIG_HOME"),
            }
        }
    }
}
