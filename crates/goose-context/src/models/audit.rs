use serde::{Deserialize, Serialize};

/// Risk level classification for banking compliance.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RiskLevel {
    Low,
    Medium,
    High,
    Critical,
}

impl std::fmt::Display for RiskLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RiskLevel::Low => write!(f, "low"),
            RiskLevel::Medium => write!(f, "medium"),
            RiskLevel::High => write!(f, "high"),
            RiskLevel::Critical => write!(f, "critical"),
        }
    }
}

/// Types of actions logged in the audit trail.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditActionType {
    ToolCall,
    LlmRequest,
    LlmResponse,
    GatePass,
    GateFail,
    FileWrite,
    FileDelete,
    CommandExec,
    SandboxCreate,
    SandboxDestroy,
    PrCreated,
    PrMerged,
    HumanApproval,
    HumanRejection,
    Escalation,
}

impl std::fmt::Display for AuditActionType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = serde_json::to_value(self)
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_else(|| format!("{:?}", self));
        write!(f, "{}", s)
    }
}
