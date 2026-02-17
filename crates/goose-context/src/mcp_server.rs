use anyhow::Result;
use rmcp::model::{
    CallToolRequestParams, CallToolResult, Content, Implementation, InitializeResult,
    JsonObject, ListToolsResult, ProtocolVersion, ServerCapabilities, Tool as McpTool,
    ToolAnnotations, ToolsCapability,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

use crate::db::pool::ContextStore;

/// MCP server that exposes context tools for the Goose agent.
///
/// Tools:
/// - `context__graph_query` — Run Cypher query against the codebase graph (AGE)
/// - `context__graph_neighbors` — Get related entities for a file/service/function
/// - `context__graph_path` — Find dependency path between two entities
/// - `context__vector_search` — Semantic search across all ingested documents
/// - `context__vector_search_code` — Semantic search scoped to code files only
/// - `context__vector_search_docs` — Semantic search scoped to documentation only
/// - `context__sql_query` — Read-only SQL query against relational tables
/// - `context__audit_log` — Append an entry to the audit trail
/// - `context__get_owners` — Who owns this file/service?
/// - `context__get_dependencies` — What does this entity depend on?
/// - `context__get_test_coverage` — Which tests cover this entity?
pub struct ContextMcpServer {
    store: Arc<ContextStore>,
    embedding_provider: Arc<dyn crate::embedding::EmbeddingProvider>,
}

// ─── Tool Input Schemas ──────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
struct GraphQueryInput {
    /// Cypher query to execute against the codebase graph.
    /// Example: "MATCH (f:File {language: 'java'}) RETURN f LIMIT 10"
    cypher: String,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
struct GraphNeighborsInput {
    /// Label of the entity (File, Function, Class, Service, Team, Package)
    label: String,
    /// Property match clause. Example: "path: '/src/main/App.java'"
    match_clause: String,
    /// How many hops to traverse (1-5)
    #[serde(default = "default_depth")]
    depth: u32,
}

fn default_depth() -> u32 {
    2
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
struct GraphPathInput {
    /// Label of the starting entity
    from_label: String,
    /// Property match for the starting entity
    from_match: String,
    /// Label of the target entity
    to_label: String,
    /// Property match for the target entity
    to_match: String,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
struct VectorSearchInput {
    /// Natural language query for semantic search
    query: String,
    /// Maximum number of results to return
    #[serde(default = "default_limit")]
    limit: i64,
    /// Minimum similarity threshold (0.0 to 1.0)
    #[serde(default = "default_threshold")]
    similarity_threshold: f64,
}

fn default_limit() -> i64 {
    10
}

fn default_threshold() -> f64 {
    0.5
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
struct SqlQueryInput {
    /// Read-only SQL query (SELECT only). Example: "SELECT * FROM agent_runs LIMIT 10"
    sql: String,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
struct AuditLogInput {
    /// UUID of the current agent run (optional)
    run_id: Option<String>,
    /// Type of action being logged
    action_type: String,
    /// Details of the action (JSON object)
    action_detail: Value,
    /// Name of the tool being called (optional)
    tool_name: Option<String>,
    /// SHA-256 hash of the input (optional, for compliance)
    input_hash: Option<String>,
    /// Brief summary of the output (optional)
    output_summary: Option<String>,
    /// Number of tokens used (optional)
    token_count: Option<i32>,
    /// Name of the deterministic gate (optional)
    gate_name: Option<String>,
    /// Result of the gate check: "pass", "fail", "skip" (optional)
    gate_result: Option<String>,
    /// Risk level: "low", "medium", "high", "critical"
    #[serde(default = "default_risk")]
    risk_level: String,
}

fn default_risk() -> String {
    "low".to_string()
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
struct EntityLookupInput {
    /// Label of the entity (File, Function, Class, Service)
    label: String,
    /// Property match clause. Example: "path: '/src/main/App.java'"
    match_clause: String,
}

impl ContextMcpServer {
    pub fn new(
        store: Arc<ContextStore>,
        embedding_provider: Arc<dyn crate::embedding::EmbeddingProvider>,
    ) -> Self {
        Self {
            store,
            embedding_provider,
        }
    }

    pub fn server_info() -> InitializeResult {
        InitializeResult {
            protocol_version: ProtocolVersion::V_2025_03_26,
            capabilities: ServerCapabilities {
                tools: Some(ToolsCapability {
                    list_changed: Some(false),
                }),
                tasks: None,
                resources: None,
                extensions: None,
                prompts: None,
                completions: None,
                experimental: None,
                logging: None,
            },
            server_info: Implementation {
                name: "goose-context".to_string(),
                description: Some("Three-tier context management (graph, vector, SQL) for enterprise agent operations".to_string()),
                title: Some("Context Store".to_string()),
                version: env!("CARGO_PKG_VERSION").to_string(),
                icons: None,
                website_url: None,
            },
            instructions: Some(
                "Context store with graph queries (Apache AGE/Cypher), semantic vector search \
                 (pgvector), and structured SQL queries. Use graph queries to explore code \
                 relationships and dependencies. Use vector search to find relevant documentation \
                 and code. Use SQL for structured data (audit logs, agent runs, templates)."
                    .to_string(),
            ),
        }
    }

    fn tool_schema<T: JsonSchema>() -> JsonObject {
        serde_json::to_value(schemars::schema_for!(T))
            .map(|v| v.as_object().unwrap().clone())
            .expect("valid schema")
    }

    pub fn list_tools() -> ListToolsResult {

        ListToolsResult {
            tools: vec![
                McpTool::new(
                    "graph_query".to_string(),
                    "Execute a Cypher query against the codebase graph (Apache AGE). \
                     Use to explore code relationships, dependencies, imports, and service topology."
                        .to_string(),
                    Self::tool_schema::<GraphQueryInput>(),
                )
                .annotate(ToolAnnotations {
                    title: Some("Graph Query".to_string()),
                    read_only_hint: Some(true),
                    destructive_hint: Some(false),
                    idempotent_hint: Some(true),
                    open_world_hint: Some(false),
                }),
                McpTool::new(
                    "graph_neighbors".to_string(),
                    "Find entities related to a given node in the codebase graph. \
                     Returns neighbors up to the specified depth."
                        .to_string(),
                    Self::tool_schema::<GraphNeighborsInput>(),
                )
                .annotate(ToolAnnotations {
                    title: Some("Graph Neighbors".to_string()),
                    read_only_hint: Some(true),
                    destructive_hint: Some(false),
                    idempotent_hint: Some(true),
                    open_world_hint: Some(false),
                }),
                McpTool::new(
                    "graph_path".to_string(),
                    "Find the shortest path between two entities in the codebase graph. \
                     Useful for understanding dependency chains."
                        .to_string(),
                    Self::tool_schema::<GraphPathInput>(),
                )
                .annotate(ToolAnnotations {
                    title: Some("Graph Path".to_string()),
                    read_only_hint: Some(true),
                    destructive_hint: Some(false),
                    idempotent_hint: Some(true),
                    open_world_hint: Some(false),
                }),
                McpTool::new(
                    "vector_search".to_string(),
                    "Semantic search across all ingested content (code, docs, wiki pages, PDFs). \
                     Returns the most similar chunks with similarity scores."
                        .to_string(),
                    Self::tool_schema::<VectorSearchInput>(),
                )
                .annotate(ToolAnnotations {
                    title: Some("Vector Search".to_string()),
                    read_only_hint: Some(true),
                    destructive_hint: Some(false),
                    idempotent_hint: Some(true),
                    open_world_hint: Some(false),
                }),
                McpTool::new(
                    "vector_search_code".to_string(),
                    "Semantic search scoped to code files only. \
                     Use when looking for code examples or implementations."
                        .to_string(),
                    Self::tool_schema::<VectorSearchInput>(),
                )
                .annotate(ToolAnnotations {
                    title: Some("Search Code".to_string()),
                    read_only_hint: Some(true),
                    destructive_hint: Some(false),
                    idempotent_hint: Some(true),
                    open_world_hint: Some(false),
                }),
                McpTool::new(
                    "vector_search_docs".to_string(),
                    "Semantic search scoped to documentation (Confluence, PDF, SharePoint). \
                     Use when looking for setup guides, architecture docs, or runbooks."
                        .to_string(),
                    Self::tool_schema::<VectorSearchInput>(),
                )
                .annotate(ToolAnnotations {
                    title: Some("Search Docs".to_string()),
                    read_only_hint: Some(true),
                    destructive_hint: Some(false),
                    idempotent_hint: Some(true),
                    open_world_hint: Some(false),
                }),
                McpTool::new(
                    "sql_query".to_string(),
                    "Execute a read-only SQL query (SELECT only) against the relational store. \
                     Tables: agent_runs, audit_log, sandbox_templates, ingestion_jobs."
                        .to_string(),
                    Self::tool_schema::<SqlQueryInput>(),
                )
                .annotate(ToolAnnotations {
                    title: Some("SQL Query".to_string()),
                    read_only_hint: Some(true),
                    destructive_hint: Some(false),
                    idempotent_hint: Some(true),
                    open_world_hint: Some(false),
                }),
                McpTool::new(
                    "audit_log".to_string(),
                    "Append an entry to the audit trail. Used by deterministic gates \
                     and the orchestrator to maintain compliance records."
                        .to_string(),
                    Self::tool_schema::<AuditLogInput>(),
                )
                .annotate(ToolAnnotations {
                    title: Some("Audit Log".to_string()),
                    read_only_hint: Some(false),
                    destructive_hint: Some(false),
                    idempotent_hint: Some(false),
                    open_world_hint: Some(false),
                }),
                McpTool::new(
                    "get_owners".to_string(),
                    "Get the team(s) that own a given file or service. \
                     Uses CODEOWNERS data and ownership edges in the graph."
                        .to_string(),
                    Self::tool_schema::<EntityLookupInput>(),
                )
                .annotate(ToolAnnotations {
                    title: Some("Get Owners".to_string()),
                    read_only_hint: Some(true),
                    destructive_hint: Some(false),
                    idempotent_hint: Some(true),
                    open_world_hint: Some(false),
                }),
                McpTool::new(
                    "get_dependencies".to_string(),
                    "Get all dependencies of a file, service, or package. \
                     Traverses DEPENDS_ON and IMPORTS edges up to 3 hops."
                        .to_string(),
                    Self::tool_schema::<EntityLookupInput>(),
                )
                .annotate(ToolAnnotations {
                    title: Some("Get Dependencies".to_string()),
                    read_only_hint: Some(true),
                    destructive_hint: Some(false),
                    idempotent_hint: Some(true),
                    open_world_hint: Some(false),
                }),
                McpTool::new(
                    "get_test_coverage".to_string(),
                    "Get tests that cover a given file or function. \
                     Uses TESTED_BY edges in the graph."
                        .to_string(),
                    Self::tool_schema::<EntityLookupInput>(),
                )
                .annotate(ToolAnnotations {
                    title: Some("Test Coverage".to_string()),
                    read_only_hint: Some(true),
                    destructive_hint: Some(false),
                    idempotent_hint: Some(true),
                    open_world_hint: Some(false),
                }),
            ],
            next_cursor: None,
            meta: None,
        }
    }

    /// Dispatch a tool call to the appropriate handler.
    pub async fn call_tool(&self, params: CallToolRequestParams) -> CallToolResult {
        let name = params.name.to_string();
        let args = params.arguments;

        let result = match name.as_str() {
            "graph_query" => self.handle_graph_query(args).await,
            "graph_neighbors" => self.handle_graph_neighbors(args).await,
            "graph_path" => self.handle_graph_path(args).await,
            "vector_search" => self.handle_vector_search(args, None).await,
            "vector_search_code" => self.handle_vector_search(args, Some("code")).await,
            "vector_search_docs" => self.handle_vector_search(args, Some("doc")).await,
            "sql_query" => self.handle_sql_query(args).await,
            "audit_log" => self.handle_audit_log(args).await,
            "get_owners" => self.handle_get_owners(args).await,
            "get_dependencies" => self.handle_get_dependencies(args).await,
            "get_test_coverage" => self.handle_get_test_coverage(args).await,
            _ => Err(format!("Unknown tool: {}", name)),
        };

        match result {
            Ok(content) => CallToolResult::success(content),
            Err(error) => CallToolResult::error(vec![Content::text(format!("Error: {}", error))]),
        }
    }

    // ─── Tool Handlers ───────────────────────────────────────────────────

    async fn handle_graph_query(
        &self,
        args: Option<JsonObject>,
    ) -> Result<Vec<Content>, String> {
        let input: GraphQueryInput = parse_args(args)?;
        let result = self
            .store
            .graph()
            .cypher_query(&input.cypher)
            .await
            .map_err(|e| format!("Graph query failed: {}", e))?;
        Ok(vec![Content::text(
            serde_json::to_string_pretty(&result).unwrap_or_default(),
        )])
    }

    async fn handle_graph_neighbors(
        &self,
        args: Option<JsonObject>,
    ) -> Result<Vec<Content>, String> {
        let input: GraphNeighborsInput = parse_args(args)?;
        let depth = input.depth.min(5); // Cap at 5 hops
        let result = self
            .store
            .graph()
            .find_neighbors(&input.label, &input.match_clause, depth)
            .await
            .map_err(|e| format!("Graph neighbors query failed: {}", e))?;
        Ok(vec![Content::text(
            serde_json::to_string_pretty(&result).unwrap_or_default(),
        )])
    }

    async fn handle_graph_path(
        &self,
        args: Option<JsonObject>,
    ) -> Result<Vec<Content>, String> {
        let input: GraphPathInput = parse_args(args)?;
        let result = self
            .store
            .graph()
            .find_path(
                &input.from_label,
                &input.from_match,
                &input.to_label,
                &input.to_match,
            )
            .await
            .map_err(|e| format!("Graph path query failed: {}", e))?;
        Ok(vec![Content::text(
            serde_json::to_string_pretty(&result).unwrap_or_default(),
        )])
    }

    async fn handle_vector_search(
        &self,
        args: Option<JsonObject>,
        source_type_filter: Option<&str>,
    ) -> Result<Vec<Content>, String> {
        let input: VectorSearchInput = parse_args(args)?;

        // Generate embedding for the query
        let query_embedding = self
            .embedding_provider
            .embed(&input.query)
            .await
            .map_err(|e| format!("Failed to generate query embedding: {}", e))?;

        let params = crate::db::vector::VectorSearchParams {
            query_embedding,
            source_type: source_type_filter.map(String::from),
            limit: input.limit,
            similarity_threshold: input.similarity_threshold,
        };

        let results = self
            .store
            .vector()
            .search(&params)
            .await
            .map_err(|e| format!("Vector search failed: {}", e))?;

        Ok(vec![Content::text(
            serde_json::to_string_pretty(&results).unwrap_or_default(),
        )])
    }

    async fn handle_sql_query(
        &self,
        args: Option<JsonObject>,
    ) -> Result<Vec<Content>, String> {
        let input: SqlQueryInput = parse_args(args)?;
        let result = self
            .store
            .relational()
            .read_only_query(&input.sql)
            .await
            .map_err(|e| format!("SQL query failed: {}", e))?;
        Ok(vec![Content::text(
            serde_json::to_string_pretty(&result).unwrap_or_default(),
        )])
    }

    async fn handle_audit_log(
        &self,
        args: Option<JsonObject>,
    ) -> Result<Vec<Content>, String> {
        let input: AuditLogInput = parse_args(args)?;

        let run_id = input
            .run_id
            .as_ref()
            .and_then(|id| uuid::Uuid::parse_str(id).ok());

        let entry = crate::db::relational::CreateAuditLog {
            run_id,
            action_type: input.action_type,
            action_detail: input.action_detail,
            tool_name: input.tool_name,
            input_hash: input.input_hash,
            output_summary: input.output_summary,
            token_count: input.token_count,
            gate_name: input.gate_name,
            gate_result: input.gate_result,
            risk_level: Some(input.risk_level),
        };

        let id = self
            .store
            .relational()
            .append_audit_log(entry)
            .await
            .map_err(|e| format!("Failed to append audit log: {}", e))?;

        Ok(vec![Content::text(format!(
            "Audit log entry created with id: {}",
            id
        ))])
    }

    async fn handle_get_owners(
        &self,
        args: Option<JsonObject>,
    ) -> Result<Vec<Content>, String> {
        let input: EntityLookupInput = parse_args(args)?;
        let result = self
            .store
            .graph()
            .get_owners(&input.label, &input.match_clause)
            .await
            .map_err(|e| format!("Get owners failed: {}", e))?;
        Ok(vec![Content::text(
            serde_json::to_string_pretty(&result).unwrap_or_default(),
        )])
    }

    async fn handle_get_dependencies(
        &self,
        args: Option<JsonObject>,
    ) -> Result<Vec<Content>, String> {
        let input: EntityLookupInput = parse_args(args)?;
        let result = self
            .store
            .graph()
            .get_dependencies(&input.label, &input.match_clause)
            .await
            .map_err(|e| format!("Get dependencies failed: {}", e))?;
        Ok(vec![Content::text(
            serde_json::to_string_pretty(&result).unwrap_or_default(),
        )])
    }

    async fn handle_get_test_coverage(
        &self,
        args: Option<JsonObject>,
    ) -> Result<Vec<Content>, String> {
        let input: EntityLookupInput = parse_args(args)?;
        let result = self
            .store
            .graph()
            .get_test_coverage(&input.label, &input.match_clause)
            .await
            .map_err(|e| format!("Get test coverage failed: {}", e))?;
        Ok(vec![Content::text(
            serde_json::to_string_pretty(&result).unwrap_or_default(),
        )])
    }
}

/// Helper to parse tool arguments from JSON.
fn parse_args<T: serde::de::DeserializeOwned>(args: Option<JsonObject>) -> Result<T, String> {
    let args = args.ok_or("Missing arguments")?;
    serde_json::from_value(Value::Object(args)).map_err(|e| format!("Invalid arguments: {}", e))
}
