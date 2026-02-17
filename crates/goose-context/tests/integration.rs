/// Integration tests for goose-context.
///
/// These tests require a live PostgreSQL instance with Apache AGE and pgvector.
/// Skip them when the env var is not set:
///
///   GOOSE_CTX_DB_HOST=localhost \
///   GOOSE_CTX_DB_USER=goose \
///   GOOSE_CTX_DB_PASSWORD=goose \
///   GOOSE_CTX_DB_NAME=goose_context \
///   cargo test -p goose-context --test integration -- --ignored
///
/// Or run everything including integration tests:
///
///   GOOSE_TEST_INTEGRATION=1 cargo test -p goose-context --test integration
use goose_context::{
    db::pool::{ContextStore, ContextStoreConfig},
    db::relational::CreateAgentRun,
    db::vector::{EmbeddingInput, VectorSearchParams},
    embedding::{local::LocalEmbeddingProvider, EmbeddingProvider},
};
use serde_json::json;
use std::path::PathBuf;

/// Returns true only when integration env var is explicitly set.
fn integration_enabled() -> bool {
    std::env::var("GOOSE_TEST_INTEGRATION")
        .map(|v| v == "1" || v.to_lowercase() == "true")
        .unwrap_or(false)
}

/// Helper to create a test ContextStore from env vars.
async fn test_store() -> ContextStore {
    let config = ContextStoreConfig::from_env();
    let store = ContextStore::new(config)
        .await
        .expect("Failed to connect to PostgreSQL. Set GOOSE_CTX_DB_* env vars.");
    store
        .run_migrations()
        .await
        .expect("Failed to run migrations");
    store
}

// ─── Relational Store ─────────────────────────────────────────────────────────

#[tokio::test]
#[ignore = "requires PostgreSQL — run with GOOSE_TEST_INTEGRATION=1"]
async fn test_create_and_get_agent_run() {
    if !integration_enabled() {
        return;
    }
    let store = test_store().await;

    let run = store
        .relational()
        .create_agent_run(CreateAgentRun {
            session_id: "test-session-001".to_string(),
            trigger_source: "slack".to_string(),
            trigger_ref: Some("https://company.slack.com/archives/C123/p456".to_string()),
            created_by: "test-user@bank.com".to_string(),
            metadata: Some(json!({"test": true})),
        })
        .await
        .expect("Failed to create agent run");

    assert_eq!(run.status, "pending");
    assert_eq!(run.trigger_source, "slack");

    let fetched = store
        .relational()
        .get_agent_run(run.id)
        .await
        .expect("Failed to fetch agent run")
        .expect("Agent run should exist");

    assert_eq!(fetched.id, run.id);
    assert_eq!(fetched.session_id, "test-session-001");
}

#[tokio::test]
#[ignore = "requires PostgreSQL — run with GOOSE_TEST_INTEGRATION=1"]
async fn test_audit_log_append_and_query() {
    if !integration_enabled() {
        return;
    }
    let store = test_store().await;

    // Create a run to attach the log to
    let run = store
        .relational()
        .create_agent_run(CreateAgentRun {
            session_id: "test-audit-session".to_string(),
            trigger_source: "ci".to_string(),
            trigger_ref: None,
            created_by: "ci-bot@bank.com".to_string(),
            metadata: None,
        })
        .await
        .unwrap();

    let entry_id = store
        .relational()
        .append_audit_log(goose_context::db::relational::CreateAuditLog {
            run_id: Some(run.id),
            action_type: "tool_call".to_string(),
            action_detail: json!({"tool": "developer__shell", "command": "mvn test"}),
            tool_name: Some("developer__shell".to_string()),
            input_hash: Some("abc123".to_string()),
            output_summary: Some("Tests passed: 42, failed: 0".to_string()),
            token_count: Some(150),
            gate_name: Some("test".to_string()),
            gate_result: Some("pass".to_string()),
            risk_level: Some("low".to_string()),
        })
        .await
        .expect("Failed to append audit log");

    assert!(entry_id > 0);

    // Query it back
    let entries = store
        .relational()
        .query_audit_log(Some(run.id), None, 10)
        .await
        .expect("Failed to query audit log");

    assert!(!entries.is_empty(), "Should have at least one audit entry");
    let entry = entries.first().unwrap();
    assert_eq!(entry.gate_name.as_deref(), Some("test"));
    assert_eq!(entry.gate_result.as_deref(), Some("pass"));
}

#[tokio::test]
#[ignore = "requires PostgreSQL — run with GOOSE_TEST_INTEGRATION=1"]
async fn test_read_only_sql_query() {
    if !integration_enabled() {
        return;
    }
    let store = test_store().await;

    let rows = store
        .relational()
        .read_only_query("SELECT COUNT(*) as count FROM agent_runs")
        .await
        .expect("Failed to run read-only query");

    assert!(!rows.is_empty());
    let count_col = rows[0].first().unwrap();
    assert_eq!(count_col.0, "count");
}

#[tokio::test]
#[ignore = "requires PostgreSQL — run with GOOSE_TEST_INTEGRATION=1"]
async fn test_read_only_sql_rejects_writes() {
    if !integration_enabled() {
        return;
    }
    let store = test_store().await;

    let result = store
        .relational()
        .read_only_query("DELETE FROM agent_runs WHERE id = '00000000-0000-0000-0000-000000000000'")
        .await;

    assert!(result.is_err(), "DELETE should be rejected");
    assert!(
        result.unwrap_err().to_string().contains("Only SELECT"),
        "Error should mention SELECT restriction"
    );
}

