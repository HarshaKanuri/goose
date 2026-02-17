use anyhow::{Context, Result};
use deadpool_postgres::{Config, Pool, Runtime};
use tokio_postgres::NoTls;
use tracing::info;

use super::graph::GraphStore;
use super::relational::RelationalStore;
use super::vector::VectorStore;

/// Configuration for connecting to PostgreSQL with AGE + pgvector.
#[derive(Debug, Clone)]
pub struct ContextStoreConfig {
    pub host: String,
    pub port: u16,
    pub database: String,
    pub user: String,
    pub password: String,
    pub max_pool_size: usize,
}

impl Default for ContextStoreConfig {
    fn default() -> Self {
        Self {
            host: "localhost".to_string(),
            port: 5432,
            database: "goose_context".to_string(),
            user: "goose".to_string(),
            password: String::new(),
            max_pool_size: 16,
        }
    }
}

impl ContextStoreConfig {
    /// Load config from environment variables.
    pub fn from_env() -> Self {
        Self {
            host: std::env::var("GOOSE_CTX_DB_HOST").unwrap_or_else(|_| "localhost".to_string()),
            port: std::env::var("GOOSE_CTX_DB_PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(5432),
            database: std::env::var("GOOSE_CTX_DB_NAME")
                .unwrap_or_else(|_| "goose_context".to_string()),
            user: std::env::var("GOOSE_CTX_DB_USER").unwrap_or_else(|_| "goose".to_string()),
            password: std::env::var("GOOSE_CTX_DB_PASSWORD").unwrap_or_default(),
            max_pool_size: std::env::var("GOOSE_CTX_DB_POOL_SIZE")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(16),
        }
    }
}

/// Three-tier context store backed by PostgreSQL (AGE + pgvector + relational).
///
/// Provides unified access to:
/// - Graph queries via Apache AGE (Cypher)
/// - Vector similarity search via pgvector
/// - Structured data via standard SQL
#[derive(Clone)]
pub struct ContextStore {
    pool: Pool,
    graph: GraphStore,
    vector: VectorStore,
    relational: RelationalStore,
}

impl ContextStore {
    /// Create a new ContextStore from configuration.
    pub async fn new(config: ContextStoreConfig) -> Result<Self> {
        let mut cfg = Config::new();
        cfg.host = Some(config.host);
        cfg.port = Some(config.port);
        cfg.dbname = Some(config.database);
        cfg.user = Some(config.user);
        cfg.password = Some(config.password);

        let pool = cfg
            .create_pool(Some(Runtime::Tokio1), NoTls)
            .context("Failed to create database connection pool")?;

        // Verify connectivity
        let client = pool
            .get()
            .await
            .context("Failed to connect to PostgreSQL")?;

        // Ensure extensions are loaded
        client
            .batch_execute("CREATE EXTENSION IF NOT EXISTS vector; LOAD 'age'; SET search_path = ag_catalog, \"$user\", public;")
            .await
            .context("Failed to load PostgreSQL extensions (vector, age). Ensure pgvector and Apache AGE are installed.")?;

        info!("Connected to PostgreSQL with AGE + pgvector extensions");

        let graph = GraphStore::new(pool.clone());
        let vector = VectorStore::new(pool.clone());
        let relational = RelationalStore::new(pool.clone());

        Ok(Self {
            pool,
            graph,
            vector,
            relational,
        })
    }

    /// Run database migrations.
    pub async fn run_migrations(&self) -> Result<()> {
        super::migrations::run(&self.pool).await
    }

    /// Access the graph store (Apache AGE).
    pub fn graph(&self) -> &GraphStore {
        &self.graph
    }

    /// Access the vector store (pgvector).
    pub fn vector(&self) -> &VectorStore {
        &self.vector
    }

    /// Access the relational store (standard SQL).
    pub fn relational(&self) -> &RelationalStore {
        &self.relational
    }

    /// Get a raw database client for custom queries.
    pub async fn get_client(
        &self,
    ) -> Result<deadpool_postgres::Client> {
        self.pool
            .get()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to get database client: {}", e))
    }
}
