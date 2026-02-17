/// Search the Goose context store with a natural language query.
///
/// Usage:
///   # First onboard a repo (see onboard example), then:
///   cargo run -p goose-context --example query -- "payment gateway timeout"
///   cargo run -p goose-context --example query -- "how to process SWIFT messages"
///   cargo run -p goose-context --example query -- --code "database connection pool"
///   cargo run -p goose-context --example query -- --docs "deployment architecture"
///   cargo run -p goose-context --example query -- --sql "SELECT * FROM agent_runs LIMIT 5"
///   cargo run -p goose-context --example query -- --stats
use goose_context::{
    db::pool::{ContextStore, ContextStoreConfig},
    db::vector::VectorSearchParams,
    embedding::{local::LocalEmbeddingProvider, EmbeddingProvider as _},
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        print_help();
        std::process::exit(1);
    }

    // Connect
    let config = ContextStoreConfig::from_env();
    let store = ContextStore::new(config).await?;

    // Parse mode
    let mode = args.get(1).map(|s| s.as_str()).unwrap_or("");

    match mode {
        "--stats" => show_stats(&store).await?,
        "--sql" => {
            let sql = args.get(2).map(|s| s.as_str()).unwrap_or("SELECT 1");
            run_sql(&store, sql).await?;
        }
        "--code" => {
            let query = args[2..].join(" ");
            search(&store, &query, Some("code")).await?;
        }
        "--docs" => {
            let query = args[2..].join(" ");
            search(&store, &query, Some("doc")).await?;
        }
        _ => {
            let query = args[1..].join(" ");
            search(&store, &query, None).await?;
        }
    }

    Ok(())
}

async fn search(store: &ContextStore, query: &str, source_type: Option<&str>) -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    std::fs::write(temp_dir.path().join("model.onnx"), b"placeholder")?;
    std::fs::write(temp_dir.path().join("tokenizer.json"), b"{}")?;
    let provider = LocalEmbeddingProvider::new(temp_dir.path().to_path_buf())?;

    let scope = source_type.unwrap_or("all");
    println!("Searching [{}]: \"{}\"\n", scope, query);

    let embedding = provider.embed(query).await?;
    let results = store
        .vector()
        .search(&VectorSearchParams {
            query_embedding: embedding,
            source_type: source_type.map(String::from),
            limit: 10,
            similarity_threshold: 0.0,
        })
        .await?;

    if results.is_empty() {
        println!("No results found. Have you onboarded a repo?");
        println!("  cargo run -p goose-context --example onboard -- /path/to/repo");
        return Ok(());
    }

    println!("Found {} results:\n", results.len());
    for (i, r) in results.iter().enumerate() {
        let similarity = r.similarity.unwrap_or(0.0);
        let lang = r.metadata.get("language").and_then(|v| v.as_str()).unwrap_or("");
        let file = r.metadata.get("file_path").and_then(|v| v.as_str()).unwrap_or(&r.source_id);

        println!("{}. [{:.3}] [{}] {} ({})", i + 1, similarity, r.source_type, file, lang);
        // Show first 3 lines of content
        let preview: String = r.content.lines().take(3).collect::<Vec<_>>().join("\n    ");
        println!("    {}", preview);
        println!();
    }

    Ok(())
}

async fn show_stats(store: &ContextStore) -> anyhow::Result<()> {
    println!("Context Store Statistics\n");

    let total = store.vector().count(None).await?;
    let code = store.vector().count(Some("code")).await?;
    let doc = store.vector().count(Some("doc")).await?;
    let jira = store.vector().count(Some("jira")).await?;

    println!("Embeddings:");
    println!("  Total:         {}", total);
    println!("  Code chunks:   {}", code);
    println!("  Doc chunks:    {}", doc);
    println!("  Jira tickets:  {}", jira);
    println!();

    let jobs = store.relational().list_ingestion_jobs(None, 10).await?;
    if !jobs.is_empty() {
        println!("Recent ingestion jobs:");
        for job in &jobs {
            println!(
                "  [{}] {} — {} | chunks={} nodes={} edges={}",
                job.status, job.source_type, job.source_uri, job.chunks_created,
                job.graph_nodes, job.graph_edges
            );
        }
        println!();
    }

    let runs = store.relational().list_agent_runs(None, 5).await?;
    if !runs.is_empty() {
        println!("Recent agent runs:");
        for run in &runs {
            println!(
                "  [{}] {} via {} — session={}",
                run.status, run.id, run.trigger_source, run.session_id
            );
        }
    }

    Ok(())
}

async fn run_sql(store: &ContextStore, sql: &str) -> anyhow::Result<()> {
    println!("SQL: {}\n", sql);
    let rows = store.relational().read_only_query(sql).await?;

    if rows.is_empty() {
        println!("(no rows)");
        return Ok(());
    }

    // Print header
    let headers: Vec<String> = rows[0].iter().map(|(k, _)| k.clone()).collect();
    println!("{}", headers.join(" | "));
    println!("{}", "-".repeat(headers.len() * 15));

    for row in &rows {
        let values: Vec<String> = row
            .iter()
            .map(|(_, v)| match v {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Null => "NULL".into(),
                other => other.to_string(),
            })
            .collect();
        println!("{}", values.join(" | "));
    }

    Ok(())
}

fn print_help() {
    eprintln!("Usage: query [OPTIONS] <query>");
    eprintln!();
    eprintln!("Search the Goose context store with natural language.");
    eprintln!();
    eprintln!("Modes:");
    eprintln!("  query <text>         Search all sources (code + docs + tickets)");
    eprintln!("  query --code <text>  Search code files only");
    eprintln!("  query --docs <text>  Search documentation only");
    eprintln!("  query --sql <SQL>    Run read-only SQL query");
    eprintln!("  query --stats        Show context store statistics");
    eprintln!();
    eprintln!("Examples:");
    eprintln!("  query payment gateway timeout");
    eprintln!("  query --code database connection pool");
    eprintln!("  query --docs SWIFT integration guide");
    eprintln!("  query --sql \"SELECT COUNT(*) FROM embeddings GROUP BY source_type\"");
    eprintln!("  query --stats");
}
