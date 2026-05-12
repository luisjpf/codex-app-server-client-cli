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
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::net::TcpListener;
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::Message;

const JSONRPC_VERSION: &str = "2.0";
static ENV_LOCK: Mutex<()> = Mutex::new(());

#[tokio::test(flavor = "multi_thread")]
async fn handshake_frames_match_v1_fixture() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind listener");
    let addr = listener.local_addr().expect("listener addr");

    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept connection");
        let mut socket = accept_async(stream).await.expect("accept websocket");

        let initialize = next_request(&mut socket).await;
        assert_json_eq(
            &request_to_value(&initialize),
            &fixture_json("handshake/initialize_request.json"),
        );
        send_result(
            &mut socket,
            initialize.id,
            json!({
                "codexHome": "/tmp/codex-home",
                "platformFamily": "unix",
                "platformOs": "macos",
                "userAgent": "codex-app-server/0.test"
            }),
        )
        .await;

        let initialized = read_notification(&mut socket).await;
        assert_json_eq(
            &notification_to_value(&initialized),
            &fixture_json("handshake/initialized_notification.json"),
        );

        expect_close(&mut socket).await;
    });

    let output = tokio::task::spawn_blocking(move || {
        ProcessCommand::new(env!("CARGO_BIN_EXE_codex-app-server-client-cli"))
            .args(["--url", &format!("ws://{addr}"), "health"])
            .output()
            .expect("run binary")
    })
    .await
    .expect("join binary task");

    assert!(output.status.success(), "stderr: {}", String::from_utf8_lossy(&output.stderr));
    server.await.expect("server task completes");
}

#[tokio::test(flavor = "multi_thread")]
async fn run_final_contract_matches_v1_fixture() {
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind listener");
    let addr = listener.local_addr().expect("listener addr");

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
                "id": "sess_cli",
                "name": "feature-auth",
                "cwd": "/tmp/repo",
                "ephemeral": false,
                "yoloMode": false
            }),
        )
        .await;

        let turn = next_request(&mut socket).await;
        assert_eq!(turn.method, "turn/start");
        send_result(
            &mut socket,
            turn.id,
            json!({
                "turnId": "turn_cli",
                "output": {"summary": "cli completed"}
            }),
        )
        .await;

        expect_close(&mut socket).await;
    });

    let output = tokio::task::spawn_blocking(move || {
        ProcessCommand::new(env!("CARGO_BIN_EXE_codex-app-server-client-cli"))
            .args(["--url", &format!("ws://{addr}"), "run", "summarize the workspace"])
            .output()
            .expect("run binary")
    })
    .await
    .expect("join binary task");

    assert!(output.status.success(), "stderr: {}", String::from_utf8_lossy(&output.stderr));
    let envelope: Value =
        serde_json::from_slice(&output.stdout).expect("parse final json envelope");
    let actual = json!({
        "ok": envelope["ok"],
        "command": envelope["command"],
        "session": normalize_repo_root(envelope["session"].clone(), &repo_root),
        "data": envelope["data"],
        "meta": {
            "session_selection": envelope["meta"]["session_selection"],
            "policy": envelope["meta"]["policy"],
            "server": envelope["meta"]["server"],
        }
    });
    assert_json_eq(&actual, &fixture_json("run/final_envelope_contract.json"));

    server.await.expect("server task completes");
}