// ─── Vector Store ─────────────────────────────────────────────────────────────

fn make_test_provider() -> impl EmbeddingProvider {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("model.onnx"), b"fake").unwrap();
    std::fs::write(dir.path().join("tokenizer.json"), b"{}").unwrap();
    // Leak the tempdir so it lives for the test
    let path = dir.into_path();
    LocalEmbeddingProvider::new(path).unwrap()
}

#[tokio::test]
#[ignore = "requires PostgreSQL — run with GOOSE_TEST_INTEGRATION=1"]
async fn test_vector_upsert_and_search() {
    if !integration_enabled() {
        return;
    }
    let store = test_store().await;
    let provider = make_test_provider();

    let content = "SWIFT MT103 single customer credit transfer message format";
    let embedding = provider.embed(content).await.unwrap();

    let id = store
        .vector()
        .upsert(&EmbeddingInput {
            source_type: "doc".to_string(),
            source_id: "confluence://swift-guide".to_string(),
            chunk_index: 0,
            content: content.to_string(),
            metadata: json!({"title": "SWIFT Guide", "space": "PAYMENTS"}),
            embedding,
        })
        .await
        .expect("Failed to upsert embedding");

    // Search for it
    let query_embedding = provider.embed("SWIFT payment message format").await.unwrap();
    let results = store
        .vector()
        .search(&VectorSearchParams {
            query_embedding,
            source_type: Some("doc".to_string()),
            limit: 5,
            similarity_threshold: 0.0, // Low threshold for deterministic test embeddings
        })
        .await
        .expect("Failed to search embeddings");

    assert!(!results.is_empty(), "Should find at least one result");
    let found = results
        .iter()
        .find(|r| r.id == id)
        .expect("Should find the upserted embedding");
    assert_eq!(found.source_id, "confluence://swift-guide");

    // Cleanup
    store
        .vector()
        .delete_source("doc", "confluence://swift-guide")
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires PostgreSQL — run with GOOSE_TEST_INTEGRATION=1"]
async fn test_vector_upsert_idempotent() {
    if !integration_enabled() {
        return;
    }
    let store = test_store().await;
    let provider = make_test_provider();

    let source_id = "confluence://idempotent-test";
    let embedding = provider.embed("idempotent test content").await.unwrap();

    let input = EmbeddingInput {
        source_type: "doc".to_string(),
        source_id: source_id.to_string(),
        chunk_index: 0,
        content: "idempotent test content".to_string(),
        metadata: json!({}),
        embedding,
    };

    let id1 = store.vector().upsert(&input).await.unwrap();
    let id2 = store.vector().upsert(&input).await.unwrap();

    // Same id returned on upsert — no duplicate rows
    assert_eq!(id1, id2, "Upsert should be idempotent");

    let count = store.vector().count(Some("doc")).await.unwrap();
    assert!(count >= 1);

    store
        .vector()
        .delete_source("doc", source_id)
        .await
        .unwrap();
}

// ─── Sandbox Templates ───────────────────────────────────────────────────────

#[tokio::test]
#[ignore = "requires PostgreSQL — run with GOOSE_TEST_INTEGRATION=1"]
async fn test_create_and_list_templates() {
    if !integration_enabled() {
        return;
    }
    let store = test_store().await;

    let template = goose_context::db::relational::SandboxTemplate {
        id: uuid::Uuid::new_v4(),
        name: "payment-service-java17".to_string(),
        repo_url: Some("git@git.bank.internal:payments/payment-service.git".to_string()),
        description: Some("Java 17 + Maven 3.9 for payment-service repo".to_string()),
        base_image: "ubuntu:22.04".to_string(),
        dockerfile: "FROM ubuntu:22.04\nRUN apt-get install -y openjdk-17-jdk maven\n".to_string(),
        setup_script: Some("git config --global user.email ci-agent@bank.com".to_string()),
        languages: vec!["java".to_string()],
        build_tools: vec!["maven".to_string()],
        pre_warm_count: 3,
        resource_limits: json!({"cpu": 4, "memory_mb": 8192, "disk_mb": 20480}),
        gates_config: json!({"gates": [
            {"name": "lint", "command": "mvn checkstyle:check", "timeout": 30},
            {"name": "test",  "command": "mvn test", "timeout": 300}
        ]}),
        tools_config: json!({"tools": ["developer__shell", "developer__text_editor"]}),
        network_policy: json!({"deny_all_external": true, "allow": ["git.bank.internal", "nexus.bank.internal"]}),
        status: "draft".to_string(),
        scan_results: Some(json!({"language_detected": "java", "build_tool": "maven"})),
        created_at: chrono::Utc::now(),
    };

    let id = store
        .relational()
        .create_template(&template)
        .await
        .expect("Failed to create template");

    // List draft templates
    let drafts = store
        .relational()
        .list_templates(Some("draft"))
        .await
        .unwrap();

    assert!(drafts.iter().any(|t| t.id == id), "Created template should appear in draft list");

    // Approve it
    store.relational().approve_template(id).await.unwrap();

    let approved = store
        .relational()
        .list_templates(Some("approved"))
        .await
        .unwrap();

    assert!(approved.iter().any(|t| t.id == id), "Template should appear in approved list after approval");
}
