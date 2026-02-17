use serde::{Deserialize, Serialize};

/// A document ingested from an external source.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentEntity {
    pub title: String,
    pub source_type: String,  // "confluence", "jira", "sharepoint", "servicenow", "pdf", "upload"
    pub source_uri: String,
    pub content_preview: String,
    pub author: Option<String>,
    pub last_updated: Option<String>,
    pub tags: Vec<String>,
}

/// A chunk of a document ready for embedding.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentChunk {
    pub document_uri: String,
    pub chunk_index: usize,
    pub content: String,
    pub metadata: serde_json::Value,
}
