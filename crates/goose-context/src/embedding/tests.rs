/// Unit tests for the embedding layer — no PostgreSQL required.
#[cfg(test)]
mod tests {
    use crate::embedding::{
        local::LocalEmbeddingProvider,
        provider::EmbeddingProvider,
    };
    use std::path::PathBuf;

    // ─── LocalEmbeddingProvider ───────────────────────────────────────────────

    #[test]
    fn test_local_provider_rejects_missing_model_dir() {
        let result = LocalEmbeddingProvider::new(PathBuf::from("/nonexistent/path"));
        assert!(result.is_err(), "Should fail when model dir doesn't exist");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("ONNX model not found"),
            "Error should mention missing model: {}",
            err
        );
    }

    #[test]
    fn test_local_provider_dimensions() {
        // We can check the dimension constant without loading a model
        // by temporarily creating a fake model dir.
        let dir = tempfile::tempdir().unwrap();
        let model_path = dir.path().join("model.onnx");
        let tokenizer_path = dir.path().join("tokenizer.json");
        std::fs::write(&model_path, b"fake onnx").unwrap();
        std::fs::write(&tokenizer_path, b"{}").unwrap();

        let provider = LocalEmbeddingProvider::new(dir.path().to_path_buf()).unwrap();
        assert_eq!(provider.dimensions(), 384, "e5-small-v2 should output 384-dim embeddings");
    }

    #[tokio::test]
    async fn test_local_provider_returns_normalized_embedding() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("model.onnx"), b"fake").unwrap();
        std::fs::write(dir.path().join("tokenizer.json"), b"{}").unwrap();

        let provider = LocalEmbeddingProvider::new(dir.path().to_path_buf()).unwrap();
        let embedding = provider.embed("hello world").await.unwrap();

        assert_eq!(embedding.len(), 384, "Embedding should be 384 dimensions");

        // Check normalization: L2 norm should be ~1.0
        let norm: f32 = embedding.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!(
            (norm - 1.0).abs() < 1e-5,
            "Embedding should be unit-normalized, got norm={}",
            norm
        );
    }

    #[tokio::test]
    async fn test_local_provider_different_texts_different_embeddings() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("model.onnx"), b"fake").unwrap();
        std::fs::write(dir.path().join("tokenizer.json"), b"{}").unwrap();

        let provider = LocalEmbeddingProvider::new(dir.path().to_path_buf()).unwrap();
        let e1 = provider.embed("add a new feature to the payment service").await.unwrap();
        let e2 = provider.embed("fix the database connection pool timeout").await.unwrap();

        // Embeddings for different texts should differ
        let diff: f32 = e1.iter().zip(e2.iter()).map(|(a, b)| (a - b).abs()).sum();
        assert!(diff > 0.01, "Different inputs should produce different embeddings");
    }

    #[tokio::test]
    async fn test_local_provider_same_text_same_embedding() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("model.onnx"), b"fake").unwrap();
        std::fs::write(dir.path().join("tokenizer.json"), b"{}").unwrap();

        let provider = LocalEmbeddingProvider::new(dir.path().to_path_buf()).unwrap();
        let text = "implement SWIFT MT103 message parser";
        let e1 = provider.embed(text).await.unwrap();
        let e2 = provider.embed(text).await.unwrap();

        // Same text should produce identical embeddings (deterministic)
        assert_eq!(e1, e2, "Same input should always produce same embedding");
    }

    #[tokio::test]
    async fn test_batch_embed_matches_individual() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("model.onnx"), b"fake").unwrap();
        std::fs::write(dir.path().join("tokenizer.json"), b"{}").unwrap();

        let provider = LocalEmbeddingProvider::new(dir.path().to_path_buf()).unwrap();
        let texts = vec![
            "payment gateway integration".to_string(),
            "credit risk model update".to_string(),
            "KYC compliance check".to_string(),
        ];

        let batch = provider.embed_batch(&texts).await.unwrap();
        assert_eq!(batch.len(), 3);

        for (i, text) in texts.iter().enumerate() {
            let individual = provider.embed(text).await.unwrap();
            assert_eq!(
                batch[i], individual,
                "Batch embedding for item {} should match individual embed",
                i
            );
        }
    }
}
