/// Onboard a git repository into the Goose context store.
///
/// This scans the repo, builds a code graph, and creates vector embeddings
/// so the Goose agent can understand your codebase before writing code.
///
/// Usage:
///   # 1. Start the database
///   docker-compose -f crates/goose-context/docker-compose.test.yml up -d
///
///   # 2. Onboard a repo (scan + index)
///   cargo run -p goose-context --example onboard -- /path/to/your/repo
///
///   # 3. Search it
///   cargo run -p goose-context --example query -- "payment gateway timeout"
use goose_context::{
    db::pool::{ContextStore, ContextStoreConfig},
    embedding::local::LocalEmbeddingProvider,
    scanner::GitScanner,
};
use std::path::PathBuf;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Parse CLI args
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: onboard <repo-path>");
        eprintln!();
        eprintln!("Scans a git repository and indexes it into the context store.");
        eprintln!("The database must be running (see docker-compose.test.yml).");
        eprintln!();
        eprintln!("Examples:");
        eprintln!("  onboard /home/user/projects/payment-service");
        eprintln!("  onboard .                          # current directory");
        eprintln!("  onboard ~/goose                    # scan Goose itself");
        std::process::exit(1);
    }

    let repo_path = PathBuf::from(&args[1]);
    if !repo_path.exists() {
        eprintln!("Error: Path does not exist: {}", repo_path.display());
        std::process::exit(1);
    }

    println!("Goose Context — Repository Onboarding\n");

    // Connect to database
    let config = ContextStoreConfig::from_env();
    println!("Connecting to PostgreSQL at {}:{}...", config.host, config.port);
    let store = ContextStore::new(config).await?;
    println!("[OK] Connected\n");

    // Run migrations
    println!("Running migrations...");
    store.run_migrations().await?;
    println!("[OK] Schema ready\n");

    // Create embedding provider
    let temp_dir = tempfile::tempdir()?;
    std::fs::write(temp_dir.path().join("model.onnx"), b"placeholder")?;
    std::fs::write(temp_dir.path().join("tokenizer.json"), b"{}")?;
    let provider = LocalEmbeddingProvider::new(temp_dir.path().to_path_buf())?;

    // Scan the repo
    println!("Scanning repository: {}\n", repo_path.display());
    let scanner = GitScanner::new(store, Box::new(provider));
    let result = scanner.scan(&repo_path).await?;

    // Print results
    println!("\n{}", "=".repeat(60));
    println!("  SCAN RESULTS: {}", result.repo_name);
    println!("{}\n", "=".repeat(60));

    println!("Files:");
    println!("  Total files scanned:  {}", result.total_files);
    println!("  Code files indexed:   {}", result.code_files);
    println!();

    println!("Languages:");
    let mut langs: Vec<_> = result.languages.iter().collect();
    langs.sort_by(|a, b| b.1.cmp(a.1));
    for (lang, count) in &langs {
        let bar = "#".repeat((*count).min(&50).to_owned());
        println!("  {:12} {:>4} files  {}", lang, count, bar);
    }
    println!();

    println!("Build tools: {:?}", result.build_tools);
    println!("Dependencies: {} total", result.dependencies.len());
    if !result.dependencies.is_empty() {
        let by_eco: std::collections::HashMap<String, usize> = result
            .dependencies
            .iter()
            .fold(std::collections::HashMap::new(), |mut acc, d| {
                *acc.entry(d.ecosystem.clone()).or_insert(0) += 1;
                acc
            });
        for (eco, count) in &by_eco {
            println!("  {}: {} deps", eco, count);
        }
    }
    println!();

    if !result.config_files.is_empty() {
        println!("Config files:");
        for cf in &result.config_files {
            println!("  [{}] {}", cf.config_type, cf.path);
        }
        println!();
    }

    if !result.teams.is_empty() {
        println!("Teams (from CODEOWNERS): {:?}", result.teams);
        println!();
    }

    println!("Context store populated:");
    println!("  Graph nodes created:  {}", result.graph_nodes_created);
    println!("  Embeddings created:   {}", result.embeddings_created);
    println!();
    println!("Next steps:");
    println!("  1. Search:  cargo run -p goose-context --example query -- \"your query\"");
    println!("  2. Add to Goose as an extension (see PLAN.md Phase 5)");

    Ok(())
}
