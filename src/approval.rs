use std::fmt;
use std::io::{self, BufRead, IsTerminal, Write};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::protocol::events::{ProtocolEvent, ProtocolEventEnvelope};
use crate::protocol::messages::RequestId;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalStatus {
    Pending,
    Approved,
    Denied,
    Expired,
    Cancelled,
    Resumed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalScope {
    CommandExecution,
    FileChange,
    Permissions,
    Unknown,
}

impl ApprovalScope {
    pub fn from_method(method: &str) -> Self {
        match method {
            "item/commandExecution/requestApproval" => Self::CommandExecution,
            "item/fileChange/requestApproval" => Self::FileChange,
            "item/permissions/requestApproval" => Self::Permissions,
            _ => Self::Unknown,
        }
    }
}

impl fmt::Display for ApprovalScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::CommandExecution => "command_execution",
            Self::FileChange => "file_change",
            Self::Permissions => "permissions",
            Self::Unknown => "unknown",
        };
        write!(f, "{value}")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Approval {
    pub approval_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub scope: ApprovalScope,
    pub risk_traits: Vec<String>,
    pub summary: String,
    pub requested_action: String,
    pub requested_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    pub resume_token: String,
    pub status: ApprovalStatus,
    pub raw_method: String,
    pub request_id: RequestId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_id: Option<String>,
    pub data: Value,
}

impl Approval {
    pub fn from_event(event: &ProtocolEventEnvelope, session_id: Option<String>) -> Option<Self> {
        let ProtocolEvent::ApprovalRequested {
            raw_method,
            request_id,
            item_id,
            data,
        } = &event.event
        else {
            return None;
        };

        Some(Self::from_request_parts(
            raw_method,
            request_id,
            item_id.as_ref(),
            data,
            session_id,
        ))
    }

    pub fn from_request_parts(
        raw_method: &str,
        request_id: &RequestId,
        item_id: Option<&String>,
        data: &Value,
        session_id: Option<String>,
    ) -> Self {
        let approval_id = stringify_request_id(request_id);
        let resume_token = resume_token(session_id.as_deref(), &approval_id);
        let requested_at = data
            .get("requestedAt")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .unwrap_or_else(now_rfc3339_like);
        let expires_at = data
            .get("expiresAt")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        let summary = data
            .get("summary")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| default_summary(raw_method, data));
        let requested_action = data
            .get("requestedAction")
            .or_else(|| data.get("command"))
            .or_else(|| data.get("action"))
            .map(render_value)
            .unwrap_or_else(|| summary.clone());

