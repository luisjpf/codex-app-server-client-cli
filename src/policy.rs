use serde::{Deserialize, Serialize};

use crate::approval::{Approval, ApprovalScope};
use crate::protocol::events::ProtocolEventEnvelope;

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum YoloOverride {
    Enable,
    Disable,
    #[default]
    None,
}

impl YoloOverride {
    pub fn as_option(self) -> Option<Self> {
        match self {
            Self::None => None,
            value => Some(value),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum YoloSource {
    DefaultPolicy,
    Session,
    CommandOverrideEnable,
    CommandOverrideDisable,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct YoloState {
    pub effective: bool,
    pub session_enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command_override: Option<YoloOverride>,
    pub source: YoloSource,
}

impl Default for YoloState {
    fn default() -> Self {
        Self::from_session(false, None)
    }
}

impl YoloState {
    pub fn for_new_session(command_override: Option<YoloOverride>) -> Self {
        Self::from_session(false, command_override)
    }

    pub fn from_session(session_enabled: bool, command_override: Option<YoloOverride>) -> Self {
        match command_override {
            Some(YoloOverride::Enable) => Self {
                effective: true,
                session_enabled,
                command_override,
                source: YoloSource::CommandOverrideEnable,
            },
            Some(YoloOverride::Disable) => Self {
                effective: false,
                session_enabled,
                command_override,
                source: YoloSource::CommandOverrideDisable,
            },
            Some(YoloOverride::None) | None => Self {
                effective: session_enabled,
                session_enabled,
                command_override: None,
                source: if session_enabled {
                    YoloSource::Session
                } else {
                    YoloSource::DefaultPolicy
                },
            },
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalPolicyDecision {
    RequireOperator,
    AutoApproveYolo,
    RequireOperatorUnknownRisk,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApprovalPolicyEvaluation {
    pub category: String,
    pub risk_traits: Vec<String>,
    pub yolo_effective: bool,
    pub decision: ApprovalPolicyDecision,
}

impl ApprovalPolicyEvaluation {
    pub fn allows_auto_approve(&self) -> bool {
        matches!(self.decision, ApprovalPolicyDecision::AutoApproveYolo)
    }
}

pub fn evaluate_approval(approval: &Approval, yolo: &YoloState) -> ApprovalPolicyEvaluation {
    let decision = if !yolo.effective {
        ApprovalPolicyDecision::RequireOperator
    } else if approval.scope == ApprovalScope::Unknown
        || approval
            .risk_traits
            .iter()
            .any(|trait_name| trait_name == "unknown")
    {
        ApprovalPolicyDecision::RequireOperatorUnknownRisk
    } else {
        ApprovalPolicyDecision::AutoApproveYolo
    };

    ApprovalPolicyEvaluation {
        category: approval.scope.to_string(),
        risk_traits: approval.risk_traits.clone(),
        yolo_effective: yolo.effective,
        decision,
    }
}

pub fn latest_approval_evaluation(
    events: &[ProtocolEventEnvelope],
    yolo: &YoloState,
) -> Option<ApprovalPolicyEvaluation> {
    let session_id = None;
    for event in events.iter().rev() {
        if let Some(approval) = Approval::from_event(event, session_id.clone()) {
            return Some(evaluate_approval(&approval, yolo));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{ApprovalPolicyDecision, YoloOverride, YoloSource, YoloState, evaluate_approval};
    use crate::approval::{Approval, ApprovalScope, ApprovalStatus};
    use crate::protocol::messages::RequestId;

    fn approval(scope: ApprovalScope, risk_traits: &[&str]) -> Approval {
        Approval {
            approval_id: "approval_1".into(),
            session_id: Some("sess_1".into()),
            scope,
            risk_traits: risk_traits
                .iter()
                .map(|value| (*value).to_string())
                .collect(),
            summary: "test approval".into(),
            requested_action: "run command".into(),
            requested_at: "2026-05-12T00:00:00Z".into(),
            expires_at: None,
            resume_token: "approval_1".into(),
            status: ApprovalStatus::Pending,
            raw_method: "item/commandExecution/requestApproval".into(),
            request_id: RequestId::String("1".into()),
            item_id: None,
            data: json!({}),
        }
    }

    #[test]
    fn session_yolo_is_used_when_no_command_override_is_present() {
        let state = YoloState::from_session(true, None);
        assert!(state.effective);
        assert_eq!(state.source, YoloSource::Session);
        assert_eq!(state.command_override, None);
    }

    #[test]
    fn explicit_disable_overrides_session_yolo() {
        let state = YoloState::from_session(true, Some(YoloOverride::Disable));
        assert!(!state.effective);
        assert_eq!(state.source, YoloSource::CommandOverrideDisable);
    }

    #[test]
    fn yolo_auto_approves_known_risk_traits() {
        let evaluation = evaluate_approval(
            &approval(
                ApprovalScope::CommandExecution,
                &["workspace_write", "network"],
            ),
            &YoloState::from_session(true, None),
        );
        assert_eq!(evaluation.decision, ApprovalPolicyDecision::AutoApproveYolo);
    }

    #[test]
    fn unknown_risk_traits_still_require_operator_even_in_yolo() {
        let evaluation = evaluate_approval(
            &approval(ApprovalScope::Unknown, &["unknown"]),
            &YoloState::from_session(true, Some(YoloOverride::Enable)),
        );
        assert_eq!(
            evaluation.decision,
            ApprovalPolicyDecision::RequireOperatorUnknownRisk
        );
    }
}
