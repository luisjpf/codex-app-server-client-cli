use serde::Serialize;
use serde_json::Value;

use crate::error::AppError;
use crate::protocol::events::{ProtocolEventEnvelope, normalize_server_message};
use crate::protocol::messages::{
    JsonRpcIncomingMessage, JsonRpcRequest, JsonRpcServerRequest, RequestId,
};

const JSONRPC_VERSION: &str = "2.0";

#[derive(Debug, Clone)]
pub struct PreparedRequest {
    pub method: String,
    pub request: JsonRpcRequest,
}

#[derive(Debug, Clone, Default)]
pub struct JsonRpcRouter {
    next_request_id: u64,
    next_event_sequence: u64,
}

impl JsonRpcRouter {
    pub fn new(next_request_id: u64) -> Self {
        Self {
            next_request_id,
            next_event_sequence: 1,
        }
    }

    pub fn next_request_id(&self) -> u64 {
        self.next_request_id
    }

    pub fn prepare_request<P: Serialize>(
        &mut self,
        method: impl Into<String>,
        params: &P,
    ) -> Result<PreparedRequest, AppError> {
        let method = method.into();
        let request_id = self.next_request_id;
        self.next_request_id += 1;

        Ok(PreparedRequest {
            method: method.clone(),
            request: JsonRpcRequest {
                jsonrpc: JSONRPC_VERSION.to_owned(),
                id: RequestId::from(request_id),
                method,
                params: serde_json::to_value(params).map_err(AppError::json)?,
            },
        })
    }

    pub fn normalize_message(
        &mut self,
        message: JsonRpcIncomingMessage,
    ) -> Option<ProtocolEventEnvelope> {
        match message {
            JsonRpcIncomingMessage::Notification(notification) => {
                Some(self.normalize_notification(notification.method, notification.params))
            }
            JsonRpcIncomingMessage::ServerRequest(request) => {
                Some(self.normalize_server_request(request))
            }
            JsonRpcIncomingMessage::Success(_) | JsonRpcIncomingMessage::Error(_) => None,
        }
    }

    pub fn normalize_notification(
        &mut self,
        raw_method: String,
        params: Value,
    ) -> ProtocolEventEnvelope {
        let sequence = self.next_sequence();
        normalize_server_message(sequence, raw_method, None, params)
    }

    pub fn normalize_server_request(
        &mut self,
        request: JsonRpcServerRequest,
    ) -> ProtocolEventEnvelope {
        let sequence = self.next_sequence();
        normalize_server_message(sequence, request.method, Some(request.id), request.params)
    }

    fn next_sequence(&mut self) -> u64 {
        let sequence = self.next_event_sequence;
        self.next_event_sequence += 1;
        sequence
    }
}

#[cfg(test)]
mod tests {
    use serde::{Deserialize, Serialize};
    use serde_json::json;

    use super::*;
    use crate::protocol::events::ProtocolEvent;

    #[derive(Debug, Deserialize, Serialize)]
    struct ExampleParams {
        value: String,
    }

    #[test]
    fn prepared_requests_increment_ids_and_preserve_typed_params() {
        let mut router = JsonRpcRouter::new(4);

        let first = router
            .prepare_request(
                "thread/start",
                &ExampleParams {
                    value: "hello".to_owned(),
                },
            )
            .expect("first request should serialize");
        let second = router
            .prepare_request("model/list", &json!({}))
            .expect("second request should serialize");

        assert_eq!(first.request.id, RequestId::from(4));
        assert_eq!(first.method, "thread/start");
        assert_eq!(first.request.params, json!({"value": "hello"}));
        assert_eq!(second.request.id, RequestId::from(5));
        assert_eq!(router.next_request_id(), 6);
    }

    #[test]
    fn normalized_events_assign_monotonic_sequences_and_preserve_raw_methods() {
        let mut router = JsonRpcRouter::new(1);

        let started = router.normalize_notification(
            "turn/started".to_owned(),
            json!({"threadId": "thread-1", "turnId": "turn-1"}),
        );
        let approval = router.normalize_server_request(JsonRpcServerRequest {
            jsonrpc: Some("2.0".to_owned()),
            id: RequestId::String("approval-1".to_owned()),
            method: "item/permissions/requestApproval".to_owned(),
            params: json!({"itemId": "item-1"}),
        });
        let delta = router.normalize_notification(
            "item/agentMessage/delta".to_owned(),
            json!({"itemId": "item-1", "delta": "hi"}),
        );

        assert_eq!(started.sequence, 1);
        assert_eq!(approval.sequence, 2);
        assert_eq!(delta.sequence, 3);

        assert!(matches!(
            started.event,
            ProtocolEvent::TurnStarted { ref raw_method, .. } if raw_method == "turn/started"
        ));
        assert!(matches!(
            approval.event,
            ProtocolEvent::ApprovalRequested { ref raw_method, .. }
                if raw_method == "item/permissions/requestApproval"
        ));
        assert!(matches!(
            delta.event,
            ProtocolEvent::Delta { ref raw_method, .. }
                if raw_method == "item/agentMessage/delta"
        ));
    }
}
