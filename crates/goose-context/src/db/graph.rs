use anyhow::{Context, Result};
use deadpool_postgres::Pool;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::debug;

/// Graph store backed by Apache AGE on PostgreSQL.
///
/// Provides Cypher query execution and convenience methods for
/// common graph operations on the codebase graph.
#[derive(Clone)]
pub struct GraphStore {
    pool: Pool,
    graph_name: String,
}

/// A node returned from a graph query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphNode {
    pub id: String,
    pub label: String,
    pub properties: Value,
}

/// An edge returned from a graph query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphEdge {
    pub id: String,
    pub label: String,
    pub start_id: String,
    pub end_id: String,
    pub properties: Value,
}

/// Result of a graph query — may contain nodes, edges, or scalar values.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphQueryResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
    pub row_count: usize,
}

impl GraphStore {
    pub fn new(pool: Pool) -> Self {
        Self {
            pool,
            graph_name: "codebase".to_string(),
        }
    }

    /// Execute a raw Cypher query against the codebase graph.
    ///
    /// The query is wrapped in `ag_catalog.cypher()` automatically.
    /// Parameters should be embedded in the Cypher query string using AGE syntax.
    ///
    /// # Safety
    /// Callers must ensure query parameters are properly escaped to prevent injection.
    /// Use the typed helper methods (create_node, find_neighbors, etc.) when possible.
    pub async fn cypher_query(&self, cypher: &str) -> Result<GraphQueryResult> {
        let client = self.pool.get().await.context("Failed to get DB client")?;

        // Load AGE and set search path
        client
            .batch_execute("LOAD 'age'; SET search_path = ag_catalog, \"$user\", public;")
            .await
            .context("Failed to load AGE")?;

        let sql = format!(
            "SELECT * FROM ag_catalog.cypher('{}', $$ {} $$) AS (result ag_catalog.agtype);",
            self.graph_name, cypher
        );

        debug!(cypher = cypher, "Executing Cypher query");

        let rows = client
            .query(&sql, &[])
            .await
            .with_context(|| format!("Cypher query failed: {}", cypher))?;

        let mut result_rows = Vec::new();
        for row in &rows {
            let val: String = row.get(0);
            let parsed: Value =
                serde_json::from_str(&val).unwrap_or_else(|_| Value::String(val.clone()));
            result_rows.push(vec![parsed]);
        }

        Ok(GraphQueryResult {
            columns: vec!["result".to_string()],
            rows: result_rows.clone(),
            row_count: result_rows.len(),
        })
    }

    /// Create a vertex in the graph.
    pub async fn create_node(
        &self,
        label: &str,
        properties: &Value,
    ) -> Result<GraphQueryResult> {
        let props_str = serde_json::to_string(properties).context("Failed to serialize properties")?;
        let cypher = format!("CREATE (n:{} {}) RETURN n", label, props_str);
        self.cypher_query(&cypher).await
    }

    /// Create an edge between two nodes.
    pub async fn create_edge(
        &self,
        from_label: &str,
        from_match: &str,
        to_label: &str,
        to_match: &str,
        edge_label: &str,
        properties: Option<&Value>,
    ) -> Result<GraphQueryResult> {
        let props = match properties {
            Some(p) => serde_json::to_string(p).context("Failed to serialize edge properties")?,
            None => "{}".to_string(),
        };
        let cypher = format!(
            "MATCH (a:{} {{{}}}), (b:{} {{{}}}) CREATE (a)-[e:{} {}]->(b) RETURN e",
            from_label, from_match, to_label, to_match, edge_label, props
        );
        self.cypher_query(&cypher).await
    }

    /// Find neighbors of a node by label and property match.
    pub async fn find_neighbors(
        &self,
        label: &str,
        match_clause: &str,
        depth: u32,
    ) -> Result<GraphQueryResult> {
        let cypher = format!(
            "MATCH (n:{} {{{}}})-[*1..{}]-(neighbor) RETURN DISTINCT neighbor",
            label, match_clause, depth
        );
        self.cypher_query(&cypher).await
    }

    /// Find the shortest path between two nodes.
    pub async fn find_path(
        &self,
        from_label: &str,
        from_match: &str,
        to_label: &str,
        to_match: &str,
    ) -> Result<GraphQueryResult> {
        let cypher = format!(
            "MATCH path = shortestPath((a:{} {{{}}})-[*]-(b:{} {{{}}})) RETURN path",
            from_label, from_match, to_label, to_match
        );
        self.cypher_query(&cypher).await
    }

    /// Get all nodes of a given label with optional property filter.
    pub async fn find_nodes(
        &self,
        label: &str,
        filter: Option<&str>,
        limit: u32,
    ) -> Result<GraphQueryResult> {
        let where_clause = filter
            .map(|f| format!(" WHERE {}", f))
            .unwrap_or_default();
        let cypher = format!(
            "MATCH (n:{}){}  RETURN n LIMIT {}",
            label, where_clause, limit
        );
        self.cypher_query(&cypher).await
    }

    /// Get team ownership for a file or service.
    pub async fn get_owners(&self, entity_label: &str, entity_match: &str) -> Result<GraphQueryResult> {
        let cypher = format!(
            "MATCH (t:Team)-[:OWNS]->(e:{} {{{}}}) RETURN t",
            entity_label, entity_match
        );
        self.cypher_query(&cypher).await
    }

    /// Get dependencies of a service or file.
    pub async fn get_dependencies(&self, entity_label: &str, entity_match: &str) -> Result<GraphQueryResult> {
        let cypher = format!(
            "MATCH (e:{} {{{}}})-[:DEPENDS_ON|IMPORTS*1..3]->(dep) RETURN DISTINCT dep",
            entity_label, entity_match
        );
        self.cypher_query(&cypher).await
    }

    /// Get tests that cover a file or function.
    pub async fn get_test_coverage(&self, entity_label: &str, entity_match: &str) -> Result<GraphQueryResult> {
        let cypher = format!(
            "MATCH (e:{} {{{}}})<-[:TESTED_BY]-(test) RETURN test",
            entity_label, entity_match
        );
        self.cypher_query(&cypher).await
    }

    /// Delete all nodes and edges for a given source (e.g., a specific repo).
    pub async fn clear_source(&self, repo: &str) -> Result<GraphQueryResult> {
        let cypher = format!(
            "MATCH (n {{repo: '{}'}}) DETACH DELETE n RETURN count(*) as deleted",
            repo
        );
        self.cypher_query(&cypher).await
    }
}
