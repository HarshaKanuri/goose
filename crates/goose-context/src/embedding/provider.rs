use anyhow::Result;
use async_trait::async_trait;

/// Trait for embedding providers.
///
/// Implementations can use local models (air-gap compatible) or API-based services.
/// The default implementation uses e5-small-v2 (384 dimensions) for air-gap deployments.
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    /// Name of this embedding provider.
    fn name(&self) -> &str;

    /// Dimensionality of the output embeddings.
    fn dimensions(&self) -> usize;

    /// Generate an embedding for a single text input.
    async fn embed(&self, text: &str) -> Result<Vec<f32>>;

    /// Generate embeddings for a batch of text inputs.
    ///
    /// Default implementation calls `embed` sequentially.
    /// Override for providers that support batch APIs.
    async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let mut results = Vec::with_capacity(texts.len());
        for text in texts {
            results.push(self.embed(text).await?);
        }
        Ok(results)
    }
}