        Self {
            approval_id: approval_id.clone(),
            session_id,
            scope: ApprovalScope::from_method(raw_method),
            risk_traits: derive_risk_traits(raw_method, data),
            summary,
            requested_action,
            requested_at,
            expires_at,
            resume_token,
            status: ApprovalStatus::Pending,
            raw_method: raw_method.to_owned(),
            request_id: request_id.clone(),
            item_id: item_id.cloned(),
            data: data.clone(),
        }
    }

    pub fn mark_approved(&mut self) {
        self.status = ApprovalStatus::Approved;
    }

    pub fn mark_denied(&mut self) {
        self.status = ApprovalStatus::Denied;
    }

    pub fn mark_resumed(&mut self) {
        self.status = ApprovalStatus::Resumed;
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApprovalDecision {
    pub approved: bool,
    pub resume: bool,
}

impl ApprovalDecision {
    pub fn approve_and_resume() -> Self {
        Self {
            approved: true,
            resume: true,
        }
    }

    pub fn deny() -> Self {
        Self {
            approved: false,
            resume: false,
        }
    }

    pub fn into_result(self, mut approval: Approval) -> ApprovalResult {
        if self.approved {
            approval.mark_approved();
            if self.resume {
                approval.mark_resumed();
            }
        } else {
            approval.mark_denied();
        }
        ApprovalResult {
            approval,
            decision: self,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApprovalResult {
    pub approval: Approval,
    pub decision: ApprovalDecision,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApprovalRequiredEnvelope {
    pub command: String,
    pub message: String,
    pub approval: Approval,
}

impl ApprovalRequiredEnvelope {
    pub fn new(command: impl Into<String>, message: impl Into<String>, approval: Approval) -> Self {
        Self {
            command: command.into(),
            message: message.into(),
            approval,
        }
    }

    pub fn to_json_value(&self) -> Value {
        json!({
            "ok": false,
            "command": self.command,
            "error": {
                "code": "approval_required",
                "message": self.message,
            },
            "approval": self.approval,
        })
    }
}

pub fn current_stdio_is_interactive() -> bool {
    io::stdin().is_terminal() && io::stdout().is_terminal()
}

pub fn prompt_for_approval<R: BufRead + ?Sized, W: Write + ?Sized>(
    approval: &Approval,
    reader: &mut R,
    writer: &mut W,
) -> io::Result<ApprovalDecision> {
    writeln!(writer, "Approval required")?;
    writeln!(writer, "  scope: {}", approval.scope)?;
    if let Some(session_id) = approval.session_id.as_deref() {
        writeln!(writer, "  session: {session_id}")?;
    }
    writeln!(writer, "  summary: {}", approval.summary)?;
    writeln!(writer, "  action: {}", approval.requested_action)?;
    if !approval.risk_traits.is_empty() {
        writeln!(writer, "  risk_traits: {}", approval.risk_traits.join(", "))?;
    }
    write!(writer, "Approve and resume from the blocked step? [y/N] ")?;
    writer.flush()?;

    let mut line = String::new();
    reader.read_line(&mut line)?;
    let answer = line.trim().to_ascii_lowercase();
    if matches!(answer.as_str(), "y" | "yes") {
        Ok(ApprovalDecision::approve_and_resume())
    } else {
        Ok(ApprovalDecision::deny())
    }
}

pub fn approval_response_payload(approval: &Approval, decision: &ApprovalDecision) -> Value {
    json!({
        "approved": decision.approved,
        "decision": if decision.approved { "approved" } else { "denied" },
        "resume": decision.resume,
        "resumeToken": approval.resume_token,
        "approvalId": approval.approval_id,
    })
}

pub fn latest_session_id(events: &[ProtocolEventEnvelope]) -> Option<String> {
    events.iter().rev().find_map(|event| match &event.event {
        ProtocolEvent::ThreadStarted { thread_id, .. }
        | ProtocolEvent::ThreadUpdated { thread_id, .. } => thread_id.clone(),
        ProtocolEvent::TurnStarted { thread_id, .. }
        | ProtocolEvent::TurnCompleted { thread_id, .. } => thread_id.clone(),
        _ => None,
    })
}

fn stringify_request_id(request_id: &RequestId) -> String {
    match request_id {
        RequestId::Number(value) => format!("apr_{value}"),
        RequestId::String(value) => value.clone(),
    }
}

fn resume_token(session_id: Option<&str>, approval_id: &str) -> String {
    match session_id {
        Some(session_id) => format!("{session_id}:{approval_id}"),
        None => approval_id.to_owned(),
    }
}

fn derive_risk_traits(raw_method: &str, data: &Value) -> Vec<String> {
    let mut traits = match raw_method {
        "item/commandExecution/requestApproval" => vec!["shell_exec".to_owned()],
        "item/fileChange/requestApproval" => vec!["write".to_owned()],
        "item/permissions/requestApproval" => vec!["permissions".to_owned()],
        _ => vec!["unknown".to_owned()],
    };

    if mentions_write(data) && !traits.iter().any(|trait_name| trait_name == "write") {
        traits.push("write".to_owned());
    }
    if mentions_network(data) && !traits.iter().any(|trait_name| trait_name == "network") {
        traits.push("network".to_owned());
    }
    traits
}

fn mentions_write(data: &Value) -> bool {
    let rendered = render_value(data).to_ascii_lowercase();
    rendered.contains("write") || rendered.contains("patch") || rendered.contains("file")
}

fn mentions_network(data: &Value) -> bool {
    let rendered = render_value(data).to_ascii_lowercase();
    rendered.contains("http") || rendered.contains("network") || rendered.contains("curl")
}

fn render_value(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        _ => serde_json::to_string(value).unwrap_or_else(|_| "<unrenderable>".to_owned()),
    }
}

fn default_summary(raw_method: &str, data: &Value) -> String {
    match ApprovalScope::from_method(raw_method) {
        ApprovalScope::CommandExecution => data
            .get("command")
            .map(render_value)
            .map(|command| format!("Run command: {command}"))
            .unwrap_or_else(|| "Run a shell command".to_owned()),
        ApprovalScope::FileChange => data
            .get("changes")
            .map(render_value)
            .map(|changes| format!("Apply file changes: {changes}"))
            .unwrap_or_else(|| "Apply a file change".to_owned()),
        ApprovalScope::Permissions => data
            .get("permission")
            .map(render_value)
            .map(|permission| format!("Grant permission: {permission}"))
            .unwrap_or_else(|| "Grant a protected permission".to_owned()),
        ApprovalScope::Unknown => "Server requested approval".to_owned(),
    }
}

fn now_rfc3339_like() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    format!("unix:{seconds}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn approval_object_uses_shared_lifecycle_shape() {
        let event = ProtocolEventEnvelope {
            sequence: 7,
            event: ProtocolEvent::ApprovalRequested {
                raw_method: "item/commandExecution/requestApproval".to_owned(),
                request_id: RequestId::String("approval-7".to_owned()),
                item_id: Some("item-7".to_owned()),
                data: json!({
                    "summary": "Run npm test",
                    "command": ["npm", "test"],
                    "requestedAt": "2026-05-11T20:15:00Z",
                    "expiresAt": "2026-05-11T20:20:00Z"
                }),
            },
        };

        let approval = Approval::from_event(&event, Some("thread-7".to_owned())).unwrap();

        assert_eq!(approval.approval_id, "approval-7");
        assert_eq!(approval.session_id.as_deref(), Some("thread-7"));
        assert_eq!(approval.scope, ApprovalScope::CommandExecution);
        assert_eq!(approval.status, ApprovalStatus::Pending);
        assert_eq!(approval.resume_token, "thread-7:approval-7");
        assert_eq!(approval.requested_at, "2026-05-11T20:15:00Z");
        assert_eq!(approval.expires_at.as_deref(), Some("2026-05-11T20:20:00Z"));
        assert!(
            approval
                .risk_traits
                .iter()
                .any(|trait_name| trait_name == "shell_exec")
        );
        assert_eq!(approval.summary, "Run npm test");
        assert_eq!(approval.requested_action, "[\"npm\",\"test\"]");
    }

    #[test]
    fn tty_prompt_approves_and_resumes_from_blocked_step_by_default() {
        let approval = Approval {
            approval_id: "approval-8".to_owned(),
            session_id: Some("thread-8".to_owned()),
            scope: ApprovalScope::Permissions,
            risk_traits: vec!["permissions".to_owned()],
            summary: "Grant file access".to_owned(),
            requested_action: "open ~/.ssh/id_rsa".to_owned(),
            requested_at: "2026-05-11T20:15:00Z".to_owned(),
            expires_at: None,
            resume_token: "approval-8".to_owned(),
            status: ApprovalStatus::Pending,
            raw_method: "item/permissions/requestApproval".to_owned(),
            request_id: RequestId::String("approval-8".to_owned()),
            item_id: Some("item-8".to_owned()),
            data: json!({}),
        };

        let mut input = io::Cursor::new("y\n");
        let mut output = Vec::new();
        let decision = prompt_for_approval(&approval, &mut input, &mut output).unwrap();
        let result = decision.into_result(approval.clone());

        assert!(result.decision.approved);
        assert!(result.decision.resume);
        assert_eq!(result.approval.status, ApprovalStatus::Resumed);
        let rendered = String::from_utf8(output).unwrap();
        assert!(rendered.contains("Approve and resume from the blocked step?"));
    }

    #[test]
    fn non_interactive_envelope_includes_resume_token() {
        let approval = Approval {
            approval_id: "approval-9".to_owned(),
            session_id: Some("thread-9".to_owned()),
            scope: ApprovalScope::FileChange,
            risk_traits: vec!["write".to_owned()],
            summary: "Patch src/main.rs".to_owned(),
            requested_action: "apply patch".to_owned(),
            requested_at: "2026-05-11T20:15:00Z".to_owned(),
            expires_at: None,
            resume_token: "approval-9".to_owned(),
            status: ApprovalStatus::Pending,
            raw_method: "item/fileChange/requestApproval".to_owned(),
            request_id: RequestId::String("approval-9".to_owned()),
            item_id: Some("item-9".to_owned()),
            data: json!({}),
        };

        let envelope = ApprovalRequiredEnvelope::new(
            "turns start",
            "Server requested approval before execution can continue",
            approval,
        );
        let value = envelope.to_json_value();

        assert_eq!(value["ok"], json!(false));
        assert_eq!(value["error"]["code"], json!("approval_required"));
        assert_eq!(value["approval"]["approval_id"], json!("approval-9"));
        assert_eq!(value["approval"]["resume_token"], json!("approval-9"));
    }
}
