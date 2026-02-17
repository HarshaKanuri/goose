use serde::{Deserialize, Serialize};

/// A microservice or application in the service topology.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceEntity {
    pub name: String,
    pub repo: String,
    pub deploy_target: Option<String>,
    pub team: Option<String>,
    pub language: Option<String>,
    pub description: Option<String>,
}
