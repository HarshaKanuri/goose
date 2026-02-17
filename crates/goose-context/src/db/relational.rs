use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use deadpool_postgres::Pool;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

/// Relational store for structured data — agent runs, audit trail, templates, ingestion jobs.
#[derive(Clone)]
pub struct RelationalStore {
    pool: Pool,
}

// ─── Agent Run ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRun {
    pub id: Uuid,
    pub session_id: String,
    pub trigger_source: String,
    pub trigger_ref: Option<String>,
    pub sandbox_id: Option<String>,
    pub template_id: Option<Uuid>,
    pub status: String,
    pub pr_url: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub created_by: String,
    pub approved_by: Option<String>,
    pub metadata: Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct CreateAgentRun {
    pub session_id: String,
    pub trigger_source: String,
    pub trigger_ref: Option<String>,
    pub created_by: String,
    pub metadata: Option<Value>,
}

// ─── Audit Log ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditLogEntry {
    pub id: i64,
    pub run_id: Option<Uuid>,
    pub timestamp: DateTime<Utc>,
    pub action_type: String,
    pub action_detail: Value,
    pub tool_name: Option<String>,
    pub input_hash: Option<String>,
    pub output_summary: Option<String>,
    pub token_count: Option<i32>,
    pub gate_name: Option<String>,
    pub gate_result: Option<String>,
    pub risk_level: String,
}

#[derive(Debug, Clone)]
pub struct CreateAuditLog {
    pub run_id: Option<Uuid>,
    pub action_type: String,
    pub action_detail: Value,
    pub tool_name: Option<String>,
    pub input_hash: Option<String>,
    pub output_summary: Option<String>,
    pub token_count: Option<i32>,
    pub gate_name: Option<String>,
    pub gate_result: Option<String>,
    pub risk_level: Option<String>,
}

// ─── Sandbox Template ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxTemplate {
    pub id: Uuid,
    pub name: String,
    pub repo_url: Option<String>,
    pub description: Option<String>,
    pub base_image: String,
    pub dockerfile: String,
    pub setup_script: Option<String>,
    pub languages: Vec<String>,
    pub build_tools: Vec<String>,
    pub pre_warm_count: i32,
    pub resource_limits: Value,
    pub gates_config: Value,
    pub tools_config: Value,
    pub network_policy: Value,
    pub status: String,
    pub scan_results: Option<Value>,
    pub created_at: DateTime<Utc>,
}

// ─── Ingestion Job ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngestionJob {
    pub id: Uuid,
    pub source_type: String,
    pub source_uri: String,
    pub status: String,
    pub chunks_created: i32,
    pub graph_nodes: i32,
    pub graph_edges: i32,
    pub error_message: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
}

impl RelationalStore {
    pub fn new(pool: Pool) -> Self {
        Self { pool }
    }

    // ─── Agent Run CRUD ──────────────────────────────────────────────────

    pub async fn create_agent_run(&self, input: CreateAgentRun) -> Result<AgentRun> {
        let client = self.pool.get().await.context("Failed to get DB client")?;
        let metadata = input.metadata.unwrap_or(Value::Object(Default::default()));

        let row = client
            .query_one(
                "INSERT INTO agent_runs (session_id, trigger_source, trigger_ref, created_by, metadata, status)
                 VALUES ($1, $2, $3, $4, $5, 'pending')
                 RETURNING *",
                &[
                    &input.session_id,
                    &input.trigger_source,
                    &input.trigger_ref,
                    &input.created_by,
                    &metadata,
                ],
            )
            .await
            .context("Failed to create agent run")?;

        Ok(agent_run_from_row(&row))
    }

    pub async fn get_agent_run(&self, id: Uuid) -> Result<Option<AgentRun>> {
        let client = self.pool.get().await.context("Failed to get DB client")?;
        let row = client
            .query_opt("SELECT * FROM agent_runs WHERE id = $1", &[&id])
            .await
            .context("Failed to get agent run")?;
        Ok(row.as_ref().map(agent_run_from_row))
    }

