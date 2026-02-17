use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::provider::EmbeddingProvider;

/// API-based embedding provider for non-air-gapped environments.
///
/// Supports OpenAI-compatible embedding APIs.
/// Falls back gracefully if the API is unavailable.
pub struct ApiEmbeddingProvider {
    api_url: String,
    api_key: String,
    model_name: String,
    dimensions: usize,
    client: reqwest::Client,
}

#[derive(Serialize)]
struct EmbeddingRequest {
    model: String,
    input: Vec<String>,
}

#[derive(Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingData>,
}

#[derive(Deserialize)]
struct EmbeddingData {
    embedding: Vec<f32>,
}

impl ApiEmbeddingProvider {
    pub fn new(
        api_url: String,
        api_key: String,
        model_name: String,
        dimensions: usize,
    ) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("Failed to create HTTP client");

        Self {
            api_url,
            api_key,
            model_name,
            dimensions,
            client,
        }
    }

    /// Create from environment variables.
    ///
    /// - `GOOSE_EMBEDDING_API_URL` — API endpoint (default: OpenAI)
    /// - `GOOSE_EMBEDDING_API_KEY` — API key
    /// - `GOOSE_EMBEDDING_MODEL` — Model name (default: text-embedding-3-small)
    /// - `GOOSE_EMBEDDING_DIMS` — Dimensions (default: 384)
    pub fn from_env() -> Result<Self> {
        let api_url = std::env::var("GOOSE_EMBEDDING_API_URL")
            .unwrap_or_else(|_| "https://api.openai.com/v1/embeddings".to_string());
        let api_key = std::env::var("GOOSE_EMBEDDING_API_KEY")
            .context("GOOSE_EMBEDDING_API_KEY must be set for API embedding provider")?;
        let model_name = std::env::var("GOOSE_EMBEDDING_MODEL")
            .unwrap_or_else(|_| "text-embedding-3-small".to_string());
        let dimensions = std::env::var("GOOSE_EMBEDDING_DIMS")
            .ok()
            .and_then(|d| d.parse().ok())
            .unwrap_or(384);

        Ok(Self::new(api_url, api_key, model_name, dimensions))
    }
}

#[async_trait]
impl EmbeddingProvider for ApiEmbeddingProvider {
    fn name(&self) -> &str {
        "api-embeddings"
    }

    fn dimensions(&self) -> usize {
        self.dimensions
    }

    async fn embed(&self, text: &str) -> Result<Vec<f32>> {
        let results = self.embed_batch(&[text.to_string()]).await?;
        results
            .into_iter()
            .next()
            .context("Empty response from embedding API")
    }

    async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let request = EmbeddingRequest {
            model: self.model_name.clone(),
            input: texts.to_vec(),
        };

        let response = self
            .client
            .post(&self.api_url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await
            .context("Failed to call embedding API")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Embedding API returned {}: {}", status, body);
        }

        let result: EmbeddingResponse = response
            .json()
            .await
            .context("Failed to parse embedding API response")?;

        Ok(result.data.into_iter().map(|d| d.embedding).collect())
    }
}
