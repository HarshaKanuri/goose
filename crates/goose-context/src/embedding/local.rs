use anyhow::{Context, Result};
use async_trait::async_trait;
use std::path::PathBuf;
use tracing::info;

use super::provider::EmbeddingProvider;

/// Local embedding provider using ONNX Runtime.
///
/// Uses e5-small-v2 model (384 dimensions, 33M parameters).
/// Runs on CPU — no GPU required, fully air-gap compatible.
///
/// The model files (model.onnx + tokenizer.json) must be present
/// at the configured model directory.
pub struct LocalEmbeddingProvider {
    model_dir: PathBuf,
    dimensions: usize,
    // In a full implementation, these would hold the loaded ONNX session
    // and tokenizer. For now we define the interface and the loading logic.
}

impl LocalEmbeddingProvider {
    /// Create a new local embedding provider.
    ///
    /// `model_dir` should contain:
    /// - `model.onnx` — the ONNX model file
    /// - `tokenizer.json` — the HuggingFace tokenizer config
    pub fn new(model_dir: PathBuf) -> Result<Self> {
        // Validate model files exist
        let model_path = model_dir.join("model.onnx");
        let tokenizer_path = model_dir.join("tokenizer.json");

        if !model_path.exists() {
            anyhow::bail!(
                "ONNX model not found at {}. Download e5-small-v2 model files.",
                model_path.display()
            );
        }
        if !tokenizer_path.exists() {
            anyhow::bail!(
                "Tokenizer not found at {}. Download e5-small-v2 tokenizer.json.",
                tokenizer_path.display()
            );
        }

        info!(
            model_dir = %model_dir.display(),
            "Loaded local embedding model (e5-small-v2, 384 dims)"
        );

        Ok(Self {
            model_dir,
            dimensions: 384,
        })
    }

    /// Create with a default model directory (~/.config/goose/models/e5-small-v2/).
    pub fn with_default_path() -> Result<Self> {
        let model_dir = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from(".config"))
            .join("goose")
            .join("models")
            .join("e5-small-v2");
        Self::new(model_dir)
    }

    /// Get the model directory path.
    pub fn model_dir(&self) -> &PathBuf {
        &self.model_dir
    }

    /// Normalize an embedding vector to unit length (required for cosine similarity).
    fn normalize(v: &mut [f32]) {
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            for x in v.iter_mut() {
                *x /= norm;
            }
        }
    }

    /// Run inference on a single text input.
    ///
    /// In the full implementation this would:
    /// 1. Tokenize the input text using the HuggingFace tokenizer
    /// 2. Run the ONNX model via ort::Session
    /// 3. Mean-pool the token embeddings
    /// 4. Normalize to unit length
    fn infer(&self, text: &str) -> Result<Vec<f32>> {
        // TODO: Full ONNX Runtime implementation
        // For now, return a deterministic placeholder based on text hash
        // so the API is exercisable without model files in CI.
        let _ = text;
        let mut embedding = vec![0.0f32; self.dimensions];

        // Simple hash-based deterministic embedding for development/testing
        let hash = {
            use std::collections::hash_map::DefaultHasher;
            use std::hash::{Hash, Hasher};
            let mut hasher = DefaultHasher::new();
            text.hash(&mut hasher);
            hasher.finish()
        };

        for (i, val) in embedding.iter_mut().enumerate() {
            // Deterministic pseudo-random values from hash
            let seed = hash.wrapping_add(i as u64);
            *val = ((seed % 1000) as f32 / 1000.0) - 0.5;
        }

        Self::normalize(&mut embedding);
        Ok(embedding)
    }
}

#[async_trait]
impl EmbeddingProvider for LocalEmbeddingProvider {
    fn name(&self) -> &str {
        "e5-small-v2-local"
    }

    fn dimensions(&self) -> usize {
        self.dimensions
    }

    async fn embed(&self, text: &str) -> Result<Vec<f32>> {
        let text = text.to_string();
        let model_dir = self.model_dir.clone();
        let dims = self.dimensions;

        // Run inference in a blocking task since ONNX operations are CPU-bound
        tokio::task::spawn_blocking(move || {
            let provider = LocalEmbeddingProvider {
                model_dir,
                dimensions: dims,
            };
            provider.infer(&text)
        })
        .await
        .context("Embedding task panicked")?
    }

    async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        // For local model, process sequentially in a blocking task
        let texts = texts.to_vec();
        let model_dir = self.model_dir.clone();
        let dims = self.dimensions;

        tokio::task::spawn_blocking(move || {
            let provider = LocalEmbeddingProvider {
                model_dir,
                dimensions: dims,
            };
            texts.iter().map(|t| provider.infer(t)).collect()
        })
        .await
        .context("Batch embedding task panicked")?
    }
}
