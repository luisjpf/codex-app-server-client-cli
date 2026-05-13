use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use clap::Parser;
use codex_app_server_client_cli::approval::{Approval, ApprovalScope, ApprovalStatus};
use codex_app_server_client_cli::cli::Cli;
use codex_app_server_client_cli::commands::{CommandExecution, CommandOutput, execute};
use codex_app_server_client_cli::config::ResolvedConfig;
use codex_app_server_client_cli::pending_approval::{
    list_pending_approvals, load_pending_approval, persist_pending_approval,
};
use codex_app_server_client_cli::protocol::messages::{
    JsonRpcNotification, JsonRpcRequest, RequestId,
};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::Message;

const JSONRPC_VERSION: &str = "2.0";
static ENV_LOCK: Mutex<()> = Mutex::new(());

type TestSocket = tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>;

#[tokio::test(flavor = "current_thread")]
async fn session_list_command_wraps_thread_list_and_returns_normalized_sessions() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind listener");
    let addr = listener.local_addr().expect("listener addr");

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
                        "id": "sess_1",
                        "name": "feature-auth",
                        "cwd": "/tmp/repo",
                        "updatedAt": "2026-05-12T00:00:00Z",
                        "ephemeral": false,
                        "yoloMode": true
                    },
                    {
                        "id": "sess_2",
                        "cwd": "/tmp/other",
                        "updatedAt": "2026-05-12T00:05:00Z",
                        "ephemeral": true,
                        "yoloMode": false
                    }
                ]
            }),
        )
        .await;

        expect_close(&mut socket).await;
    });

    let output = run_final(&["bin", "--url", &format!("ws://{addr}"), "session", "list"]).await;

    assert_eq!(output.command, "session list");
    assert!(output.session.is_none());
    assert_eq!(
        output.data.pointer("/sessions/0/id"),
        Some(&json!("sess_1"))
    );
    assert_eq!(
        output.data.pointer("/sessions/0/alias"),
        Some(&json!("feature-auth"))
    );
    assert_eq!(output.data.pointer("/sessions/0/yolo"), Some(&json!(true)));
    assert_eq!(
        output.data.pointer("/sessions/1/ephemeral"),
        Some(&json!(true))
    );

    server.await.expect("server task should finish");
}

#[tokio::test(flavor = "current_thread")]
async fn session_show_command_uses_thread_read_and_returns_selected_session() {
    let repo_root = temp_dir("session-show-repo");
    fs::create_dir_all(repo_root.join(".git")).expect("create git dir");

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
                    "id": "sess_1",
                    "name": "feature-auth",
                    "cwd": repo_root_for_server,
                    "updatedAt": "2026-05-12T00:00:00Z",
                    "ephemeral": false,
                    "yoloMode": true
                }]
            }),
        )
        .await;

        let read = next_request(&mut socket).await;
        assert_eq!(read.method, "thread/read");
        assert_eq!(read.params.get("threadId"), Some(&json!("sess_1")));
        send_result(
            &mut socket,
            read.id,
            json!({
                "id": "sess_1",
                "name": "feature-auth",
                "cwd": repo_root_for_server,
                "updatedAt": "2026-05-12T00:10:00Z",
                "ephemeral": false,
                "yoloMode": true,
                "turns": [{"id": "turn_1"}]
            }),
        )
        .await;

        expect_close(&mut socket).await;
    });

    let canonical_repo_root = fs::canonicalize(&repo_root).expect("canonical repo root");
    let output = run_final(&[
        "bin",
        "--url",
        &format!("ws://{addr}"),
        "--cwd",
        repo_root.to_str().expect("repo root utf8"),
        "session",
        "show",
        "--alias",
        "feature-auth",
    ])
    .await;

    assert_eq!(output.command, "session show");
    assert_eq!(
        output.session.as_ref().and_then(|value| value.get("id")),
        Some(&json!("sess_1"))
    );
    assert_eq!(
        output
            .session
            .as_ref()
            .and_then(|value| value.get("workspace_root")),
        Some(&json!(canonical_repo_root))
    );
    assert_eq!(
        output.data.pointer("/thread/turns/0/id"),
        Some(&json!("turn_1"))
    );

    server.await.expect("server task should finish");
}

