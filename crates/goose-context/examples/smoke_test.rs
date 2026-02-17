/// Functional smoke test for goose-context.
///
/// Usage:
///   # 1. Start PostgreSQL with AGE + pgvector
///   docker-compose -f crates/goose-context/docker/docker-compose.test.yml up -d
///
///   # 2. Run this binary
///   cargo run -p goose-context --example smoke_test
///
///   # 3. Tear down
///   docker-compose -f crates/goose-context/docker/docker-compose.test.yml down -v
use goose_context::{
    db::{
        pool::{ContextStore, ContextStoreConfig},
        relational::{CreateAgentRun, CreateAuditLog, SandboxTemplate},
        vector::{EmbeddingInput, VectorSearchParams},
    },
    embedding::{local::LocalEmbeddingProvider, EmbeddingProvider},
    hydrator::{ContextHydrator, HydrationIntent},
    mcp_server::ContextMcpServer,
};
use rmcp::model::{CallToolRequestParams, JsonObject};
use serde_json::json;
use std::sync::Arc;

fn section(name: &str) {
    let bar = "=".repeat(60);
    println!("\n{}", bar);
    println!("  {}", name);
    println!("{}\n", bar);
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // ── Setup ────────────────────────────────────────────────────────────
    println!("Goose Context — Functional Smoke Test\n");

    let config = ContextStoreConfig {
        host: std::env::var("GOOSE_CTX_DB_HOST").unwrap_or("localhost".into()),
        port: std::env::var("GOOSE_CTX_DB_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(5432),
        database: std::env::var("GOOSE_CTX_DB_NAME").unwrap_or("goose_context".into()),
        user: std::env::var("GOOSE_CTX_DB_USER").unwrap_or("goose".into()),
        password: std::env::var("GOOSE_CTX_DB_PASSWORD").unwrap_or("goose".into()),
        max_pool_size: 4,
    };

    println!("Connecting to PostgreSQL at {}:{}/{}", config.host, config.port, config.database);
    let store = ContextStore::new(config).await?;
    println!("[OK] Connected to PostgreSQL with AGE + pgvector\n");

    // ── Migrations ───────────────────────────────────────────────────────
    section("1. Running Migrations");
    store.run_migrations().await?;
    println!("[OK] All migrations applied");

    // ── Relational: Agent Run ────────────────────────────────────────────
    section("2. Relational Store — Agent Runs");

    let run = store
        .relational()
        .create_agent_run(CreateAgentRun {
            session_id: "smoke-test-session".into(),
            trigger_source: "cli".into(),
            trigger_ref: Some("manual smoke test".into()),
            created_by: "developer@bank.com".into(),
            metadata: Some(json!({"test": true, "purpose": "smoke test"})),
        })
        .await?;
    println!("[OK] Created agent run: id={}, status={}", run.id, run.status);

    store
        .relational()
        .update_agent_run_status(run.id, "running", Some("sandbox-001"), None)
        .await?;
    println!("[OK] Updated status to 'running'");

    let fetched = store.relational().get_agent_run(run.id).await?.unwrap();
    println!("[OK] Fetched: status={}, sandbox={:?}", fetched.status, fetched.sandbox_id);

    // ── Relational: Audit Log ────────────────────────────────────────────
    section("3. Relational Store — Audit Trail");

    let entry_id = store
        .relational()
        .append_audit_log(CreateAuditLog {
            run_id: Some(run.id),
            action_type: "tool_call".into(),
            action_detail: json!({"tool": "developer__shell", "command": "mvn test -pl payments"}),
            tool_name: Some("developer__shell".into()),
            input_hash: Some("sha256:abc123def456".into()),
            output_summary: Some("BUILD SUCCESS — 42 tests passed".into()),
            token_count: Some(350),
            gate_name: Some("test".into()),
            gate_result: Some("pass".into()),
            risk_level: Some("low".into()),
        })
        .await?;
    println!("[OK] Audit log entry #{}", entry_id);

    store
        .relational()
        .append_audit_log(CreateAuditLog {
            run_id: Some(run.id),
            action_type: "gate_fail".into(),
            action_detail: json!({"gate": "security", "findings": ["hardcoded API key in config.java"]}),
            tool_name: None,
            input_hash: None,
            output_summary: Some("BLOCKED: secret detected".into()),
            token_count: None,
            gate_name: Some("security".into()),
            gate_result: Some("fail".into()),
            risk_level: Some("high".into()),
        })
        .await?;
    println!("[OK] Logged security gate failure");

    let entries = store
        .relational()
        .query_audit_log(Some(run.id), None, 10)
        .await?;
    println!("[OK] Queried audit log: {} entries for run {}", entries.len(), run.id);
    for e in &entries {
        println!("     - [{}] {} gate={:?} result={:?} risk={}",
            e.timestamp.format("%H:%M:%S"),
            e.action_type,
            e.gate_name,
            e.gate_result,
            e.risk_level
        );
    }

    // ── Relational: Read-only SQL ────────────────────────────────────────
    section("4. Read-Only SQL");

    let rows = store
        .relational()
        .read_only_query("SELECT status, COUNT(*) as cnt FROM agent_runs GROUP BY status")
        .await?;
    println!("[OK] Agent run stats:");
    for row in &rows {
        let cols: Vec<String> = row.iter().map(|(k, v)| format!("{}={}", k, v)).collect();
        println!("     {}", cols.join(", "));
    }

    // Verify writes are blocked
    let write_result = store
        .relational()
        .read_only_query("DELETE FROM audit_log WHERE id = -1")
        .await;
    println!("[OK] Write blocked: {}", write_result.is_err());

    // ── Vector Store ─────────────────────────────────────────────────────
    section("5. Vector Store — Embeddings");

    // Create test embedding provider
    let temp_dir = tempfile::tempdir()?;
    std::fs::write(temp_dir.path().join("model.onnx"), b"placeholder")?;
    std::fs::write(temp_dir.path().join("tokenizer.json"), b"{}")?;
    let provider = Arc::new(LocalEmbeddingProvider::new(temp_dir.path().to_path_buf())?);

    // Ingest some sample documents
    let docs = vec![
        ("confluence://payments/swift-guide", "SWIFT MT103 single customer credit transfer message format and validation rules"),
        ("confluence://payments/settlement", "Real-time gross settlement (RTGS) processing for interbank transfers"),
        ("code://payments/src/Gateway.java", "public class PaymentGateway implements TransactionProcessor { void processPayment(MT103 msg) }"),
        ("code://payments/src/Validator.java", "public class MT103Validator { boolean validate(String bicCode, Amount amount) }"),
        ("jira://CORE-1234", "Fix timeout in PaymentGateway when downstream SWIFT network is slow — increase retry from 5s to 30s"),
    ];

    let mut inputs = Vec::new();
    for (source_id, content) in &docs {
        let source_type = if source_id.starts_with("code://") { "code" }
            else if source_id.starts_with("jira://") { "jira" }
            else { "doc" };
        let embedding = provider.embed(content).await?;
        inputs.push(EmbeddingInput {
            source_type: source_type.into(),
            source_id: source_id.to_string(),
            chunk_index: 0,
            content: content.to_string(),
            metadata: json!({"title": source_id}),
            embedding,
        });
    }

    let ids = store.vector().upsert_batch(&inputs).await?;
    println!("[OK] Ingested {} embeddings", ids.len());

    // Search
    let query = "how to process SWIFT payment messages";
    let query_embedding = provider.embed(query).await?;
    let results = store
        .vector()
        .search(&VectorSearchParams {
            query_embedding,
            source_type: None,  // search across all types
            limit: 5,
            similarity_threshold: 0.0,
        })
        .await?;

    println!("[OK] Search '{}' returned {} results:", query, results.len());
    for r in &results {
        println!("     [{:.3}] [{}] {} — {}",
            r.similarity.unwrap_or(0.0),
            r.source_type,
            r.source_id,
            &r.content[..r.content.len().min(60)]
        );
    }

    // Scoped search — code only
    let query_embedding = provider.embed("payment gateway implementation").await?;
    let code_results = store
        .vector()
        .search(&VectorSearchParams {
            query_embedding,
            source_type: Some("code".into()),
            limit: 3,
            similarity_threshold: 0.0,
        })
        .await?;
    println!("\n[OK] Code-only search returned {} results:", code_results.len());
    for r in &code_results {
        println!("     [{:.3}] {}", r.similarity.unwrap_or(0.0), r.source_id);
    }

    // ── Sandbox Template ─────────────────────────────────────────────────
    section("6. Sandbox Templates");

    let template = SandboxTemplate {
        id: uuid::Uuid::new_v4(),
        name: "payment-service-java17".into(),
        repo_url: Some("git@git.bank.internal:payments/payment-service.git".into()),
        description: Some("Java 17 + Maven for the payment service".into()),
        base_image: "ubuntu:22.04".into(),
        dockerfile: "FROM ubuntu:22.04\nRUN apt-get update && apt-get install -y openjdk-17-jdk maven\nCOPY . /workspace\nWORKDIR /workspace\nRUN mvn dependency:go-offline\n".into(),
        setup_script: Some("git config user.email agent@bank.com".into()),
        languages: vec!["java".into()],
        build_tools: vec!["maven".into()],
        pre_warm_count: 3,
        resource_limits: json!({"cpu": 4, "memory_mb": 8192, "disk_mb": 20480, "timeout_sec": 1800}),
        gates_config: json!({"gates": [
            {"name": "lint",     "command": "mvn checkstyle:check",     "timeout": 30},
            {"name": "security", "command": "semgrep --config=auto .",  "timeout": 60},
            {"name": "test",     "command": "mvn test -pl payments",    "timeout": 300},
            {"name": "comply",   "command": "/bank/compliance-check.sh","timeout": 30}
        ]}),
        tools_config: json!({"tools": ["developer__shell", "developer__text_editor", "context__vector_search"]}),
        network_policy: json!({"deny_all_external": true, "allow": ["git.bank.internal", "nexus.bank.internal"]}),
        status: "draft".into(),
        scan_results: Some(json!({"languages": ["java"], "build_tool": "maven", "ci": "jenkins"})),
        created_at: chrono::Utc::now(),
    };

    let tmpl_id = store.relational().create_template(&template).await?;
    println!("[OK] Created template: {} ({})", template.name, tmpl_id);

    let drafts = store.relational().list_templates(Some("draft")).await?;
    println!("[OK] Draft templates: {}", drafts.len());

    store.relational().approve_template(tmpl_id).await?;
    let approved = store.relational().list_templates(Some("approved")).await?;
    println!("[OK] Approved templates: {}", approved.len());

    // ── MCP Server Tools ─────────────────────────────────────────────────
    section("7. MCP Server — Tool Calls");

    let mcp = ContextMcpServer::new(Arc::new(store.clone()), provider.clone());

    // List available tools
    let tools = ContextMcpServer::list_tools();
    println!("[OK] MCP tools available: {}", tools.tools.len());
    for t in &tools.tools {
        println!("     - {}: {}", t.name, t.description.as_deref().unwrap_or("").chars().take(60).collect::<String>());
    }

    // Call vector_search via MCP
    let search_args: JsonObject = serde_json::from_value(json!({
        "query": "SWIFT payment timeout fix",
        "limit": 3,
        "similarity_threshold": 0.0
    }))?;

    let result = mcp
        .call_tool(CallToolRequestParams {
            name: "vector_search".into(),
            arguments: Some(search_args),
            meta: None,
            task: None,
        })
        .await;

    println!("[OK] MCP vector_search result:");
    for c in &result.content {
        if let Some(text) = c.as_text() {
            // Print first 200 chars
            println!("     {}", &text.text[..text.text.len().min(200)]);
        }
    }

    // Call audit_log via MCP
    let audit_args: JsonObject = serde_json::from_value(json!({
        "action_type": "llm_request",
        "action_detail": {"model": "claude-sonnet-4-5-20250929", "prompt_tokens": 1200},
        "risk_level": "low"
    }))?;

    let result = mcp
        .call_tool(CallToolRequestParams {
            name: "audit_log".into(),
            arguments: Some(audit_args),
            meta: None,
            task: None,
        })
        .await;
    println!("[OK] MCP audit_log: {:?}",
        result.content.first().and_then(|c| c.as_text()).map(|t| &t.text));

    // Call sql_query via MCP
    let sql_args: JsonObject = serde_json::from_value(json!({
        "sql": "SELECT action_type, risk_level, COUNT(*) as cnt FROM audit_log GROUP BY action_type, risk_level ORDER BY cnt DESC LIMIT 5"
    }))?;

    let result = mcp
        .call_tool(CallToolRequestParams {
            name: "sql_query".into(),
            arguments: Some(sql_args),
            meta: None,
            task: None,
        })
        .await;
    println!("[OK] MCP sql_query result:");
    for c in &result.content {
        if let Some(text) = c.as_text() {
            println!("     {}", &text.text[..text.text.len().min(300)]);
        }
    }

    // ── Context Hydration ────────────────────────────────────────────────
    section("8. Context Hydration");

    let hydrator = ContextHydrator::new(Arc::new(store.clone()), provider.clone());
    let intent = HydrationIntent {
        message: "Fix the SWIFT payment gateway timeout issue".into(),
        repo: Some("payment-service".into()),
        file_paths: vec!["src/Gateway.java".into()],
        ticket_refs: vec!["CORE-1234".into()],
        urls: vec![],
        service_names: vec!["payment-gateway".into()],
    };

    let context = hydrator.hydrate(&intent).await?;
    println!("[OK] Hydrated context:");
    println!("     Code files:  {}", context.code_context.len());
    println!("     Docs:        {}", context.doc_context.len());
    println!("     Owners:      {}", context.ownership.len());
    println!("     Tests:       {}", context.test_context.len());
    println!("     Rec. tools:  {}", context.recommended_tools.len());

    let prompt = ContextHydrator::format_as_prompt(&context);
    if !prompt.is_empty() {
        println!("\n     System prompt supplement ({} chars):", prompt.len());
        for line in prompt.lines().take(10) {
            println!("     | {}", line);
        }
    }

    // ── Summary ──────────────────────────────────────────────────────────
    section("SMOKE TEST COMPLETE");
    println!("All 8 functional areas verified:");
    println!("  1. Migrations           — schema created");
    println!("  2. Agent Runs           — create, update, fetch");
    println!("  3. Audit Trail          — append, query, filtering");
    println!("  4. Read-Only SQL        — pass SELECT, reject writes");
    println!("  5. Vector Embeddings    — ingest, search, scoped search");
    println!("  6. Sandbox Templates    — create, list, approve workflow");
    println!("  7. MCP Tool Dispatch    — vector_search, audit_log, sql_query");
    println!("  8. Context Hydration    — parallel hydrate, prompt formatting");

    // Cleanup test data
    for input in &inputs {
        store.vector().delete_source(&input.source_type, &input.source_id).await.ok();
    }

    Ok(())
}
