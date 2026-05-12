use std::io::{self, Write};

use serde_json::{Map, Value, json};

use crate::approval::Approval;
use crate::cli::OutputFormat;
use crate::commands::{CommandExecution, CommandOutput, EventStream};
use crate::error::AppError;
use crate::pending_approval::persist_pending_approval;
use crate::protocol::events::{ProtocolEvent, ProtocolEventEnvelope};

pub async fn print_execution(execution: CommandExecution) -> Result<(), AppError> {
    match execution {
        CommandExecution::Final(output) => print(&output),
        CommandExecution::Watch(stream) => print_watch_stream(*stream).await,
    }
}

pub fn print_error(err: &AppError) -> Result<(), AppError> {
    if let AppError::ApprovalRequired { envelope, .. } = err {
        persist_pending_approval(&envelope.approval)?;
        let stdout = io::stdout();
        let mut handle = stdout.lock();
        let rendered = serde_json::to_string(&envelope.to_json_value()).map_err(AppError::json)?;
        writeln!(handle, "{rendered}").map_err(AppError::stdout)?;
    }
    Ok(())
}
pub fn print(output: &CommandOutput) -> Result<(), AppError> {
    let stdout = io::stdout();
    let mut handle = stdout.lock();

    let envelope = json!({
        "ok": true,
        "command": output.command,
        "session": output.session,
        "data": output.data,
        "meta": output.meta,
    });

    let rendered = match detect_format(output) {
        OutputFormat::Json if should_pretty_print(output) => {
            serde_json::to_string_pretty(&envelope)
        }
        OutputFormat::Json => serde_json::to_string(&envelope),
        OutputFormat::Jsonl => serde_json::to_string(&envelope),
        OutputFormat::Text => serde_json::to_string_pretty(&envelope),
    }
    .map_err(AppError::json)?;

    writeln!(handle, "{rendered}").map_err(AppError::stdout)
}

async fn print_watch_stream(mut stream: EventStream) -> Result<(), AppError> {
    let mut last_sequence = 0;

    for event in &stream.buffered_events {
        print_event_jsonl(event)?;
        last_sequence = event.sequence;
        if is_terminal_event(event) {
            stream.connection.close().await?;
            return Ok(());
        }
    }

    loop {
        match stream.connection.next_event().await {
            Ok(event) => {
                print_event_jsonl(&event)?;
                last_sequence = event.sequence;
                if is_terminal_event(&event) {
                    break;
                }
            }
            Err(err) => {
                print_error_jsonl(last_sequence + 1, stream.command, &err)?;
                if stream.connection.state().transport_open {
                    let _ = stream.connection.close().await;
                }
                return Ok(());
            }
        }
    }

    stream.connection.close().await?;
    Ok(())
}

pub fn print_event_jsonl(event: &ProtocolEventEnvelope) -> Result<(), AppError> {
    let stdout = io::stdout();
    let mut handle = stdout.lock();
    let rendered = serde_json::to_string(&event_json_value(event)).map_err(AppError::json)?;
    writeln!(handle, "{rendered}").map_err(AppError::stdout)
}

fn print_error_jsonl(sequence: u64, command: &str, err: &AppError) -> Result<(), AppError> {
    let stdout = io::stdout();
    let mut handle = stdout.lock();
    let rendered = serde_json::to_string(&terminal_error_json_value(sequence, command, err))
        .map_err(AppError::json)?;
    writeln!(handle, "{rendered}").map_err(AppError::stdout)
}

fn event_json_value(event: &ProtocolEventEnvelope) -> Value {
    let mut object = Map::new();
    object.insert("sequence".to_owned(), json!(event.sequence));

    match &event.event {
        ProtocolEvent::ThreadStarted {
            raw_method,
            thread_id,
            data,
        } => {
            base_fields(&mut object, "thread.started", raw_method, data);
            optional_string(&mut object, "thread_id", thread_id.clone());
        }
        ProtocolEvent::TurnStarted {
            raw_method,
            thread_id,
            turn_id,
            data,
        }
        | ProtocolEvent::TurnCompleted {
            raw_method,
            thread_id,
            turn_id,
            data,
        } => {
            let event_type = if matches!(event.event, ProtocolEvent::TurnStarted { .. }) {
                "turn.started"
            } else {
                "turn.completed"
            };
            base_fields(&mut object, event_type, raw_method, data);
            optional_string(&mut object, "thread_id", thread_id.clone());
            optional_string(&mut object, "turn_id", turn_id.clone());
        }
        ProtocolEvent::Delta {
            raw_method,
            item_id,
            text,
            data,
        } => {
            base_fields(&mut object, delta_event_type(raw_method), raw_method, data);
            optional_string(&mut object, "item_id", item_id.clone());
            optional_string(&mut object, "delta", text.clone());
        }
        ProtocolEvent::ItemCompleted {
            raw_method,
            item_id,
            data,
        } => {
            base_fields(&mut object, "item.completed", raw_method, data);
            optional_string(&mut object, "item_id", item_id.clone());
        }
        ProtocolEvent::ThreadUpdated {
            raw_method,
            thread_id,
            data,
        } => {
            base_fields(&mut object, "thread.updated", raw_method, data);
            optional_string(&mut object, "thread_id", thread_id.clone());
        }
        ProtocolEvent::ApprovalRequested {
            raw_method,
            request_id,
            item_id,
            data,
        } => {
            let approval =
                Approval::from_request_parts(raw_method, request_id, item_id.as_ref(), data, None);
            base_fields(&mut object, "approval.requested", raw_method, data);
            object.insert("request_id".to_owned(), json!(request_id));
            object.insert("approval_id".to_owned(), json!(approval.approval_id));
            object.insert("scope".to_owned(), json!(approval.scope));
            object.insert("resume_token".to_owned(), json!(approval.resume_token));
            object.insert("risk_traits".to_owned(), json!(approval.risk_traits));
            object.insert("summary".to_owned(), json!(approval.summary));
            optional_string(&mut object, "item_id", item_id.clone());
        }
        ProtocolEvent::Error {
            raw_method,
            message,
            data,
        } => {
            base_fields(&mut object, "error", raw_method, data);
            object.insert("message".to_owned(), Value::String(message.clone()));
        }
        ProtocolEvent::Unknown { raw_method, data } => {
            base_fields(&mut object, "protocol.unknown", raw_method, data);
        }
    }

    Value::Object(object)
}

