use std::process::Command;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::Message;

const JSONRPC_VERSION: &str = "2.0";

#[tokio::test(flavor = "multi_thread")]
async fn run_binary_prints_session_metadata_in_final_json_envelope() {
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

        let list = read_json(&mut socket).await;
        assert_eq!(list["method"], "thread/list");
        let list_id = list["id"].clone();
        write_json(
            &mut socket,
            json!({
                "jsonrpc": JSONRPC_VERSION,
                "id": list_id,
                "result": {
                    "threads": []
                }
            }),
        )
        .await;

        let start = read_json(&mut socket).await;
        assert_eq!(start["method"], "thread/start");
        let start_id = start["id"].clone();
        write_json(
            &mut socket,
            json!({
                "jsonrpc": JSONRPC_VERSION,
                "id": start_id,
                "result": {
                    "id": "sess_cli",
                    "name": "feature-auth",
                    "cwd": "/tmp/repo",
                    "ephemeral": false,
                    "yoloMode": false
                }
            }),
        )
        .await;

        let turn = read_json(&mut socket).await;
        assert_eq!(turn["method"], "turn/start");
        let turn_id = turn["id"].clone();
        write_json(
            &mut socket,
            json!({
                "jsonrpc": JSONRPC_VERSION,
                "id": turn_id,
                "result": {
                    "turnId": "turn_cli",
                    "output": {
                        "summary": "cli completed"
                    }
                }
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
                "run",
                "summarize the workspace",
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
    let envelope: Value = serde_json::from_str(stdout.trim()).expect("valid json envelope");

    assert_eq!(envelope["ok"], true);
    assert_eq!(envelope["command"], "run");
    assert_eq!(envelope["session"]["id"], "sess_cli");
    assert_eq!(envelope["session"]["alias"], "feature-auth");
    assert_eq!(envelope["data"]["turn_id"], "turn_cli");
    assert_eq!(envelope["data"]["output"]["summary"], "cli completed");
    assert!(envelope["data"].get("response").is_none());

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
        .send(Message::Text(value.to_string()))
        .await
        .expect("send json frame");
}
