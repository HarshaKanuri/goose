use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use tracing::info;

use crate::db::pool::ContextStore;
use crate::db::vector::EmbeddingInput;
use crate::embedding::EmbeddingProvider;
use super::dep_parser;
use super::language;

/// Result of scanning a git repository.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanResult {
    pub repo_path: String,
    pub repo_name: String,
    pub languages: HashMap<String, usize>,   // language -> file count
    pub build_tools: HashSet<String>,
    pub total_files: usize,
    pub code_files: usize,
    pub config_files: Vec<ConfigFile>,
    pub dependencies: Vec<DepInfo>,
    pub teams: Vec<String>,
    pub graph_nodes_created: usize,
    pub embeddings_created: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigFile {
    pub path: String,
    pub config_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DepInfo {
    pub name: String,
    pub version: Option<String>,
    pub ecosystem: String,
    pub dep_type: String,
}

/// Git repository scanner.
///
/// Walks a repository, detects languages, parses dependencies,
/// and populates the three-tier context store (graph + vector + relational).
pub struct GitScanner {
    store: ContextStore,
    embedding_provider: Box<dyn EmbeddingProvider>,
    /// Max file size to index (default 100KB)
    max_file_size: u64,
    /// File patterns to skip
    skip_patterns: Vec<String>,
}

impl GitScanner {
    pub fn new(store: ContextStore, embedding_provider: Box<dyn EmbeddingProvider>) -> Self {
        Self {
            store,
            embedding_provider,
            max_file_size: 100_000,
            skip_patterns: vec![
                "node_modules".into(),
                ".git".into(),
                "target".into(),
                "build".into(),
                "dist".into(),
                ".idea".into(),
                ".vscode".into(),
                "__pycache__".into(),
                ".gradle".into(),
                "vendor".into(),
                ".next".into(),
            ],
        }
    }

    /// Scan a local git repository and populate the context store.
    pub async fn scan(&self, repo_path: &Path) -> Result<ScanResult> {
        let repo_path = repo_path
            .canonicalize()
            .with_context(|| format!("Repo path does not exist: {}", repo_path.display()))?;

        let repo_name = repo_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();

        info!(repo = %repo_name, path = %repo_path.display(), "Starting repository scan");

        let mut result = ScanResult {
            repo_path: repo_path.display().to_string(),
            repo_name: repo_name.clone(),
            languages: HashMap::new(),
            build_tools: HashSet::new(),
            total_files: 0,
            code_files: 0,
            config_files: Vec::new(),
            dependencies: Vec::new(),
            teams: Vec::new(),
            graph_nodes_created: 0,
            embeddings_created: 0,
        };

        // Phase 1: Walk the file tree
        let files = self.walk_tree(&repo_path)?;
        result.total_files = files.len();
        info!(total_files = files.len(), "File tree walked");

        // Phase 2: Classify files and collect metadata
        let mut code_files: Vec<(PathBuf, String)> = Vec::new(); // (path, language)
        let mut dep_files: Vec<PathBuf> = Vec::new();
        let mut config_files: Vec<(PathBuf, String)> = Vec::new();
        let mut codeowners_path: Option<PathBuf> = None;

        for file in &files {
            let relative = file.strip_prefix(&repo_path).unwrap_or(file);

            // Detect language
            if let Some(lang) = language::detect_language(file) {
                *result.languages.entry(lang.to_string()).or_insert(0) += 1;
                code_files.push((relative.to_path_buf(), lang.to_string()));
            }

            // Detect config type
            if let Some(config_type) = language::detect_config_type(relative) {
                config_files.push((relative.to_path_buf(), config_type.to_string()));
                result.config_files.push(ConfigFile {
                    path: relative.display().to_string(),
                    config_type: config_type.to_string(),
                });
            }

            // Detect build tool
            if let Some(tool) = language::detect_build_tool(file) {
                result.build_tools.insert(tool.to_string());
                dep_files.push(file.clone());
            }

            // Detect CODEOWNERS
            let name = file.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name == "CODEOWNERS" || name == ".codeowners" {
                codeowners_path = Some(file.clone());
            }
        }

        result.code_files = code_files.len();
        info!(
            code_files = code_files.len(),
            languages = result.languages.len(),
            build_tools = ?result.build_tools,
            configs = config_files.len(),
            "Files classified"
        );

        // Phase 3: Parse dependencies
        for dep_file in &dep_files {
            if let Ok(content) = std::fs::read_to_string(dep_file) {
                let deps = dep_parser::parse_dependencies(dep_file, &content);
                for dep in deps {
                    result.dependencies.push(DepInfo {
                        name: dep.name.clone(),
                        version: dep.version.clone(),
                        ecosystem: dep.ecosystem.clone(),
                        dep_type: dep.dep_type.clone(),
                    });
                }
            }
        }
        info!(dependencies = result.dependencies.len(), "Dependencies parsed");

        // Phase 4: Parse CODEOWNERS
        if let Some(co_path) = &codeowners_path {
            if let Ok(content) = std::fs::read_to_string(co_path) {
                result.teams = parse_codeowners_teams(&content);
                info!(teams = result.teams.len(), "CODEOWNERS parsed");
            }
        }

        // Phase 5: Create graph nodes
        result.graph_nodes_created = self
            .populate_graph(&repo_name, &code_files, &config_files, &result.dependencies, &result.teams)
            .await
            .unwrap_or_else(|e| {
                tracing::warn!("Graph population failed (AGE may not be available): {}", e);
                0
            });

        // Phase 6: Create embeddings for code files
        result.embeddings_created = self
            .populate_embeddings(&repo_path, &repo_name, &code_files)
            .await?;

        // Phase 7: Track ingestion job
        self.store
            .relational()
            .create_ingestion_job("git", &repo_path.display().to_string())
            .await
            .ok();

        info!(
            graph_nodes = result.graph_nodes_created,
            embeddings = result.embeddings_created,
            "Repository scan complete"
        );

        Ok(result)
    }

    /// Walk the file tree, respecting skip patterns.
    fn walk_tree(&self, root: &Path) -> Result<Vec<PathBuf>> {
        let mut files = Vec::new();
        self.walk_recursive(root, root, &mut files)?;
        Ok(files)
    }

    fn walk_recursive(&self, root: &Path, dir: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
        let entries = std::fs::read_dir(dir)
            .with_context(|| format!("Cannot read directory: {}", dir.display()))?;

        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();

            // Skip hidden files and configured patterns
            if name.starts_with('.') && name != ".github" && name != ".codeowners" {
                continue;
            }
            if self.skip_patterns.iter().any(|p| name == *p) {
                continue;
            }

            if path.is_dir() {
                self.walk_recursive(root, &path, files)?;
            } else if path.is_file() {
                // Skip files that are too large
                if let Ok(metadata) = path.metadata() {
                    if metadata.len() <= self.max_file_size {
                        files.push(path);
                    }
                }
            }
        }
        Ok(())
    }

    /// Populate the graph store with nodes and edges.
    async fn populate_graph(
        &self,
        repo_name: &str,
        code_files: &[(PathBuf, String)],
        config_files: &[(PathBuf, String)],
        dependencies: &[DepInfo],
        teams: &[String],
    ) -> Result<usize> {
        let mut count = 0;

        // Create File nodes for code files
        for (path, lang) in code_files {
            let props = json!({
                "path": path.display().to_string(),
                "language": lang,
                "repo": repo_name
            });
            self.store.graph().create_node("File", &props).await.ok();
            count += 1;
        }

        // Create Config nodes
        for (path, config_type) in config_files {
            let props = json!({
                "path": path.display().to_string(),
                "config_type": config_type,
                "repo": repo_name
            });
            self.store.graph().create_node("Config", &props).await.ok();
            count += 1;
        }

        // Create Package nodes for dependencies
        for dep in dependencies {
            let props = json!({
                "name": dep.name,
                "version": dep.version,
                "ecosystem": dep.ecosystem
            });
            self.store.graph().create_node("Package", &props).await.ok();
            count += 1;
        }

        // Create Team nodes
        for team in teams {
            let props = json!({
                "name": team,
                "repo": repo_name
            });
            self.store.graph().create_node("Team", &props).await.ok();
            count += 1;
        }

        Ok(count)
    }

    /// Chunk and embed code files into the vector store.
    async fn populate_embeddings(
        &self,
        repo_path: &Path,
        repo_name: &str,
        code_files: &[(PathBuf, String)],
    ) -> Result<usize> {
        let mut count = 0;
        let mut batch: Vec<EmbeddingInput> = Vec::new();
        let batch_size = 50;

        for (relative_path, lang) in code_files {
            let full_path = repo_path.join(relative_path);
            let content = match std::fs::read_to_string(&full_path) {
                Ok(c) => c,
                Err(_) => continue, // skip binary files
            };

            // Skip very small files
            if content.len() < 50 {
                continue;
            }

            // Chunk the file (512 chars with 64 char overlap for simplicity)
            let chunks = chunk_text(&content, 512, 64);
            let source_id = format!("{}:{}", repo_name, relative_path.display());

            for (i, chunk) in chunks.iter().enumerate() {
                let embedding = self.embedding_provider.embed(chunk).await?;
                batch.push(EmbeddingInput {
                    source_type: "code".into(),
                    source_id: source_id.clone(),
                    chunk_index: i as i32,
                    content: chunk.clone(),
                    metadata: json!({
                        "language": lang,
                        "repo": repo_name,
                        "file_path": relative_path.display().to_string(),
                        "chunk": i,
                        "total_chunks": chunks.len()
                    }),
                    embedding,
                });

                // Flush batch
                if batch.len() >= batch_size {
                    let n = batch.len();
                    self.store.vector().upsert_batch(&batch).await?;
                    count += n;
                    batch.clear();
                }
            }
        }

        // Flush remaining
        if !batch.is_empty() {
            let n = batch.len();
            self.store.vector().upsert_batch(&batch).await?;
            count += n;
        }

        Ok(count)
    }
}

/// Split text into overlapping chunks.
fn chunk_text(text: &str, chunk_size: usize, overlap: usize) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= chunk_size {
        return vec![text.to_string()];
    }

    let mut chunks = Vec::new();
    let mut start = 0;

    while start < chars.len() {
        let end = (start + chunk_size).min(chars.len());
        let chunk: String = chars[start..end].iter().collect();
        chunks.push(chunk);

        if end >= chars.len() {
            break;
        }
        start += chunk_size - overlap;
    }

    chunks
}

/// Extract team names from a CODEOWNERS file.
fn parse_codeowners_teams(content: &str) -> Vec<String> {
    let mut teams = HashSet::new();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // CODEOWNERS format: <pattern> @owner1 @owner2
        for part in line.split_whitespace().skip(1) {
            if part.starts_with('@') {
                teams.insert(part.trim_start_matches('@').to_string());
            }
        }
    }

    teams.into_iter().collect()
}