fn terminal_error_json_value(sequence: u64, command: &str, err: &AppError) -> Value {
    json!({
        "sequence": sequence,
        "type": "error",
        "command": command,
        "error": {
            "code": stable_error_code(err),
            "message": err.to_string(),
            "details": error_details(err),
        }
    })
}

fn base_fields(object: &mut Map<String, Value>, event_type: &str, raw_method: &str, data: &Value) {
    object.insert("type".to_owned(), Value::String(event_type.to_owned()));
    object.insert(
        "protocol_method".to_owned(),
        Value::String(raw_method.to_owned()),
    );
    object.insert("data".to_owned(), data.clone());
}

fn optional_string(object: &mut Map<String, Value>, key: &str, value: Option<String>) {
    if let Some(value) = value {
        object.insert(key.to_owned(), Value::String(value));
    }
}

fn delta_event_type(raw_method: &str) -> &'static str {
    match raw_method {
        "item/agentMessage/delta" => "item.agent_message.delta",
        "item/plan/delta" => "item.plan.delta",
        "item/fileChange/outputDelta" => "item.file_change.output_delta",
        "item/fileChange/patchUpdated" => "item.file_change.patch_updated",
        "item/reasoning/summaryTextDelta" => "item.reasoning.summary_text_delta",
        "item/reasoning/textDelta" => "item.reasoning.text_delta",
        "command/exec/outputDelta" | "item/commandExecution/outputDelta" => {
            "item.command_execution.output_delta"
        }
        _ => "item.delta",
    }
}

fn stable_error_code(err: &AppError) -> &'static str {
    match err {
        AppError::ConfigIo { .. } | AppError::ConfigParse { .. } => "local.config",
        AppError::Stdout(_) => "io.stdout",
        AppError::Json(_) => "serialization.json",
        AppError::TracingInit(_) => "tracing.init",
        AppError::UnsupportedTransport { .. } => "transport.unsupported",
        AppError::Connection { .. } => "connection.failure",
        AppError::Authentication { .. } => "authentication.failure",
        AppError::Protocol { .. } => "protocol.failure",
        AppError::ApprovalRequired { .. } => "approval_required",
    }
}

fn error_details(err: &AppError) -> Value {
    match err {
        AppError::ConfigIo { path, source } => json!({"path": path, "source": source.to_string()}),
        AppError::ConfigParse { path, source } => {
            json!({"path": path, "source": source.to_string()})
        }
        AppError::Stdout(source) => json!({"source": source.to_string()}),
        AppError::Json(source) => json!({"source": source.to_string()}),
        AppError::TracingInit(detail) => json!({"detail": detail}),
        AppError::UnsupportedTransport { transport, detail } => {
            json!({"transport": transport, "detail": detail})
        }
        AppError::Connection { phase, detail }
        | AppError::Authentication { phase, detail }
        | AppError::Protocol { phase, detail } => json!({"phase": phase, "detail": detail}),
        AppError::ApprovalRequired { envelope, message } => {
            json!({"message": message, "approval": envelope.approval})
        }
    }
}

fn is_terminal_event(event: &ProtocolEventEnvelope) -> bool {
    matches!(
        event.event,
        ProtocolEvent::TurnCompleted { .. } | ProtocolEvent::Error { .. }
    )
}

fn detect_format(output: &CommandOutput) -> OutputFormat {
    output
        .meta
        .get("resolved_config")
        .and_then(|config| config.get("output"))
        .and_then(|output| output.get("format"))
        .and_then(|value| serde_json::from_value(value.clone()).ok())
        .unwrap_or(OutputFormat::Json)
}

fn should_pretty_print(output: &CommandOutput) -> bool {
    output
        .meta
        .get("resolved_config")
        .and_then(|config| config.get("output"))
        .and_then(|output| output.get("pretty"))
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
}