#[tokio::test(flavor = "current_thread")]
async fn session_fork_command_uses_thread_fork_with_exclude_turns() {
    let repo_root = temp_dir("session-fork-repo");
    fs::create_dir_all(repo_root.join(".git")).expect("create git dir");

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
                    "id": "sess_1",
                    "name": "feature-auth",
                    "cwd": repo_root_for_server,
                    "ephemeral": false,
                    "yoloMode": false
                }]
            }),
        )
        .await;

        let fork = next_request(&mut socket).await;
        assert_eq!(fork.method, "thread/fork");
        assert_eq!(fork.params.get("threadId"), Some(&json!("sess_1")));
        assert_eq!(fork.params.get("excludeTurns"), Some(&json!(true)));
        send_result(
            &mut socket,
            fork.id,
            json!({
                "id": "sess_2",
                "name": "feature-auth-fork",
                "cwd": repo_root_for_server,
                "ephemeral": true,
                "yoloMode": true
            }),
        )
        .await;

        expect_close(&mut socket).await;
    });

    let output = run_final(&[
        "bin",
        "--url",
        &format!("ws://{addr}"),
        "--cwd",
        repo_root.to_str().expect("repo root utf8"),
        "session",
        "fork",
        "--id",
        "sess_1",
    ])
    .await;

    assert_eq!(output.command, "session fork");
    assert_eq!(
        output.session.as_ref().and_then(|value| value.get("id")),
        Some(&json!("sess_2"))
    );
    assert_eq!(
        output
            .session
            .as_ref()
            .and_then(|value| value.get("ephemeral")),
        Some(&json!(true))
    );
    assert_eq!(
        output.data.pointer("/thread/name"),
        Some(&json!("feature-auth-fork"))
    );

    server.await.expect("server task should finish");
}

#[tokio::test(flavor = "current_thread")]
async fn models_list_command_wraps_model_list_response() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind listener");
    let addr = listener.local_addr().expect("listener addr");

    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept connection");
        let mut socket = accept_async(stream).await.expect("accept websocket");
        handshake(&mut socket).await;

        let request = next_request(&mut socket).await;
        assert_eq!(request.method, "model/list");
        send_result(
            &mut socket,
            request.id,
            json!({
                "models": [
                    {"id": "gpt-5.4", "context_window": 200000},
                    {"id": "gpt-5-mini", "context_window": 128000}
                ]
            }),
        )
        .await;

        expect_close(&mut socket).await;
    });

    let output = run_final(&["bin", "--url", &format!("ws://{addr}"), "models", "list"]).await;

    assert_eq!(output.command, "models list");
    assert_eq!(output.data.pointer("/models/0/id"), Some(&json!("gpt-5.4")));
    assert_eq!(
        output.data.pointer("/response/models/1/context_window"),
        Some(&json!(128000))
    );

    server.await.expect("server task should finish");
}

