use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::protocol::messages::RequestId;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProtocolEventEnvelope {
    pub sequence: u64,
    #[serde(flatten)]
    pub event: ProtocolEvent,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProtocolEvent {
    ThreadStarted {
        raw_method: String,
        thread_id: Option<String>,
        data: Value,
    },
    TurnStarted {
        raw_method: String,
        thread_id: Option<String>,
        turn_id: Option<String>,
        data: Value,
    },
    TurnCompleted {
        raw_method: String,
        thread_id: Option<String>,
        turn_id: Option<String>,
        data: Value,
    },
    Delta {
        raw_method: String,
        item_id: Option<String>,
        text: Option<String>,
        data: Value,
    },
    ItemCompleted {
        raw_method: String,
        item_id: Option<String>,
        data: Value,
    },
    ThreadUpdated {
        raw_method: String,
        thread_id: Option<String>,
        data: Value,
    },
    ApprovalRequested {
        raw_method: String,
        request_id: RequestId,
        item_id: Option<String>,
        data: Value,
    },
    Error {
        raw_method: String,
        message: String,
        data: Value,
    },
    Unknown {
        raw_method: String,
        data: Value,
    },
}

pub fn normalize_server_message(
    sequence: u64,
    raw_method: String,
    request_id: Option<RequestId>,
    params: Value,
) -> ProtocolEventEnvelope {
    let event = match raw_method.as_str() {
        "thread/started" => ProtocolEvent::ThreadStarted {
            raw_method,
            thread_id: find_string(&params, &["threadId", "thread_id", "id"]),
            data: params,
        },
        "turn/started" => ProtocolEvent::TurnStarted {
            raw_method,
            thread_id: find_string(&params, &["threadId", "thread_id"]),
            turn_id: find_string(&params, &["turnId", "turn_id", "id"]),
            data: params,
        },
        "turn/completed" => ProtocolEvent::TurnCompleted {
            raw_method,
            thread_id: find_string(&params, &["threadId", "thread_id"]),
            turn_id: find_string(&params, &["turnId", "turn_id", "id"]),
            data: params,
        },
        "item/agentMessage/delta"
        | "item/plan/delta"
        | "item/fileChange/outputDelta"
        | "item/fileChange/patchUpdated"
        | "item/reasoning/summaryTextDelta"
        | "item/reasoning/textDelta"
        | "command/exec/outputDelta"
        | "item/commandExecution/outputDelta" => ProtocolEvent::Delta {
            raw_method,
            item_id: find_string(&params, &["itemId", "item_id", "id"]),
            text: find_string(
                &params,
                &["delta", "text", "summaryTextDelta", "outputDelta", "patch"],
            ),
            data: params,
        },
        "item/completed" => ProtocolEvent::ItemCompleted {
            raw_method,
            item_id: find_string(&params, &["itemId", "item_id", "id"]),
            data: params,
        },
        "thread/name/updated" | "thread/status/changed" => ProtocolEvent::ThreadUpdated {
            raw_method,
            thread_id: find_string(&params, &["threadId", "thread_id", "id"]),
            data: params,
        },
        "error" => ProtocolEvent::Error {
            raw_method,
            message: find_string(&params, &["message", "error"]).unwrap_or_else(|| {
                "server emitted error notification without a message field".to_owned()
            }),
            data: params,
        },
        method if method.ends_with("/requestApproval") => ProtocolEvent::ApprovalRequested {
            raw_method,
            request_id: request_id.unwrap_or(RequestId::String("approval-request".to_owned())),
            item_id: find_string(&params, &["itemId", "item_id", "id"]),
            data: params,
        },
        _ => ProtocolEvent::Unknown {
            raw_method,
            data: params,
        },
    };

    ProtocolEventEnvelope { sequence, event }
}

fn find_string(value: &Value, keys: &[&str]) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Object(map) => {
            for key in keys {
                if let Some(text) = map.get(*key).and_then(Value::as_str) {
                    return Some(text.to_owned());
                }
            }

            for nested in map.values() {
                if let Some(text) = find_string(nested, keys) {
                    return Some(text);
                }
            }

            None
        }
        Value::Array(items) => items.iter().find_map(|item| find_string(item, keys)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn normalizes_custom_request_approval_methods_as_approval_requests() {
        let envelope = normalize_server_message(
            7,
            "item/custom/requestApproval".to_owned(),
            Some(RequestId::String("approval-custom-1".to_owned())),
            json!({
                "itemId": "item-custom-1",
                "summary": "Confirm risky external action",
                "requestedAction": "confirm external action"
            }),
        );

        assert_eq!(envelope.sequence, 7);
        assert!(matches!(
            envelope.event,
            ProtocolEvent::ApprovalRequested {
                ref raw_method,
                ref request_id,
                ref item_id,
                ..
            } if raw_method == "item/custom/requestApproval"
                && request_id == &RequestId::String("approval-custom-1".to_owned())
                && item_id.as_deref() == Some("item-custom-1")
        ));
    }
}
