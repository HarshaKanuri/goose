use serde::{Deserialize, Serialize};

/// A package or library dependency.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageEntity {
    pub name: String,
    pub version: Option<String>,
    pub ecosystem: String, // "maven", "npm", "pip", "cargo", "go", "nuget"
    pub license: Option<String>,
}

/// A dependency relationship between entities.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DependencyEdge {
    pub from_entity: String,
    pub to_entity: String,
    pub dependency_type: String, // "runtime", "dev", "build", "test"
    pub version_constraint: Option<String>,
}
