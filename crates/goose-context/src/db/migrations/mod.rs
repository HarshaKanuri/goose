use anyhow::{Context, Result};
use deadpool_postgres::Pool;
use tracing::info;

/// SQL migration scripts embedded as constants.
/// In production these would use refinery or sqlx migrations,
/// but we embed them for simplicity and air-gap compatibility.

const V001_INIT_SCHEMA: &str = r#"
-- Agent runs (extends Goose sessions with enterprise fields)
CREATE TABLE IF NOT EXISTS agent_runs (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    session_id      VARCHAR(100) NOT NULL,
    trigger_source  VARCHAR(50) NOT NULL,
    trigger_ref     TEXT,
    sandbox_id      VARCHAR(100),
    template_id     UUID,
    status          VARCHAR(20) NOT NULL DEFAULT 'pending',
    pr_url          TEXT,
    started_at      TIMESTAMPTZ,
    completed_at    TIMESTAMPTZ,
    created_by      VARCHAR(200) NOT NULL,
    approved_by     VARCHAR(200),
    metadata        JSONB DEFAULT '{}',
    created_at      TIMESTAMPTZ DEFAULT now(),
    updated_at      TIMESTAMPTZ DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_agent_runs_session ON agent_runs(session_id);
CREATE INDEX IF NOT EXISTS idx_agent_runs_status ON agent_runs(status);
CREATE INDEX IF NOT EXISTS idx_agent_runs_trigger ON agent_runs(trigger_source);

-- Full audit trail (banking compliance requirement)
CREATE TABLE IF NOT EXISTS audit_log (
    id              BIGSERIAL PRIMARY KEY,
    run_id          UUID REFERENCES agent_runs(id) ON DELETE SET NULL,
    timestamp       TIMESTAMPTZ DEFAULT now(),
    action_type     VARCHAR(50) NOT NULL,
    action_detail   JSONB NOT NULL,
    tool_name       VARCHAR(200),
    input_hash      VARCHAR(64),
    output_summary  TEXT,
    token_count     INT,
    gate_name       VARCHAR(100),
    gate_result     VARCHAR(20),
    risk_level      VARCHAR(20) DEFAULT 'low'
);

CREATE INDEX IF NOT EXISTS idx_audit_log_run ON audit_log(run_id);
CREATE INDEX IF NOT EXISTS idx_audit_log_timestamp ON audit_log(timestamp);
CREATE INDEX IF NOT EXISTS idx_audit_log_action ON audit_log(action_type);
CREATE INDEX IF NOT EXISTS idx_audit_log_gate ON audit_log(gate_name) WHERE gate_name IS NOT NULL;

-- Sandbox templates
CREATE TABLE IF NOT EXISTS sandbox_templates (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name            VARCHAR(200) NOT NULL,
    repo_url        TEXT,
    description     TEXT,
    base_image      VARCHAR(500) NOT NULL,
    dockerfile      TEXT NOT NULL,
    setup_script    TEXT,
    languages       TEXT[] DEFAULT '{}',
    build_tools     TEXT[] DEFAULT '{}',
    pre_warm_count  INT DEFAULT 2,
    resource_limits JSONB DEFAULT '{"cpu": 2, "memory_mb": 4096, "disk_mb": 10240}',
    gates_config    JSONB DEFAULT '{"gates": []}',
    tools_config    JSONB DEFAULT '{"tools": []}',
    network_policy  JSONB DEFAULT '{"allow": [], "deny_all_external": true}',
    status          VARCHAR(20) DEFAULT 'draft',
    scan_results    JSONB,
    created_at      TIMESTAMPTZ DEFAULT now(),
    updated_at      TIMESTAMPTZ DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_sandbox_templates_status ON sandbox_templates(status);
CREATE INDEX IF NOT EXISTS idx_sandbox_templates_name ON sandbox_templates(name);

-- Document ingestion tracking
CREATE TABLE IF NOT EXISTS ingestion_jobs (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    source_type     VARCHAR(50) NOT NULL,
    source_uri      TEXT NOT NULL,
    status          VARCHAR(20) DEFAULT 'pending',
    chunks_created  INT DEFAULT 0,
    graph_nodes     INT DEFAULT 0,
    graph_edges     INT DEFAULT 0,
    error_message   TEXT,
    started_at      TIMESTAMPTZ,
    completed_at    TIMESTAMPTZ,
    created_at      TIMESTAMPTZ DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_ingestion_jobs_status ON ingestion_jobs(status);

-- Migration tracking
CREATE TABLE IF NOT EXISTS schema_migrations (
    version     INT PRIMARY KEY,
    applied_at  TIMESTAMPTZ DEFAULT now()
);
"#;

const V002_AGE_GRAPH_SCHEMA: &str = r#"
-- Create the codebase graph in Apache AGE
SELECT * FROM ag_catalog.create_graph('codebase');

-- Vertex labels
SELECT * FROM ag_catalog.create_vlabel('codebase', 'File');
SELECT * FROM ag_catalog.create_vlabel('codebase', 'Function');
SELECT * FROM ag_catalog.create_vlabel('codebase', 'Class');
SELECT * FROM ag_catalog.create_vlabel('codebase', 'Service');
SELECT * FROM ag_catalog.create_vlabel('codebase', 'Team');
SELECT * FROM ag_catalog.create_vlabel('codebase', 'Package');
SELECT * FROM ag_catalog.create_vlabel('codebase', 'Config');
SELECT * FROM ag_catalog.create_vlabel('codebase', 'Document');

-- Edge labels
SELECT * FROM ag_catalog.create_elabel('codebase', 'IMPORTS');
SELECT * FROM ag_catalog.create_elabel('codebase', 'CALLS');
SELECT * FROM ag_catalog.create_elabel('codebase', 'DEPENDS_ON');
SELECT * FROM ag_catalog.create_elabel('codebase', 'OWNS');
SELECT * FROM ag_catalog.create_elabel('codebase', 'DEPLOYS_TO');
SELECT * FROM ag_catalog.create_elabel('codebase', 'EXTENDS');
SELECT * FROM ag_catalog.create_elabel('codebase', 'IMPLEMENTS');
SELECT * FROM ag_catalog.create_elabel('codebase', 'DEFINED_IN');
SELECT * FROM ag_catalog.create_elabel('codebase', 'TESTED_BY');
SELECT * FROM ag_catalog.create_elabel('codebase', 'DOCUMENTS');
"#;

const V003_PGVECTOR_TABLES: &str = r#"
-- Embedding storage with pgvector
CREATE TABLE IF NOT EXISTS embeddings (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    source_type VARCHAR(50) NOT NULL,
    source_id   VARCHAR(500) NOT NULL,
    chunk_index INT NOT NULL DEFAULT 0,
    content     TEXT NOT NULL,
    metadata    JSONB NOT NULL DEFAULT '{}',
    embedding   vector(384) NOT NULL,
    created_at  TIMESTAMPTZ DEFAULT now(),
    updated_at  TIMESTAMPTZ DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_embeddings_source ON embeddings(source_type, source_id);
CREATE INDEX IF NOT EXISTS idx_embeddings_source_type ON embeddings(source_type);

-- IVFFlat index for cosine similarity search
-- Note: This index should be created AFTER initial data load for best performance.
-- With small datasets, a sequential scan may be faster.
-- For production with >10k embeddings, create with: lists = sqrt(num_rows)
CREATE INDEX IF NOT EXISTS idx_embeddings_vector ON embeddings
    USING ivfflat (embedding vector_cosine_ops) WITH (lists = 100);

-- Unique constraint to prevent duplicate chunks
CREATE UNIQUE INDEX IF NOT EXISTS idx_embeddings_unique_chunk
    ON embeddings(source_type, source_id, chunk_index);
"#;

struct Migration {
    version: i32,
    name: &'static str,
    sql: &'static str,
    /// If true, migration needs AGE loaded first
    requires_age: bool,
}

const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "init_schema",
        sql: V001_INIT_SCHEMA,
        requires_age: false,
    },
    Migration {
        version: 2,
        name: "age_graph_schema",
        sql: V002_AGE_GRAPH_SCHEMA,
        requires_age: true,
    },
    Migration {
        version: 3,
        name: "pgvector_tables",
        sql: V003_PGVECTOR_TABLES,
        requires_age: false,
    },
];

/// Run all pending migrations.
pub async fn run(pool: &Pool) -> Result<()> {
    let client = pool
        .get()
        .await
        .context("Failed to get DB client for migrations")?;

    // Ensure migration tracking table exists
    client
        .batch_execute(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version INT PRIMARY KEY,
                applied_at TIMESTAMPTZ DEFAULT now()
            );",
        )
        .await
        .context("Failed to create schema_migrations table")?;

    // Get applied versions
    let rows = client
        .query("SELECT version FROM schema_migrations ORDER BY version", &[])
        .await
        .context("Failed to query schema_migrations")?;

    let applied: Vec<i32> = rows.iter().map(|r| r.get("version")).collect();

    for migration in MIGRATIONS {
        if applied.contains(&migration.version) {
            continue;
        }

        info!(
            "Applying migration V{:03}: {}",
            migration.version, migration.name
        );

        if migration.requires_age {
            // Load AGE extension before running graph migrations
            client
                .batch_execute("LOAD 'age'; SET search_path = ag_catalog, \"$user\", public;")
                .await
                .context("Failed to load AGE for migration")?;
        }

        client
            .batch_execute(migration.sql)
            .await
            .with_context(|| {
                format!(
                    "Failed to apply migration V{:03}: {}",
                    migration.version, migration.name
                )
            })?;

        client
            .execute(
                "INSERT INTO schema_migrations (version) VALUES ($1)",
                &[&migration.version],
            )
            .await
            .with_context(|| {
                format!(
                    "Failed to record migration V{:03}",
                    migration.version
                )
            })?;

        info!(
            "Applied migration V{:03}: {}",
            migration.version, migration.name
        );
    }

    info!("All migrations applied successfully");
    Ok(())
}