    pub async fn update_agent_run_status(
        &self,
        id: Uuid,
        status: &str,
        sandbox_id: Option<&str>,
        pr_url: Option<&str>,
    ) -> Result<()> {
        let client = self.pool.get().await.context("Failed to get DB client")?;

        let now: Option<DateTime<Utc>> = match status {
            "running" => Some(Utc::now()),
            _ => None,
        };
        let completed: Option<DateTime<Utc>> = match status {
            "completed" | "failed" | "escalated" => Some(Utc::now()),
            _ => None,
        };

        client
            .execute(
                "UPDATE agent_runs SET status = $2, sandbox_id = COALESCE($3, sandbox_id),
                 pr_url = COALESCE($4, pr_url),
                 started_at = COALESCE($5, started_at),
                 completed_at = COALESCE($6, completed_at),
                 updated_at = now()
                 WHERE id = $1",
                &[&id, &status, &sandbox_id, &pr_url, &now, &completed],
            )
            .await
            .context("Failed to update agent run")?;

        Ok(())
    }

    pub async fn list_agent_runs(
        &self,
        status: Option<&str>,
        limit: i64,
    ) -> Result<Vec<AgentRun>> {
        let client = self.pool.get().await.context("Failed to get DB client")?;
        let rows = if let Some(s) = status {
            client
                .query(
                    "SELECT * FROM agent_runs WHERE status = $1 ORDER BY created_at DESC LIMIT $2",
                    &[&s, &limit],
                )
                .await?
        } else {
            client
                .query(
                    "SELECT * FROM agent_runs ORDER BY created_at DESC LIMIT $1",
                    &[&limit],
                )
                .await?
        };
        Ok(rows.iter().map(agent_run_from_row).collect())
    }

    // ─── Audit Log ───────────────────────────────────────────────────────

    pub async fn append_audit_log(&self, input: CreateAuditLog) -> Result<i64> {
        let client = self.pool.get().await.context("Failed to get DB client")?;
        let risk_level = input.risk_level.unwrap_or_else(|| "low".to_string());

        let row = client
            .query_one(
                "INSERT INTO audit_log (run_id, action_type, action_detail, tool_name,
                 input_hash, output_summary, token_count, gate_name, gate_result, risk_level)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
                 RETURNING id",
                &[
                    &input.run_id,
                    &input.action_type,
                    &input.action_detail,
                    &input.tool_name,
                    &input.input_hash,
                    &input.output_summary,
                    &input.token_count,
                    &input.gate_name,
                    &input.gate_result,
                    &risk_level,
                ],
            )
            .await
            .context("Failed to append audit log")?;

