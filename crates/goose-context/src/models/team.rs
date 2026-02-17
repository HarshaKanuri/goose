use serde::{Deserialize, Serialize};

/// A team that owns code or services.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamEntity {
    pub name: String,
    pub slack_channel: Option<String>,
    pub oncall_rotation: Option<String>,
    pub members: Vec<String>,
}
