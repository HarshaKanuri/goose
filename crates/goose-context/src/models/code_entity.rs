use serde::{Deserialize, Serialize};

/// A source code file in the codebase graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntity {
    pub path: String,
    pub language: String,
    pub size: u64,
    pub last_modified: String,
    pub repo: String,
}

/// A function or method extracted from source code.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionEntity {
    pub name: String,
    pub signature: String,
    pub file_path: String,
    pub line_start: u32,
    pub line_end: u32,
    pub language: String,
    pub repo: String,
}

/// A class, struct, or interface.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassEntity {
    pub name: String,
    pub file_path: String,
    pub language: String,
    pub kind: String, // "class", "struct", "interface", "enum"
    pub repo: String,
}

/// A configuration file (Dockerfile, CI config, k8s manifest, etc.).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigEntity {
    pub path: String,
    pub config_type: String, // "dockerfile", "ci", "k8s", "helm", "terraform"
    pub repo: String,
}