#[tokio::test(flavor = "current_thread")]
async fn fs_commands_wrap_directory_and_file_reads() {
    let ls_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ls listener");
    let ls_addr = ls_listener.local_addr().expect("ls listener addr");

    let ls_server = tokio::spawn(async move {
        let (stream, _) = ls_listener.accept().await.expect("accept connection");
        let mut socket = accept_async(stream).await.expect("accept websocket");
        handshake(&mut socket).await;

        let request = next_request(&mut socket).await;
        assert_eq!(request.method, "fs/readDirectory");
        assert_eq!(request.params.get("path"), Some(&json!("/repo/src")));
        send_result(
            &mut socket,
            request.id,
            json!({
                "entries": [
                    {"name": "main.rs", "kind": "file"},
                    {"name": "lib.rs", "kind": "file"}
                ]
            }),
        )
        .await;

        expect_close(&mut socket).await;
    });

    let ls_output = run_final(&[
        "bin",
        "--url",
        &format!("ws://{ls_addr}"),
        "fs",
        "ls",
        "--path",
        "/repo/src",
    ])
    .await;
    assert_eq!(ls_output.command, "fs ls");
    assert_eq!(
        ls_output.data.pointer("/entries/0/name"),
        Some(&json!("main.rs"))
    );

    ls_server.await.expect("ls server task should finish");

    let cat_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind cat listener");
    let cat_addr = cat_listener.local_addr().expect("cat listener addr");

    let cat_server = tokio::spawn(async move {
        let (stream, _) = cat_listener.accept().await.expect("accept connection");
        let mut socket = accept_async(stream).await.expect("accept websocket");
        handshake(&mut socket).await;

        let request = next_request(&mut socket).await;
        assert_eq!(request.method, "fs/readFile");
        assert_eq!(
            request.params.get("path"),
            Some(&json!("/repo/src/main.rs"))
        );
        send_result(
            &mut socket,
            request.id,
            json!({"content": "fn main() {}\n"}),
        )
        .await;

        expect_close(&mut socket).await;
    });

    let cat_output = run_final(&[
        "bin",
        "--url",
        &format!("ws://{cat_addr}"),
        "fs",
        "cat",
        "--path",
        "/repo/src/main.rs",
    ])
    .await;
    assert_eq!(cat_output.command, "fs cat");
    assert_eq!(
        cat_output.data.pointer("/content"),
        Some(&json!("fn main() {}\n"))
    );

    cat_server.await.expect("cat server task should finish");
}

#[tokio::test(flavor = "current_thread")]
async fn health_command_reports_handshake_state() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind listener");
    let addr = listener.local_addr().expect("listener addr");

    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept connection");
        let mut socket = accept_async(stream).await.expect("accept websocket");
        handshake(&mut socket).await;
        expect_close(&mut socket).await;
    });

    let output = run_final(&["bin", "--url", &format!("ws://{addr}"), "health"]).await;

    assert_eq!(output.command, "health");
    assert_eq!(output.data.get("status"), Some(&json!("ok")));
    assert_eq!(
        output.meta.pointer("/server/handshake_complete"),
        Some(&json!(true))
    );
    assert_eq!(
        output.meta.pointer("/server/transport_open"),
        Some(&json!(true))
    );

    server.await.expect("server task should finish");
}

