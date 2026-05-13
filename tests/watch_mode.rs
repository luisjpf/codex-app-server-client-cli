use std::process::Command;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::Message;

const JSONRPC_VERSION: &str = "2.0";

#[tokio::test(flavor = "multi_thread")]
async fn turns_start_watch_emits_stable_jsonl_and_terminal_completion_event() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind listener");
    let addr = listener.local_addr().expect("listener addr");

    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept connection");
        let mut socket = accept_async(stream).await.expect("accept websocket");

        let initialize = read_json(&mut socket).await;
        assert_eq!(initialize["method"], "initialize");
        let initialize_id = initialize["id"].clone();

        write_json(
            &mut socket,
            json!({
                "jsonrpc": JSONRPC_VERSION,
                "id": initialize_id,
                "result": {
                    "codexHome": "/tmp/codex-home",
                    "platformFamily": "unix",
                    "platformOs": "macos",
                    "userAgent": "codex-app-server/0.test"
                }
            }),
        )
        .await;

        let initialized = read_json(&mut socket).await;
        assert_eq!(initialized["method"], "initialized");

        let turn_start = read_json(&mut socket).await;
        assert_eq!(turn_start["method"], "turn/start");
        assert_eq!(turn_start["params"]["threadId"], "thread-1");
        assert_eq!(turn_start["params"]["input"], "hello from watch mode");
        let turn_start_id = turn_start["id"].clone();

        write_json(
            &mut socket,
            json!({
                "jsonrpc": JSONRPC_VERSION,
                "method": "turn/started",
                "params": {"threadId": "thread-1", "turnId": "turn-1"}
            }),
        )
        .await;
        write_json(
            &mut socket,
            json!({
                "jsonrpc": JSONRPC_VERSION,
                "method": "item/agentMessage/delta",
                "params": {"itemId": "item-1", "delta": "Hello"}
            }),
        )
        .await;
        write_json(
            &mut socket,
            json!({
                "jsonrpc": JSONRPC_VERSION,
                "id": turn_start_id,
                "result": {"turnId": "turn-1", "status": "accepted"}
            }),
        )
        .await;
        write_json(
            &mut socket,
            json!({
                "jsonrpc": JSONRPC_VERSION,
                "method": "turn/completed",
                "params": {"threadId": "thread-1", "turnId": "turn-1", "status": "completed"}
            }),
        )
        .await;

        let close = socket.next().await.expect("close frame present");
        assert!(matches!(close, Ok(Message::Close(_))));
    });

    let output = tokio::task::spawn_blocking(move || {
        Command::new(env!("CARGO_BIN_EXE_codex-app-server-client-cli"))
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

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    let lines: Vec<Value> = stdout
        .lines()
        .map(|line| serde_json::from_str(line).expect("valid jsonl line"))
        .collect();

    assert_eq!(lines.len(), 3, "stdout was: {stdout}");
    assert_eq!(lines[0]["type"], "turn.started");
    assert_eq!(lines[0]["sequence"], 1);
    assert_eq!(lines[0]["protocol_method"], "turn/started");
    assert_eq!(lines[0]["thread_id"], "thread-1");
    assert_eq!(lines[0]["turn_id"], "turn-1");

    assert_eq!(lines[1]["type"], "item.agent_message.delta");
    assert_eq!(lines[1]["sequence"], 2);
    assert_eq!(lines[1]["protocol_method"], "item/agentMessage/delta");
    assert_eq!(lines[1]["item_id"], "item-1");
    assert_eq!(lines[1]["delta"], "Hello");

    assert_eq!(lines[2]["type"], "turn.completed");
    assert_eq!(lines[2]["sequence"], 3);
    assert_eq!(lines[2]["protocol_method"], "turn/completed");
    assert_eq!(lines[2]["thread_id"], "thread-1");
    assert_eq!(lines[2]["turn_id"], "turn-1");

    assert!(String::from_utf8_lossy(&output.stderr).trim().is_empty());

    server.await.expect("server task completes");
}

#[tokio::test(flavor = "multi_thread")]
async fn turns_start_watch_treats_error_event_as_terminal_jsonl_output() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind listener");
    let addr = listener.local_addr().expect("listener addr");

    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept connection");
        let mut socket = accept_async(stream).await.expect("accept websocket");

        let initialize = read_json(&mut socket).await;
        let initialize_id = initialize["id"].clone();
        write_json(
            &mut socket,
            json!({
                "jsonrpc": JSONRPC_VERSION,
                "id": initialize_id,
                "result": {
                    "codexHome": "/tmp/codex-home",
                    "platformFamily": "unix",
                    "platformOs": "macos",
                    "userAgent": "codex-app-server/0.test"
                }
            }),
        )
        .await;

        let initialized = read_json(&mut socket).await;
        assert_eq!(initialized["method"], "initialized");

        let turn_start = read_json(&mut socket).await;
        let turn_start_id = turn_start["id"].clone();

        write_json(
            &mut socket,
            json!({
                "jsonrpc": JSONRPC_VERSION,
                "method": "turn/started",
                "params": {"threadId": "thread-1", "turnId": "turn-1"}
            }),
        )
        .await;
        write_json(
            &mut socket,
            json!({
                "jsonrpc": JSONRPC_VERSION,
                "method": "error",
                "params": {"message": "turn failed", "code": "server.turn_failed"}
            }),
        )
        .await;
        write_json(
            &mut socket,
            json!({
                "jsonrpc": JSONRPC_VERSION,
                "id": turn_start_id,
                "result": {"turnId": "turn-1", "status": "failed"}
            }),
        )
        .await;

        let close = socket.next().await.expect("close frame present");
        assert!(matches!(close, Ok(Message::Close(_))));
    });

    let output = tokio::task::spawn_blocking(move || {
        Command::new(env!("CARGO_BIN_EXE_codex-app-server-client-cli"))
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

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    let lines: Vec<Value> = stdout
        .lines()
        .map(|line| serde_json::from_str(line).expect("valid jsonl line"))
        .collect();

    assert_eq!(lines.len(), 2, "stdout was: {stdout}");
    assert_eq!(lines[0]["type"], "turn.started");
    assert_eq!(lines[1]["type"], "error");
    assert_eq!(lines[1]["sequence"], 2);
    assert_eq!(lines[1]["protocol_method"], "error");
    assert_eq!(lines[1]["message"], "turn failed");
    assert_eq!(lines[1]["data"]["code"], "server.turn_failed");

    server.await.expect("server task completes");
}

#[tokio::test(flavor = "multi_thread")]
async fn turns_start_watch_treats_transport_drop_as_terminal_jsonl_error_without_stderr_noise() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind listener");
    let addr = listener.local_addr().expect("listener addr");

    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept connection");
        let mut socket = accept_async(stream).await.expect("accept websocket");

        let initialize = read_json(&mut socket).await;
        let initialize_id = initialize["id"].clone();
        write_json(
            &mut socket,
            json!({
                "jsonrpc": JSONRPC_VERSION,
                "id": initialize_id,
                "result": {
                    "codexHome": "/tmp/codex-home",
                    "platformFamily": "unix",
                    "platformOs": "macos",
                    "userAgent": "codex-app-server/0.test"
                }
            }),
        )
        .await;

        let initialized = read_json(&mut socket).await;
        assert_eq!(initialized["method"], "initialized");

        let turn_start = read_json(&mut socket).await;
        let turn_start_id = turn_start["id"].clone();

        write_json(
            &mut socket,
            json!({
                "jsonrpc": JSONRPC_VERSION,
                "method": "turn/started",
                "params": {"threadId": "thread-1", "turnId": "turn-1"}
            }),
        )
        .await;
        write_json(
            &mut socket,
            json!({
                "jsonrpc": JSONRPC_VERSION,
                "id": turn_start_id,
                "result": {"turnId": "turn-1", "status": "accepted"}
            }),
        )
        .await;

        socket.close(None).await.expect("server closes websocket");
    });

    let output = tokio::task::spawn_blocking(move || {
        Command::new(env!("CARGO_BIN_EXE_codex-app-server-client-cli"))
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

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stderr).trim().is_empty());

    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    let lines: Vec<Value> = stdout
        .lines()
        .map(|line| serde_json::from_str(line).expect("valid jsonl line"))
        .collect();

    assert_eq!(lines.len(), 2, "stdout was: {stdout}");
    assert_eq!(lines[0]["type"], "turn.started");
    assert_eq!(lines[0]["sequence"], 1);
    assert_eq!(lines[1]["type"], "error");
    assert_eq!(lines[1]["sequence"], 2);
    assert_eq!(lines[1]["error"]["code"], "connection.failure");
    assert_eq!(
        lines[1]["error"]["message"],
        "connection failure during event: websocket closed while reading server message: None"
    );
    assert_eq!(lines[1]["error"]["details"]["phase"], "event");

    server.await.expect("server task completes");
}

async fn read_json(
    socket: &mut tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
) -> Value {
    let frame = socket
        .next()
        .await
        .expect("frame present")
        .expect("frame ok");
    let text = frame.into_text().expect("text frame");
    serde_json::from_str(&text).expect("valid json frame")
}

async fn write_json(
    socket: &mut tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
    value: Value,
) {
    socket
        .send(Message::Text(value.to_string().into()))
        .await
        .expect("send json frame");
}