        Ok(row.get("id"))
    }

    pub async fn query_audit_log(
        &self,
        run_id: Option<Uuid>,
        action_type: Option<&str>,
        limit: i64,
    ) -> Result<Vec<AuditLogEntry>> {
        let client = self.pool.get().await.context("Failed to get DB client")?;

        let (sql, params): (String, Vec<Box<dyn tokio_postgres::types::ToSql + Sync>>) =
            match (run_id, action_type) {
                (Some(rid), Some(at)) => (
                    "SELECT * FROM audit_log WHERE run_id = $1 AND action_type = $2 ORDER BY timestamp DESC LIMIT $3".to_string(),
                    vec![Box::new(rid), Box::new(at.to_string()), Box::new(limit)],
                ),
                (Some(rid), None) => (
                    "SELECT * FROM audit_log WHERE run_id = $1 ORDER BY timestamp DESC LIMIT $2".to_string(),
                    vec![Box::new(rid), Box::new(limit)],
                ),
                (None, Some(at)) => (
                    "SELECT * FROM audit_log WHERE action_type = $1 ORDER BY timestamp DESC LIMIT $2".to_string(),
                    vec![Box::new(at.to_string()), Box::new(limit)],
                ),
                (None, None) => (
                    "SELECT * FROM audit_log ORDER BY timestamp DESC LIMIT $1".to_string(),
                    vec![Box::new(limit)],
                ),
            };

        let param_refs: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> =
            params.iter().map(|p| p.as_ref()).collect();

        let rows = client.query(&sql, &param_refs).await?;
        Ok(rows.iter().map(audit_log_from_row).collect())
    }

    // ─── Sandbox Templates ───────────────────────────────────────────────

    pub async fn create_template(&self, template: &SandboxTemplate) -> Result<Uuid> {
        let client = self.pool.get().await.context("Failed to get DB client")?;
        let row = client
            .query_one(
                "INSERT INTO sandbox_templates (name, repo_url, description, base_image, dockerfile,
                 setup_script, languages, build_tools, pre_warm_count, resource_limits,
                 gates_config, tools_config, network_policy, status, scan_results)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)
                 RETURNING id",
                &[
                    &template.name,
                    &template.repo_url,
                    &template.description,
                    &template.base_image,
                    &template.dockerfile,
                    &template.setup_script,
                    &template.languages,
                    &template.build_tools,
                    &template.pre_warm_count,
                    &template.resource_limits,
                    &template.gates_config,
                    &template.tools_config,
                    &template.network_policy,
                    &template.status,
                    &template.scan_results,
                ],
            )
            .await
            .context("Failed to create template")?;
        Ok(row.get("id"))
    }

    pub async fn get_template(&self, id: Uuid) -> Result<Option<SandboxTemplate>> {
        let client = self.pool.get().await.context("Failed to get DB client")?;
        let row = client
            .query_opt("SELECT * FROM sandbox_templates WHERE id = $1", &[&id])
            .await?;
        Ok(row.as_ref().map(template_from_row))
    }

    pub async fn list_templates(&self, status: Option<&str>) -> Result<Vec<SandboxTemplate>> {
        let client = self.pool.get().await.context("Failed to get DB client")?;
        let rows = if let Some(s) = status {
            client
                .query(
                    "SELECT * FROM sandbox_templates WHERE status = $1 ORDER BY created_at DESC",
                    &[&s],
                )
                .await?
        } else {
            client
                .query(
                    "SELECT * FROM sandbox_templates ORDER BY created_at DESC",
                    &[],
                )
                .await?
        };
        Ok(rows.iter().map(template_from_row).collect())
    }

    pub async fn approve_template(&self, id: Uuid) -> Result<()> {
        let client = self.pool.get().await.context("Failed to get DB client")?;
        client
            .execute(
                "UPDATE sandbox_templates SET status = 'approved', updated_at = now() WHERE id = $1",
                &[&id],
            )
            .await?;
        Ok(())
    }

    // ─── Ingestion Jobs ──────────────────────────────────────────────────

    pub async fn create_ingestion_job(
        &self,
        source_type: &str,
        source_uri: &str,
    ) -> Result<Uuid> {
        let client = self.pool.get().await.context("Failed to get DB client")?;
        let row = client
            .query_one(
                "INSERT INTO ingestion_jobs (source_type, source_uri, status)
                 VALUES ($1, $2, 'pending') RETURNING id",
                &[&source_type, &source_uri],
            )
            .await?;
        Ok(row.get("id"))
    }

    pub async fn update_ingestion_job(
        &self,
        id: Uuid,
        status: &str,
        chunks: Option<i32>,
        nodes: Option<i32>,
        edges: Option<i32>,
        error: Option<&str>,
    ) -> Result<()> {
        let client = self.pool.get().await.context("Failed to get DB client")?;
        let started = if status == "running" {
            Some(Utc::now())
        } else {
            None
        };
        let completed = if status == "completed" || status == "failed" {
            Some(Utc::now())
        } else {
            None
        };

        client
            .execute(
                "UPDATE ingestion_jobs SET status = $2,
                 chunks_created = COALESCE($3, chunks_created),
                 graph_nodes = COALESCE($4, graph_nodes),
                 graph_edges = COALESCE($5, graph_edges),
                 error_message = COALESCE($6, error_message),
                 started_at = COALESCE($7, started_at),
                 completed_at = COALESCE($8, completed_at)
                 WHERE id = $1",
                &[&id, &status, &chunks, &nodes, &edges, &error, &started, &completed],
            )
            .await?;
        Ok(())
    }

    pub async fn list_ingestion_jobs(
        &self,
        status: Option<&str>,
        limit: i64,
    ) -> Result<Vec<IngestionJob>> {
        let client = self.pool.get().await.context("Failed to get DB client")?;
        let rows = if let Some(s) = status {
            client
                .query(
                    "SELECT * FROM ingestion_jobs WHERE status = $1 ORDER BY created_at DESC LIMIT $2",
                    &[&s, &limit],
                )
                .await?
        } else {
            client
                .query(
                    "SELECT * FROM ingestion_jobs ORDER BY created_at DESC LIMIT $1",
                    &[&limit],
                )
                .await?
        };
        Ok(rows.iter().map(ingestion_job_from_row).collect())
    }

    // ─── Read-Only SQL ───────────────────────────────────────────────────

    /// Execute a read-only SQL query. Only SELECT statements are allowed.
    pub async fn read_only_query(&self, sql: &str) -> Result<Vec<Vec<(String, Value)>>> {
        // Safety: only allow SELECT statements
        let trimmed = sql.trim().to_uppercase();
        if !trimmed.starts_with("SELECT") {
            anyhow::bail!("Only SELECT queries are allowed via read_only_query");
        }

        let client = self.pool.get().await.context("Failed to get DB client")?;
        let rows = client.query(sql, &[]).await.context("SQL query failed")?;

        let mut result = Vec::new();
        for row in &rows {
            let mut row_data = Vec::new();
            for (i, col) in row.columns().iter().enumerate() {
                let value: Value = match col.type_().name() {
                    "int4" => serde_json::to_value(row.get::<_, Option<i32>>(i)).unwrap_or(Value::Null),
                    "int8" => serde_json::to_value(row.get::<_, Option<i64>>(i)).unwrap_or(Value::Null),
                    "text" | "varchar" => {
                        serde_json::to_value(row.get::<_, Option<String>>(i)).unwrap_or(Value::Null)
                    }
                    "bool" => serde_json::to_value(row.get::<_, Option<bool>>(i)).unwrap_or(Value::Null),
                    "jsonb" | "json" => row.get::<_, Option<Value>>(i).unwrap_or(Value::Null),
                    "uuid" => serde_json::to_value(
                        row.get::<_, Option<Uuid>>(i).map(|u| u.to_string()),
                    )
                    .unwrap_or(Value::Null),
                    "timestamptz" | "timestamp" => serde_json::to_value(
                        row.get::<_, Option<DateTime<Utc>>>(i)
                            .map(|d| d.to_rfc3339()),
                    )
                    .unwrap_or(Value::Null),
                    _ => Value::String(format!("<unsupported type: {}>", col.type_().name())),
                };
                row_data.push((col.name().to_string(), value));
            }
            result.push(row_data);
        }
        Ok(result)
    }
}