#[tokio::test(flavor = "current_thread")]
async fn approval_list_show_and_no_resume_approve_use_pending_store() {
    let _config = TestConfigDir::install("approval-store");
    let approval = sample_approval("approval-1", "sess_1");
    persist_pending_approval(&approval).expect("persist pending approval");

    let list_output = run_final(&["bin", "approval", "list"]).await;
    assert_eq!(list_output.command, "approval list");
    assert_eq!(
        list_output.data.pointer("/approvals/0/approval_id"),
        Some(&json!("approval-1"))
    );

    let show_output = run_final(&["bin", "approval", "show", "--id", "approval-1"]).await;
    assert_eq!(show_output.command, "approval show");
    assert_eq!(show_output.session, Some(json!({"id": "sess_1"})));
    assert_eq!(
        show_output.data.pointer("/approval/requested_action"),
        Some(&json!("npm test"))
    );

    let approve_output = run_final(&[
        "bin",
        "approval",
        "approve",
        "--id",
        "approval-1",
        "--no-resume",
    ])
    .await;
    assert_eq!(approve_output.command, "approve");
    assert_eq!(approve_output.data.get("status"), Some(&json!("approved")));
    assert_eq!(approve_output.data.get("resumed"), Some(&json!(false)));

    let stored = load_pending_approval("approval-1")
        .expect("load approval after approve")
        .expect("approval remains stored after --no-resume");
    assert_eq!(stored.status, ApprovalStatus::Approved);

    let approvals = list_pending_approvals().expect("list pending approvals after approve");
    assert_eq!(approvals.len(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn approval_deny_command_resolves_pending_approval_and_returns_denied_status() {
    let _config = TestConfigDir::install("approval-deny");
    let approval = sample_approval("approval-2", "sess_denied");
    persist_pending_approval(&approval).expect("persist pending approval");

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind listener");
    let addr = listener.local_addr().expect("listener addr");

    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept connection");
        let mut socket = accept_async(stream).await.expect("accept websocket");
        handshake(&mut socket).await;

        let resume = next_request(&mut socket).await;
        assert_eq!(resume.method, "thread/resume");
        assert_eq!(resume.params.get("threadId"), Some(&json!("sess_denied")));

        send_notification(
            &mut socket,
            json!({
                "jsonrpc": JSONRPC_VERSION,
                "id": "approval-2",
                "method": "item/commandExecution/requestApproval",
                "params": {
                    "itemId": "item-approval",
                    "summary": "Run npm test",
                    "command": ["npm", "test"],
                    "requestedAt": "2026-05-12T00:00:00Z"
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
        assert_eq!(approval_response["id"], json!("approval-2"));
        assert_eq!(approval_response["result"]["approved"], json!(false));
        assert_eq!(approval_response["result"]["resume"], json!(false));

        send_error(&mut socket, resume.id, 4001, "denied by operator").await;
        expect_close(&mut socket).await;
    });

    let output = run_final(&[
        "bin",
        "--url",
        &format!("ws://{addr}"),
        "approval",
        "deny",
        "--id",
        "approval-2",
    ])
    .await;

    assert_eq!(output.command, "deny");
    assert_eq!(output.session, Some(json!({"id": "sess_denied"})));
    assert_eq!(output.data.get("status"), Some(&json!("denied")));
    assert_eq!(output.data.get("resumed"), Some(&json!(false)));
    assert_eq!(
        output.data.pointer("/approval/status"),
        Some(&json!("denied"))
    );
    assert!(
        load_pending_approval("approval-2")
            .expect("load pending approval after deny")
            .is_none()
    );

    server.await.expect("server task should finish");
}

#[tokio::test(flavor = "current_thread")]
async fn approval_approve_command_preserves_resumed_session_yolo_policy_metadata() {
    let _config = TestConfigDir::install("approval-approve-yolo");
    let approval = sample_approval("approval-yolo", "sess_yolo");
    persist_pending_approval(&approval).expect("persist pending approval");

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind listener");
    let addr = listener.local_addr().expect("listener addr");

    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept connection");
        let mut socket = accept_async(stream).await.expect("accept websocket");
        handshake(&mut socket).await;

        let resume = next_request(&mut socket).await;
        assert_eq!(resume.method, "thread/resume");
        assert_eq!(resume.params.get("threadId"), Some(&json!("sess_yolo")));
        assert_eq!(resume.params.get("yoloMode"), None);

        send_notification(
            &mut socket,
            json!({
                "jsonrpc": JSONRPC_VERSION,
                "id": "approval-yolo",
                "method": "item/commandExecution/requestApproval",
                "params": {
                    "itemId": "item-approval",
                    "summary": "Run npm test",
                    "command": ["npm", "test"],
                    "requestedAt": "2026-05-12T00:00:00Z"
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
        assert_eq!(approval_response["id"], json!("approval-yolo"));
        assert_eq!(approval_response["result"]["approved"], json!(true));
        assert_eq!(approval_response["result"]["resume"], json!(true));

        send_result(
            &mut socket,
            resume.id,
            json!({
                "threadId": "sess_yolo",
                "turnId": "turn_yolo",
                "output": {"summary": "approval resumed with yolo"},
                "cwd": "/tmp/repo",
                "ephemeral": false,
                "yoloMode": true
            }),
        )
        .await;

        expect_close(&mut socket).await;
    });

    let output = run_final(&[
        "bin",
        "--url",
        &format!("ws://{addr}"),
        "approval",
        "approve",
        "--id",
        "approval-yolo",
    ])
    .await;

    assert_eq!(output.command, "approve");
    assert_eq!(output.session, Some(json!({"id": "sess_yolo"})));
    assert_eq!(
        output.meta.pointer("/policy/yolo/effective"),
        Some(&json!(true))
    );
    assert_eq!(
        output.meta.pointer("/policy/yolo/session_enabled"),
        Some(&json!(true))
    );
    assert_eq!(
        output.meta.pointer("/policy/yolo/source"),
        Some(&json!("session"))
    );
    assert_eq!(
        output.meta.pointer("/policy/last_approval/decision"),
        Some(&json!("auto_approve_yolo"))
    );

    server.await.expect("server task should finish");
}

async fn run_final(args: &[&str]) -> CommandOutput {
    let cli = Cli::parse_from(args);
    let config = ResolvedConfig::load(&cli).expect("config should load");
    expect_final(
        execute(cli.command, config)
            .await
            .expect("command succeeds"),
    )
}

fn expect_final(execution: CommandExecution) -> CommandOutput {
    match execution {
        CommandExecution::Final(output) => output,
        CommandExecution::Watch(_) => panic!("expected final output"),
    }
}

async fn handshake(socket: &mut TestSocket) {
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

    let initialized = next_notification(socket).await;
    assert_eq!(initialized.method, "initialized");
}

async fn next_request(socket: &mut TestSocket) -> JsonRpcRequest {
    let frame = socket
        .next()
        .await
        .expect("request frame present")
        .expect("request frame ok");
    let text = frame.into_text().expect("request text frame");
    serde_json::from_str::<JsonRpcRequest>(&text).expect("valid request")
}

async fn next_notification(socket: &mut TestSocket) -> JsonRpcNotification {
    let frame = socket
        .next()
        .await
        .expect("notification frame present")
        .expect("notification frame ok");
    let text = frame.into_text().expect("notification text frame");
    serde_json::from_str::<JsonRpcNotification>(&text).expect("valid notification")
}

async fn send_result(socket: &mut TestSocket, id: RequestId, result: Value) {
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

async fn send_error(socket: &mut TestSocket, id: RequestId, code: i64, message: &str) {
    socket
        .send(Message::Text(
            json!({
                "jsonrpc": JSONRPC_VERSION,
                "id": id,
                "error": {
                    "code": code,
                    "message": message,
                }
            })
            .to_string()
            .into(),
        ))
        .await
        .expect("send error");
}

async fn send_notification(socket: &mut TestSocket, payload: Value) {
    socket
        .send(Message::Text(payload.to_string().into()))
        .await
        .expect("send notification");
}

async fn expect_close(socket: &mut TestSocket) {
    let frame = socket
        .next()
        .await
        .expect("close frame present")
        .expect("close frame ok");
    assert!(matches!(frame, Message::Close(_)));
}

fn sample_approval(approval_id: &str, session_id: &str) -> Approval {
    Approval {
        approval_id: approval_id.to_owned(),
        session_id: Some(session_id.to_owned()),
        scope: ApprovalScope::CommandExecution,
        risk_traits: vec!["workspace_write".to_owned()],
        summary: "Run npm test".to_owned(),
        requested_action: "npm test".to_owned(),
        requested_at: "2026-05-12T00:00:00Z".to_owned(),
        expires_at: None,
        resume_token: approval_id.to_owned(),
        status: ApprovalStatus::Pending,
        raw_method: "item/commandExecution/requestApproval".to_owned(),
        request_id: RequestId::String(approval_id.to_owned()),
        item_id: Some("item-approval".to_owned()),
        data: json!({"command": ["npm", "test"]}),
    }
}

fn temp_dir(label: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time after epoch")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("codex-app-server-client-cli-{label}-{unique}"));
    fs::create_dir_all(&path).expect("create temp dir");
    path
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
