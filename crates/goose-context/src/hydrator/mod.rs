use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{debug, info};

use crate::db::pool::ContextStore;
use crate::db::vector::VectorSearchParams;
use crate::embedding::EmbeddingProvider;

/// Hydrated context bundle assembled before an agent run starts.
///
/// This is the "context hydration" step from Stripe's architecture:
/// before the agent even begins, the orchestrator pre-fetches relevant
/// context from all three tiers (graph, vector, SQL) and packages it
/// into a structured bundle that gets injected into the agent's system prompt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HydratedContext {
    /// Summary of relevant code files and their relationships
    pub code_context: Vec<CodeContextItem>,
    /// Relevant documentation snippets from semantic search
    pub doc_context: Vec<DocContextItem>,
    /// Team ownership information for affected files
    pub ownership: Vec<OwnershipItem>,
    /// Relevant test files for affected code
    pub test_context: Vec<TestContextItem>,
    /// Structured metadata (e.g., Jira ticket details)
    pub metadata: serde_json::Value,
    /// Recommended MCP tools for this task (curated subset)
    pub recommended_tools: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeContextItem {
    pub file_path: String,
    pub language: String,
    pub relevance: f64,
    pub snippet: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocContextItem {
    pub title: String,
    pub source: String,
    pub relevance: f64,
    pub snippet: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OwnershipItem {
    pub entity: String,
    pub team: String,
    pub slack_channel: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestContextItem {
    pub test_file: String,
    pub covers: String,
}

/// The intent extracted from a trigger message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HydrationIntent {
    /// The raw user message / trigger text
    pub message: String,
    /// Repository being worked on (if known)
    pub repo: Option<String>,
    /// Specific file paths mentioned
    pub file_paths: Vec<String>,
    /// Jira ticket references (e.g., "CORE-1234")
    pub ticket_refs: Vec<String>,
    /// URLs found in the message
    pub urls: Vec<String>,
    /// Service names mentioned
    pub service_names: Vec<String>,
}

/// Context hydrator — pre-fetches relevant context before agent runs.
///
/// Implements the "context hydration with MCP" pattern from Stripe's architecture.
/// Scans the trigger message for references, then queries all three tiers
/// to assemble a comprehensive context bundle.
pub struct ContextHydrator {
    store: Arc<ContextStore>,
    embedding_provider: Arc<dyn EmbeddingProvider>,
    max_code_results: usize,
    max_doc_results: usize,
}

impl ContextHydrator {
    pub fn new(
        store: Arc<ContextStore>,
        embedding_provider: Arc<dyn EmbeddingProvider>,
    ) -> Self {
        Self {
            store,
            embedding_provider,
            max_code_results: 10,
            max_doc_results: 5,
        }
    }

    /// Hydrate context for a given intent.
    ///
    /// This runs before the agent starts and provides:
    /// 1. Code files related to mentioned paths/services (graph queries)
    /// 2. Relevant documentation (vector search)
    /// 3. Team ownership for affected code (graph queries)
    /// 4. Related test files (graph queries)
    /// 5. Curated tool recommendations
    pub async fn hydrate(&self, intent: &HydrationIntent) -> Result<HydratedContext> {
        info!(message = %intent.message, "Starting context hydration");

        // Run searches in parallel using tokio::join!
        let (code_context, doc_context, ownership, test_context) = tokio::join!(
            self.hydrate_code_context(intent),
            self.hydrate_doc_context(intent),
            self.hydrate_ownership(intent),
            self.hydrate_test_context(intent),
        );

        let code_context = code_context.unwrap_or_default();
        let doc_context = doc_context.unwrap_or_default();
        let ownership = ownership.unwrap_or_default();
        let test_context = test_context.unwrap_or_default();

        // Curate recommended tools based on context
        let recommended_tools = self.curate_tools(intent, &code_context);

        let context = HydratedContext {
            code_context,
            doc_context,
            ownership,
            test_context,
            metadata: serde_json::Value::Object(Default::default()),
            recommended_tools,
        };

        info!(
            code_items = context.code_context.len(),
            doc_items = context.doc_context.len(),
            owners = context.ownership.len(),
            tests = context.test_context.len(),
            tools = context.recommended_tools.len(),
            "Context hydration complete"
        );

        Ok(context)
    }

    /// Search for relevant code using vector similarity on the message.
    async fn hydrate_code_context(
        &self,
        intent: &HydrationIntent,
    ) -> Result<Vec<CodeContextItem>> {
        let mut items = Vec::new();

        // If specific file paths are mentioned, look them up in the graph
        for path in &intent.file_paths {
            let graph_result = self
                .store
                .graph()
                .find_nodes("File", Some(&format!("n.path = '{}'", path)), 1)
                .await;

            if let Ok(result) = graph_result {
                for row in &result.rows {
                    if let Some(val) = row.first() {
                        items.push(CodeContextItem {
                            file_path: path.clone(),
                            language: val
                                .get("language")
                                .and_then(|v| v.as_str())
                                .unwrap_or("unknown")
                                .to_string(),
                            relevance: 1.0,
                            snippet: format!("Directly referenced: {}", path),
                        });
                    }
                }
            }
        }

        // Semantic search for code related to the message
        let query_embedding = self.embedding_provider.embed(&intent.message).await?;
        let search_params = VectorSearchParams {
            query_embedding,
            source_type: Some("code".to_string()),
            limit: self.max_code_results as i64,
            similarity_threshold: 0.6,
        };

        let results = self.store.vector().search(&search_params).await?;
        for result in results {
            items.push(CodeContextItem {
                file_path: result.source_id.clone(),
                language: result
                    .metadata
                    .get("language")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string(),
                relevance: result.similarity.unwrap_or(0.0),
                snippet: result.content.chars().take(200).collect(),
            });
        }

        // Deduplicate by file_path
        items.sort_by(|a, b| b.relevance.partial_cmp(&a.relevance).unwrap_or(std::cmp::Ordering::Equal));
        items.dedup_by(|a, b| a.file_path == b.file_path);

        Ok(items)
    }

    /// Search for relevant documentation.
    async fn hydrate_doc_context(
        &self,
        intent: &HydrationIntent,
    ) -> Result<Vec<DocContextItem>> {
        let query_embedding = self.embedding_provider.embed(&intent.message).await?;
        let search_params = VectorSearchParams {
            query_embedding,
            source_type: Some("doc".to_string()),
            limit: self.max_doc_results as i64,
            similarity_threshold: 0.5,
        };

        let results = self.store.vector().search(&search_params).await?;
        Ok(results
            .into_iter()
            .map(|r| DocContextItem {
                title: r
                    .metadata
                    .get("title")
                    .and_then(|v| v.as_str())
                    .unwrap_or(&r.source_id)
                    .to_string(),
                source: r.source_type,
                relevance: r.similarity.unwrap_or(0.0),
                snippet: r.content.chars().take(300).collect(),
            })
            .collect())
    }

    /// Look up team ownership for mentioned files/services.
    async fn hydrate_ownership(
        &self,
        intent: &HydrationIntent,
    ) -> Result<Vec<OwnershipItem>> {
        let mut items = Vec::new();

        for path in &intent.file_paths {
            let result = self
                .store
                .graph()
                .get_owners("File", &format!("path: '{}'", path))
                .await;

            if let Ok(result) = result {
                for row in &result.rows {
                    if let Some(val) = row.first() {
                        items.push(OwnershipItem {
                            entity: path.clone(),
                            team: val
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("unknown")
                                .to_string(),
                            slack_channel: val
                                .get("slack_channel")
                                .and_then(|v| v.as_str())
                                .map(String::from),
                        });
                    }
                }
            }
        }

        for service in &intent.service_names {
            let result = self
                .store
                .graph()
                .get_owners("Service", &format!("name: '{}'", service))
                .await;

            if let Ok(result) = result {
                for row in &result.rows {
                    if let Some(val) = row.first() {
                        items.push(OwnershipItem {
                            entity: service.clone(),
                            team: val
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("unknown")
                                .to_string(),
                            slack_channel: val
                                .get("slack_channel")
                                .and_then(|v| v.as_str())
                                .map(String::from),
                        });
                    }
                }
            }
        }

        Ok(items)
    }

    /// Find tests related to mentioned files.
    async fn hydrate_test_context(
        &self,
        intent: &HydrationIntent,
    ) -> Result<Vec<TestContextItem>> {
        let mut items = Vec::new();

        for path in &intent.file_paths {
            let result = self
                .store
                .graph()
                .get_test_coverage("File", &format!("path: '{}'", path))
                .await;

            if let Ok(result) = result {
                for row in &result.rows {
                    if let Some(val) = row.first() {
                        items.push(TestContextItem {
                            test_file: val
                                .get("file_path")
                                .and_then(|v| v.as_str())
                                .unwrap_or("unknown")
                                .to_string(),
                            covers: path.clone(),
                        });
                    }
                }
            }
        }

        Ok(items)
    }

    /// Curate a subset of recommended MCP tools based on the task context.
    ///
    /// This follows Stripe's pattern of giving agents ~15 curated tools
    /// instead of all 400+ to prevent wasting tokens on irrelevant options.
    fn curate_tools(
        &self,
        intent: &HydrationIntent,
        code_context: &[CodeContextItem],
    ) -> Vec<String> {
        let mut tools = vec![
            // Always include core developer tools
            "developer__shell".to_string(),
            "developer__text_editor".to_string(),
            // Always include context tools
            "context__vector_search".to_string(),
            "context__graph_query".to_string(),
            "context__audit_log".to_string(),
        ];

        // Add language-specific tools based on detected languages
        let languages: Vec<&str> = code_context
            .iter()
            .map(|c| c.language.as_str())
            .collect();

        if languages.iter().any(|l| *l == "java" || *l == "kotlin") {
            tools.push("developer__shell".to_string()); // for mvn/gradle
        }

        // Add git tools if repo context is present
        if intent.repo.is_some() {
            tools.push("developer__shell".to_string()); // for git operations
        }

        // Add Jira tools if tickets are referenced
        if !intent.ticket_refs.is_empty() {
            tools.push("context__sql_query".to_string());
        }

        // Deduplicate
        tools.sort();
        tools.dedup();

        debug!(count = tools.len(), "Curated tool subset");
        tools
    }

    /// Format the hydrated context as a system prompt supplement.
    pub fn format_as_prompt(context: &HydratedContext) -> String {
        let mut parts = Vec::new();

        if !context.code_context.is_empty() {
            parts.push("## Relevant Code Files\n".to_string());
            for item in &context.code_context {
                parts.push(format!(
                    "- `{}` ({}, relevance: {:.2}): {}\n",
                    item.file_path, item.language, item.relevance, item.snippet
                ));
            }
        }

        if !context.doc_context.is_empty() {
            parts.push("\n## Relevant Documentation\n".to_string());
            for item in &context.doc_context {
                parts.push(format!(
                    "- **{}** (from {}, relevance: {:.2}): {}\n",
                    item.title, item.source, item.relevance, item.snippet
                ));
            }
        }

        if !context.ownership.is_empty() {
            parts.push("\n## Code Ownership\n".to_string());
            for item in &context.ownership {
                let channel = item
                    .slack_channel
                    .as_deref()
                    .map(|c| format!(" ({})", c))
                    .unwrap_or_default();
                parts.push(format!("- `{}` owned by **{}**{}\n", item.entity, item.team, channel));
            }
        }

        if !context.test_context.is_empty() {
            parts.push("\n## Related Tests\n".to_string());
            for item in &context.test_context {
                parts.push(format!("- `{}` covers `{}`\n", item.test_file, item.covers));
            }
        }

        parts.join("")
    }
}

#[cfg(test)]
mod tests;
