use anyhow::{Context, Result};
use deadpool_postgres::Pool;
use pgvector::Vector;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::debug;
use uuid::Uuid;

/// Vector store backed by pgvector on PostgreSQL.
///
/// Handles embedding storage and similarity search operations.
#[derive(Clone)]
pub struct VectorStore {
    pool: Pool,
}

/// An embedding record stored in the vector database.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingRecord {
    pub id: Uuid,
    pub source_type: String,
    pub source_id: String,
    pub chunk_index: i32,
    pub content: String,
    pub metadata: Value,
    pub similarity: Option<f64>,
}

/// Input for upserting an embedding.
#[derive(Debug, Clone)]
pub struct EmbeddingInput {
    pub source_type: String,
    pub source_id: String,
    pub chunk_index: i32,
    pub content: String,
    pub metadata: Value,
    pub embedding: Vec<f32>,
}

/// Search parameters for vector similarity search.
#[derive(Debug, Clone)]
pub struct VectorSearchParams {
    pub query_embedding: Vec<f32>,
    pub source_type: Option<String>,
    pub limit: i64,
    pub similarity_threshold: f64,
}

impl Default for VectorSearchParams {
    fn default() -> Self {
        Self {
            query_embedding: Vec::new(),
            source_type: None,
            limit: 10,
            similarity_threshold: 0.5,
        }
    }
}

impl VectorStore {
    pub fn new(pool: Pool) -> Self {
        Self { pool }
    }

    /// Upsert a single embedding (insert or update on conflict).
    pub async fn upsert(&self, input: &EmbeddingInput) -> Result<Uuid> {
        let client = self.pool.get().await.context("Failed to get DB client")?;
        let embedding = Vector::from(input.embedding.clone());

        let row = client
            .query_one(
                "INSERT INTO embeddings (source_type, source_id, chunk_index, content, metadata, embedding)
                 VALUES ($1, $2, $3, $4, $5, $6)
                 ON CONFLICT (source_type, source_id, chunk_index)
                 DO UPDATE SET content = $4, metadata = $5, embedding = $6, updated_at = now()
                 RETURNING id",
                &[
                    &input.source_type,
                    &input.source_id,
                    &input.chunk_index,
                    &input.content,
                    &input.metadata,
                    &embedding,
                ],
            )
            .await
            .context("Failed to upsert embedding")?;

        Ok(row.get("id"))
    }

    /// Batch upsert multiple embeddings.
    pub async fn upsert_batch(&self, inputs: &[EmbeddingInput]) -> Result<Vec<Uuid>> {
        let mut ids = Vec::with_capacity(inputs.len());
        // Use a transaction for batch operations
        let mut client = self.pool.get().await.context("Failed to get DB client")?;
        let txn = client
            .transaction()
            .await
            .context("Failed to start transaction")?;

        let stmt = txn
            .prepare(
                "INSERT INTO embeddings (source_type, source_id, chunk_index, content, metadata, embedding)
                 VALUES ($1, $2, $3, $4, $5, $6)
                 ON CONFLICT (source_type, source_id, chunk_index)
                 DO UPDATE SET content = $4, metadata = $5, embedding = $6, updated_at = now()
                 RETURNING id",
            )
            .await
            .context("Failed to prepare batch upsert statement")?;

        for input in inputs {
            let embedding = Vector::from(input.embedding.clone());
            let row = txn
                .query_one(
                    &stmt,
                    &[
                        &input.source_type,
                        &input.source_id,
                        &input.chunk_index,
                        &input.content,
                        &input.metadata,
                        &embedding,
                    ],
                )
                .await
                .with_context(|| {
                    format!(
                        "Failed to upsert embedding for {}:{}",
                        input.source_type, input.source_id
                    )
                })?;
            ids.push(row.get("id"));
        }

        txn.commit().await.context("Failed to commit batch upsert")?;
        debug!(count = ids.len(), "Batch upserted embeddings");
        Ok(ids)
    }

    /// Search for similar embeddings using cosine distance.
    pub async fn search(&self, params: &VectorSearchParams) -> Result<Vec<EmbeddingRecord>> {
        let client = self.pool.get().await.context("Failed to get DB client")?;
        let embedding = Vector::from(params.query_embedding.clone());

        let (sql, query_params): (String, Vec<Box<dyn tokio_postgres::types::ToSql + Sync>>) =
            if let Some(ref source_type) = params.source_type {
                (
                    "SELECT id, source_type, source_id, chunk_index, content, metadata,
                            1 - (embedding <=> $1) as similarity
                     FROM embeddings
                     WHERE source_type = $2
                       AND 1 - (embedding <=> $1) >= $3
                     ORDER BY embedding <=> $1
                     LIMIT $4"
                        .to_string(),
                    vec![
                        Box::new(embedding),
                        Box::new(source_type.clone()),
                        Box::new(params.similarity_threshold),
                        Box::new(params.limit),
                    ],
                )
            } else {
                (
                    "SELECT id, source_type, source_id, chunk_index, content, metadata,
                            1 - (embedding <=> $1) as similarity
                     FROM embeddings
                     WHERE 1 - (embedding <=> $1) >= $2
                     ORDER BY embedding <=> $1
                     LIMIT $3"
                        .to_string(),
                    vec![
                        Box::new(embedding),
                        Box::new(params.similarity_threshold),
                        Box::new(params.limit),
                    ],
                )
            };

        let param_refs: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> =
            query_params.iter().map(|p| p.as_ref()).collect();

        let rows = client
            .query(&sql, &param_refs)
            .await
            .context("Vector similarity search failed")?;

        let records = rows
            .iter()
            .map(|row| EmbeddingRecord {
                id: row.get("id"),
                source_type: row.get("source_type"),
                source_id: row.get("source_id"),
                chunk_index: row.get("chunk_index"),
                content: row.get("content"),
                metadata: row.get("metadata"),
                similarity: row.get("similarity"),
            })
            .collect();

        Ok(records)
    }

    /// Delete all embeddings for a given source.
    pub async fn delete_source(&self, source_type: &str, source_id: &str) -> Result<u64> {
        let client = self.pool.get().await.context("Failed to get DB client")?;
        let count = client
            .execute(
                "DELETE FROM embeddings WHERE source_type = $1 AND source_id = $2",
                &[&source_type, &source_id],
            )
            .await
            .context("Failed to delete embeddings")?;
        Ok(count)
    }

    /// Delete all embeddings for a given source type.
    pub async fn delete_source_type(&self, source_type: &str) -> Result<u64> {
        let client = self.pool.get().await.context("Failed to get DB client")?;
        let count = client
            .execute(
                "DELETE FROM embeddings WHERE source_type = $1",
                &[&source_type],
            )
            .await
            .context("Failed to delete embeddings by type")?;
        Ok(count)
    }

    /// Count embeddings, optionally filtered by source type.
    pub async fn count(&self, source_type: Option<&str>) -> Result<i64> {
        let client = self.pool.get().await.context("Failed to get DB client")?;
        let row = if let Some(st) = source_type {
            client
                .query_one(
                    "SELECT COUNT(*) as count FROM embeddings WHERE source_type = $1",
                    &[&st],
                )
                .await?
        } else {
            client
                .query_one("SELECT COUNT(*) as count FROM embeddings", &[])
                .await?
        };
        Ok(row.get("count"))
    }
}