#[tokio::test(flavor = "multi_thread")]
async fn approval_required_contract_matches_v1_fixture() {
    let _config_dir = TestConfigDir::install("approval_required_contract_matches_v1_fixture");
    let repo_root = temp_repo("approval_contract");

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind listener");
    let addr = listener.local_addr().expect("listener addr");
    let repo_root_for_server = repo_root.clone();

    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept connection");
        let mut socket = accept_async(stream).await.expect("accept websocket");
        handshake(&mut socket).await;

        let list = next_request(&mut socket).await;
        send_result(&mut socket, list.id, json!({"threads": []})).await;

        let start = next_request(&mut socket).await;
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
        send_notification(
            &mut socket,
            json!({
                "jsonrpc": JSONRPC_VERSION,
                "method": "turn/started",
                "params": {"threadId": "sess_new", "turnId": "turn_approval"}
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
        let _ = turn;

        expect_close(&mut socket).await;
    });

    let output = tokio::task::spawn_blocking({
        let repo_root = repo_root.clone();
        move || {
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
        }
    })
    .await
    .expect("join binary task");

    assert_eq!(output.status.code(), Some(7));
    let mut envelope: Value =
        serde_json::from_slice(&output.stdout).expect("parse approval envelope");
    replace_string(&mut envelope, &repo_root.display().to_string(), "__REPO_ROOT__");
    assert_json_eq(&envelope, &fixture_json("approval/required_envelope.json"));

    server.await.expect("server task completes");
}

#[tokio::test(flavor = "multi_thread")]
async fn resume_contract_matches_v1_fixture() {
    let _config_dir = TestConfigDir::install("resume_contract_matches_v1_fixture");
    let repo_root = temp_repo("resume_contract");

    let approval_listener = TcpListener::bind("127.0.0.1:0").await.expect("bind approval listener");
    let approval_addr = approval_listener.local_addr().expect("approval listener addr");
    let repo_root_for_approval_server = repo_root.clone();

    let approval_server = tokio::spawn(async move {
        let (stream, _) = approval_listener.accept().await.expect("accept connection");
        let mut socket = accept_async(stream).await.expect("accept websocket");
        handshake(&mut socket).await;

        let list = next_request(&mut socket).await;
        send_result(&mut socket, list.id, json!({"threads": []})).await;

        let start = next_request(&mut socket).await;
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

        let _turn = next_request(&mut socket).await;
        send_notification(
            &mut socket,
            json!({
                "jsonrpc": JSONRPC_VERSION,
                "method": "turn/started",
                "params": {"threadId": "sess_new", "turnId": "turn_pending_approval"}
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
    let initial_envelope: Value =
        serde_json::from_slice(&initial_output.stdout).expect("parse initial approval envelope");
    let resume_token = initial_envelope["approval"]["resume_token"]
        .as_str()
        .expect("resume token string")
        .to_owned();

    approval_server.await.expect("approval server completes");

    let resume_listener = TcpListener::bind("127.0.0.1:0").await.expect("bind resume listener");
    let resume_addr = resume_listener.local_addr().expect("resume listener addr");
    let repo_root_for_resume_server = repo_root.clone();
    let resume_token_for_server = resume_token.clone();

    let resume_server = tokio::spawn(async move {
        let (stream, _) = resume_listener.accept().await.expect("accept resume connection");
        let mut socket = accept_async(stream).await.expect("accept websocket");
        handshake(&mut socket).await;

        let resume = next_request(&mut socket).await;
        assert_eq!(resume.method, "thread/resume");
        assert_eq!(resume.params.get("threadId"), Some(&json!("sess_new")));

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

        send_result(
            &mut socket,
            resume.id,
            json!({
                "id": "sess_new",
                "name": "feature-auth",
                "cwd": repo_root_for_resume_server,
                "ephemeral": false,
                "yoloMode": false,
                "turnId": "turn_resumed",
                "output": {"summary": "approval resumed"}
            }),
        )
        .await;

        expect_close(&mut socket).await;
        let _ = resume_token_for_server;
    });

    let resumed_output = tokio::task::spawn_blocking({
        let repo_root = repo_root.clone();
        let resume_token = resume_token.clone();
        move || {
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
                .expect("run resume binary")
        }
    })
    .await
    .expect("join resume binary task");

    assert!(resumed_output.status.success());
    let envelope: Value =
        serde_json::from_slice(&resumed_output.stdout).expect("parse resumed envelope");
    let actual = json!({
        "ok": envelope["ok"],
        "command": envelope["command"],
        "session": normalize_repo_root(envelope["session"].clone(), &repo_root),
        "data": normalize_repo_root(envelope["data"].clone(), &repo_root),
        "meta": {
            "session_selection": envelope["meta"]["session_selection"],
            "policy": envelope["meta"]["policy"],
            "server": envelope["meta"]["server"],
        }
    });
    assert_json_eq(&actual, &fixture_json("resume/final_envelope_contract.json"));

    resume_server.await.expect("resume server completes");
}

#[tokio::test(flavor = "multi_thread")]
async fn watch_contract_matches_v1_fixture() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind listener");
    let addr = listener.local_addr().expect("listener addr");

    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept connection");
        let mut socket = accept_async(stream).await.expect("accept websocket");
        handshake(&mut socket).await;

        let turn_start = next_request(&mut socket).await;
        assert_eq!(turn_start.method, "turn/start");
        let turn_start_id = turn_start.id.clone();

        send_notification(
            &mut socket,
            json!({
                "jsonrpc": JSONRPC_VERSION,
                "method": "turn/started",
                "params": {"threadId": "thread-1", "turnId": "turn-1"}
            }),
        )
        .await;
        send_notification(
            &mut socket,
            json!({
                "jsonrpc": JSONRPC_VERSION,
                "method": "item/agentMessage/delta",
                "params": {"itemId": "item-1", "delta": "Hello"}
            }),
        )
        .await;
        send_result(
            &mut socket,
            turn_start_id,
            json!({"turnId": "turn-1", "status": "accepted"}),
        )
        .await;
        send_notification(
            &mut socket,
            json!({
                "jsonrpc": JSONRPC_VERSION,
                "method": "turn/completed",
                "params": {"threadId": "thread-1", "turnId": "turn-1", "status": "completed"}
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
                "turns",
                "start",
                "--thread-id",
                "thread-1",
                "--prompt",
                "hello from watch mode",
                "--watch",
            ])
            .output()
            .expect("run binary")
    })
    .await
    .expect("join binary task");

    assert!(output.status.success());
    let lines = parse_jsonl(&output.stdout);
    assert_json_eq(&Value::Array(lines), &fixture_json("watch/events.json"));

    server.await.expect("server task completes");
}

#[tokio::test]
async fn explicit_ephemeral_flow_matches_v1_fixture() {
    let repo_root = temp_repo("ephemeral_contract");
    let nested = repo_root.join("src/bin");
    fs::create_dir_all(&nested).expect("nested dir should exist");

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind listener");
    let addr = listener.local_addr().expect("listener addr");
    let repo_root_for_server = repo_root.clone();

    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept connection");
        let mut socket = accept_async(stream).await.expect("accept websocket");
        handshake(&mut socket).await;

        let list = next_request(&mut socket).await;
        send_result(
            &mut socket,
            list.id,
            json!({
                "threads": [{
                    "id": "sess_existing",
                    "name": "feature-auth",
                    "cwd": repo_root_for_server,
                    "updatedAt": "2026-05-12T00:00:00Z",
                    "ephemeral": false,
                    "yoloMode": false
                }]
            }),
        )
        .await;

        let start = next_request(&mut socket).await;
        let start_params = start.params.clone();
        send_result(
            &mut socket,
            start.id,
            json!({
                "id": "sess_ephemeral",
                "cwd": repo_root_for_server,
                "ephemeral": true,
                "yoloMode": false
            }),
        )
        .await;

        let turn = next_request(&mut socket).await;
        send_result(
            &mut socket,
            turn.id,
            json!({
                "turnId": "turn_ephemeral",
                "output": {"summary": "ephemeral session created"}
            }),
        )
        .await;

        expect_close(&mut socket).await;
        start_params
    });

    let cli = Cli::parse_from([
        "bin",
        "--url",
        &format!("ws://{addr}"),
        "--cwd",
        nested.to_str().expect("nested path utf8"),
        "run",
        "scratch this",
        "--ephemeral",
    ]);
    let config = load_config_locked(&cli);
    let output = expect_final(execute(cli.command, config).await.expect("run should succeed"));
    let start_params = server.await.expect("server task completes");

    let actual = json!({
        "thread_start_params": normalize_repo_root(start_params, &repo_root),
        "session": normalize_repo_root(output.session.expect("session envelope"), &repo_root),
        "meta": {
            "session_selection": output.meta["session_selection"],
            "policy": output.meta["policy"],
        }
    });
    assert_json_eq(&actual, &fixture_json("run/ephemeral_contract.json"));
}

#[tokio::test]
async fn session_scoped_yolo_visibility_matches_v1_fixture() {
    let repo_root = temp_repo("yolo_contract");

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind listener");
    let addr = listener.local_addr().expect("listener addr");
    let repo_root_for_server = repo_root.clone();

    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept connection");
        let mut socket = accept_async(stream).await.expect("accept websocket");
        handshake(&mut socket).await;

        let list = next_request(&mut socket).await;
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
        let resume_params = resume.params.clone();
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
        let turn_params = turn.params.clone();
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
        (resume_params, turn_params)
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
    let config = load_config_locked(&cli);
    let output = expect_final(execute(cli.command, config).await.expect("run should succeed"));
    let (resume_params, turn_params) = server.await.expect("server task completes");

    let actual = json!({
        "thread_resume_params": normalize_repo_root(resume_params, &repo_root),
        "turn_start_params": normalize_repo_root(turn_params, &repo_root),
        "session": normalize_repo_root(output.session.expect("session envelope"), &repo_root),
        "meta": {
            "session_selection": output.meta["session_selection"],
            "policy": output.meta["policy"],
        }
    });
    assert_json_eq(&actual, &fixture_json("run/session_yolo_contract.json"));
}

fn fixture_json(name: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("protocol")
        .join(name);
    let raw = fs::read_to_string(&path).unwrap_or_else(|err| panic!("read fixture {}: {err}", path.display()));
    serde_json::from_str(&raw).unwrap_or_else(|err| panic!("parse fixture {}: {err}", path.display()))
}

fn parse_jsonl(stdout: &[u8]) -> Vec<Value> {
    let text = String::from_utf8(stdout.to_vec()).expect("stdout utf8");
    text.lines()
        .map(|line| serde_json::from_str(line).expect("valid jsonl line"))
        .collect()
}

fn request_to_value(request: &JsonRpcRequest) -> Value {
    serde_json::to_value(request).expect("serialize request")
}

fn notification_to_value(notification: &JsonRpcNotification) -> Value {
    serde_json::to_value(notification).expect("serialize notification")
}

async fn read_notification(
    socket: &mut tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
) -> JsonRpcNotification {
    let frame = socket
        .next()
        .await
        .expect("notification frame present")
        .expect("notification frame ok")
        .into_text()
        .expect("notification text");
    serde_json::from_str(&frame).expect("parse notification")
}

fn assert_json_eq(actual: &Value, expected: &Value) {
    assert_eq!(actual, expected, "actual:\n{}\n\nexpected:\n{}", pretty(actual), pretty(expected));
}

fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).expect("pretty json")
}