// ─── Row Mapping Helpers ─────────────────────────────────────────────────────

fn agent_run_from_row(row: &tokio_postgres::Row) -> AgentRun {
    AgentRun {
        id: row.get("id"),
        session_id: row.get("session_id"),
        trigger_source: row.get("trigger_source"),
        trigger_ref: row.get("trigger_ref"),
        sandbox_id: row.get("sandbox_id"),
        template_id: row.get("template_id"),
        status: row.get("status"),
        pr_url: row.get("pr_url"),
        started_at: row.get("started_at"),
        completed_at: row.get("completed_at"),
        created_by: row.get("created_by"),
        approved_by: row.get("approved_by"),
        metadata: row.get("metadata"),
        created_at: row.get("created_at"),
    }
}

fn audit_log_from_row(row: &tokio_postgres::Row) -> AuditLogEntry {
    AuditLogEntry {
        id: row.get("id"),
        run_id: row.get("run_id"),
        timestamp: row.get("timestamp"),
        action_type: row.get("action_type"),
        action_detail: row.get("action_detail"),
        tool_name: row.get("tool_name"),
        input_hash: row.get("input_hash"),
        output_summary: row.get("output_summary"),
        token_count: row.get("token_count"),
        gate_name: row.get("gate_name"),
        gate_result: row.get("gate_result"),
        risk_level: row.get("risk_level"),
    }
}

fn template_from_row(row: &tokio_postgres::Row) -> SandboxTemplate {
    SandboxTemplate {
        id: row.get("id"),
        name: row.get("name"),
        repo_url: row.get("repo_url"),
        description: row.get("description"),
        base_image: row.get("base_image"),
        dockerfile: row.get("dockerfile"),
        setup_script: row.get("setup_script"),
        languages: row.get("languages"),
        build_tools: row.get("build_tools"),
        pre_warm_count: row.get("pre_warm_count"),
        resource_limits: row.get("resource_limits"),
        gates_config: row.get("gates_config"),
        tools_config: row.get("tools_config"),
        network_policy: row.get("network_policy"),
        status: row.get("status"),
        scan_results: row.get("scan_results"),
        created_at: row.get("created_at"),
    }
}

fn ingestion_job_from_row(row: &tokio_postgres::Row) -> IngestionJob {
    IngestionJob {
        id: row.get("id"),
        source_type: row.get("source_type"),
        source_uri: row.get("source_uri"),
        status: row.get("status"),
        chunks_created: row.get("chunks_created"),
        graph_nodes: row.get("graph_nodes"),
        graph_edges: row.get("graph_edges"),
        error_message: row.get("error_message"),
        started_at: row.get("started_at"),
        completed_at: row.get("completed_at"),
    }
}