fn replace_string(value: &mut Value, target: &str, replacement: &str) {
    match value {
        Value::String(current) => {
            if current.contains(target) {
                *current = current.replace(target, replacement);
            }
        }
        Value::Array(items) => {
            for item in items {
                replace_string(item, target, replacement);
            }
        }
        Value::Object(map) => {
            for item in map.values_mut() {
                replace_string(item, target, replacement);
            }
        }
        _ => {}
    }
}

fn normalize_repo_root(value: Value, repo_root: &Path) -> Value {
    let mut value = value;
    if let Ok(canonical) = fs::canonicalize(repo_root) {
        replace_string(&mut value, &canonical.display().to_string(), "__REPO_ROOT__");
    }
    let repo_root_display = repo_root.display().to_string();
    replace_string(&mut value, &repo_root_display, "__REPO_ROOT__");
    value
}

async fn handshake(socket: &mut tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>) {
    let initialize = next_request(socket).await;
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

    let initialized = read_notification(socket).await;
    assert_eq!(initialized.method, "initialized");
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
            json!({"jsonrpc": JSONRPC_VERSION, "id": id, "result": result}).to_string(),
        ))
        .await
        .expect("send result");
}

async fn send_notification(
    socket: &mut tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
    payload: Value,
) {
    socket
        .send(Message::Text(payload.to_string()))
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

fn load_config_locked(cli: &Cli) -> ResolvedConfig {
    let _lock = ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    ResolvedConfig::load(cli).expect("config should load")
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
        let lock = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
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
