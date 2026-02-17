# Goose for Banking: Enterprise Agent Platform

## Architecture Overview

```
                         ENTRY POINTS
  ┌──────┬──────┬──────┬────────┬──────────┬──────────────┐
  │Slack │Teams │WebUI │  CLI   │Jira Hook │ CI Webhook   │
  │ Bot  │ Bot  │      │        │          │(flaky tests) │
  └──┬───┴──┬───┴──┬───┴───┬────┴────┬─────┴──────┬───────┘
     │      │      │       │         │            │
     └──────┴──────┴───┬───┴─────────┴────────────┘
                       ▼
  ┌─────────────────────────────────────────────────────────┐
  │              ORCHESTRATION LAYER                         │
  │          crates/goose-orchestrator/                      │
  │                                                         │
  │  ┌─────────────────┐  ┌──────────────────────────────┐  │
  │  │ Request Router   │  │ Context Hydrator             │  │
  │  │                  │  │ (pre-fetch before agent run) │  │
  │  │ • Parse trigger  │  │                              │  │
  │  │ • Extract intent │  │ • Scan prompt for links/refs │  │
  │  │ • Select recipe  │  │ • Pull Jira ticket details   │  │
  │  │ • Choose sandbox │  │ • Search codebase via graph  │  │
  │  │   template       │  │ • Embed & retrieve from      │  │
  │  └────────┬─────────┘  │   vector store               │  │
  │           │            │ • Curate tool subset (15-20)  │  │
  │           │            └──────────────┬───────────────┘  │
  │           │                           │                  │
  │  ┌────────▼───────────────────────────▼───────────────┐  │
  │  │ Sandbox Pool Manager                               │  │
  │  │                                                    │  │
  │  │ • Pre-warm Firecracker microVMs from templates     │  │
  │  │ • Assign sandbox to agent run                      │  │
  │  │ • Network isolation (no internet, no prod access)  │  │
  │  │ • Enforce resource limits (CPU, mem, disk, time)   │  │
  │  │ • Snapshot & destroy on completion                 │  │
  │  └────────┬───────────────────────────────────────────┘  │
  │           │                                              │
  │  ┌────────▼───────────────────────────────────────────┐  │
  │  │ Deterministic Gate Pipeline                        │  │
  │  │                                                    │  │
  │  │ Agent ──► [LLM writes code]                        │  │
  │  │   │                                                │  │
  │  │   ├──► GATE: Lint (auto, <5s)                      │  │
  │  │   │     └─ fail? → feed back to agent              │  │
  │  │   ├──► GATE: Security scan (Semgrep/Snyk)          │  │
  │  │   │     └─ fail? → block, notify human             │  │
  │  │   ├──► GATE: Compliance check (bank rules)         │  │
  │  │   │     └─ fail? → block, log to audit             │  │
  │  │   ├──► GATE: Selective tests (<2 min)              │  │
  │  │   │     └─ fail? → retry once, then escalate       │  │
  │  │   └──► GATE: PR template + human review            │  │
  │  └───────────────────────────────────────────────────┘  │
  └─────────────────────────┬───────────────────────────────┘
                            │
              ┌─────────────┼─────────────────────┐
              ▼             ▼                     ▼
  ┌────────────────┐ ┌──────────────┐ ┌───────────────────────┐
  │ Goose Agent    │ │ Context MCP  │ │ Source Connectors MCP │
  │ (core loop)    │ │ Server       │ │ Server                │
  │                │ │              │ │                       │
  │ agents/agent.rs│ │ Graph queries│ │ • Git scanner         │
  │ (mostly        │ │ Vector search│ │ • Confluence API      │
  │  unchanged)    │ │ SQL queries  │ │ • Jira API            │
  │                │ │ Audit logging│ │ • SharePoint API      │
  └───────┬────────┘ └──────┬───────┘ │ • ServiceNow API     │
          │                 │         │ • PDF/DOCX parser     │
          │                 │         └───────────┬───────────┘
          │                 ▼                     │
          │     ┌────────────────────────┐        │
          │     │ PostgreSQL             │◄───────┘
          │     │                        │
          │     │ ┌────────────────────┐ │
          │     │ │ Apache AGE         │ │  ← Graph: code deps,
          │     │ │ (graph engine)     │ │    service topology,
          │     │ └────────────────────┘ │    team ownership,
          │     │ ┌────────────────────┐ │    compliance lineage
          │     │ │ pgvector           │ │
          │     │ │ (embedding store)  │ │  ← Vector: semantic
          │     │ └────────────────────┘ │    search over docs,
          │     │ ┌────────────────────┐ │    code, wiki pages
          │     │ │ Relational tables  │ │
          │     │ │ (structured data)  │ │  ← SQL: sessions,
          │     │ └────────────────────┘ │    audit trail, tasks,
          │     └────────────────────────┘    compliance records
          │
          ▼
  ┌─────────────────────────────────────────┐
  │ Firecracker microVM Sandbox             │
  │ (E2B-compatible, self-hosted)           │
  │                                         │
  │ ┌─────────────────────────────────────┐ │
  │ │ Pre-built template per project      │ │
  │ │ • Language runtime (Java/Python/Go) │ │
  │ │ • Build tools (Maven/Gradle/npm)    │ │
  │ │ • Repo pre-cloned                   │ │
  │ │ • Dependencies pre-installed        │ │
  │ │ • Linters & security scanners       │ │
  │ │ • Bank-specific CLI tools           │ │
  │ └─────────────────────────────────────┘ │
  │                                         │
  │ Network: BLOCKED (no internet, no prod) │
  │ Filesystem: copy-on-write from template │
  │ Lifetime: destroyed after run           │
  └─────────────────────────────────────────┘
```

---

## Phase 1: Context Management Foundation

**Goal**: Build the three-tier context store on PostgreSQL (AGE + pgvector) as an MCP server
that the agent can query for code relationships, semantic search, and structured data.

### 1.1 New Crate: `crates/goose-context/`

```
crates/goose-context/
├── Cargo.toml
├── src/
│   ├── lib.rs
│   ├── mcp_server.rs          # MCP server exposing context tools
│   ├── db/
│   │   ├── mod.rs
│   │   ├── pool.rs            # Connection pool (deadpool-postgres)
│   │   ├── migrations/        # SQL migrations (refinery or sqlx)
│   │   │   ├── V001__init_schema.sql
│   │   │   ├── V002__age_graph_schema.sql
│   │   │   └── V003__pgvector_tables.sql
│   │   ├── graph.rs           # Apache AGE query builder
│   │   ├── vector.rs          # pgvector operations (embed, search, upsert)
│   │   └── relational.rs      # Structured data queries (sessions, audit, etc.)
│   ├── models/
│   │   ├── mod.rs
│   │   ├── code_entity.rs     # Files, functions, classes, modules
│   │   ├── service.rs         # Microservices, APIs, deployments
│   │   ├── dependency.rs      # Code & service dependencies
│   │   ├── document.rs        # Ingested docs (Confluence, PDF, etc.)
│   │   ├── team.rs            # Team ownership, CODEOWNERS
│   │   └── audit.rs           # Audit trail records
│   ├── embedding/
│   │   ├── mod.rs
│   │   ├── provider.rs        # Trait for embedding providers
│   │   ├── local.rs           # Local model (e5-small, all-MiniLM) for air-gap
│   │   └── api.rs             # API-based embeddings (if network available)
│   └── hydrator.rs            # Context hydration pipeline
```

### 1.2 PostgreSQL Schema Design

#### Graph Layer (Apache AGE)

```sql
-- Code entity graph
SELECT * FROM ag_catalog.create_graph('codebase');

-- Vertex labels
-- :File {path, language, size, last_modified, repo}
-- :Function {name, signature, file_path, line_start, line_end}
-- :Class {name, file_path, language}
-- :Service {name, repo, deploy_target, team}
-- :Team {name, slack_channel, oncall_rotation}
-- :Package {name, version, ecosystem}  -- maven, npm, pip, etc.
-- :Config {path, type}                 -- Dockerfile, CI, k8s manifests

-- Edge labels
-- :IMPORTS {from_line}
-- :CALLS {from_line, to_line}
-- :DEPENDS_ON {version_constraint}
-- :OWNS {}                             -- Team -> Service/File
-- :DEPLOYS_TO {environment}
-- :EXTENDS / :IMPLEMENTS {}
-- :DEFINED_IN {}                       -- Function/Class -> File
-- :TESTED_BY {test_file, test_name}
```

#### Vector Layer (pgvector)

```sql
CREATE EXTENSION IF NOT EXISTS vector;

CREATE TABLE embeddings (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    source_type VARCHAR(50) NOT NULL,  -- 'code', 'doc', 'confluence', 'jira', 'pdf'
    source_id   VARCHAR(500) NOT NULL, -- file path, page URL, ticket ID
    chunk_index INT NOT NULL DEFAULT 0,
    content     TEXT NOT NULL,
    metadata    JSONB NOT NULL DEFAULT '{}',
    embedding   vector(384) NOT NULL,  -- dimension matches model (e5-small=384)
    created_at  TIMESTAMPTZ DEFAULT now(),
    updated_at  TIMESTAMPTZ DEFAULT now()
);

CREATE INDEX idx_embeddings_source ON embeddings(source_type, source_id);
CREATE INDEX idx_embeddings_vector ON embeddings
    USING ivfflat (embedding vector_cosine_ops) WITH (lists = 100);
```

#### Relational Layer

```sql
-- Agent runs (extends Goose sessions with bank-specific fields)
CREATE TABLE agent_runs (
    id              UUID PRIMARY KEY,
    session_id      VARCHAR(100) NOT NULL,  -- maps to Goose session
    trigger_source  VARCHAR(50) NOT NULL,   -- 'slack', 'jira', 'ci', 'cli', 'web'
    trigger_ref     TEXT,                   -- Slack thread URL, Jira ticket, etc.
    sandbox_id      VARCHAR(100),
    template_id     UUID REFERENCES sandbox_templates(id),
    status          VARCHAR(20) NOT NULL DEFAULT 'pending',
    pr_url          TEXT,
    started_at      TIMESTAMPTZ,
    completed_at    TIMESTAMPTZ,
    created_by      VARCHAR(200) NOT NULL,  -- user/service identity
    approved_by     VARCHAR(200),           -- human who approved PR
    metadata        JSONB DEFAULT '{}'
);

-- Full audit trail (compliance requirement)
CREATE TABLE audit_log (
    id              BIGSERIAL PRIMARY KEY,
    run_id          UUID REFERENCES agent_runs(id),
    timestamp       TIMESTAMPTZ DEFAULT now(),
    action_type     VARCHAR(50) NOT NULL,   -- 'tool_call', 'llm_request', 'gate_pass',
                                            -- 'gate_fail', 'file_write', 'command_exec'
    action_detail   JSONB NOT NULL,
    tool_name       VARCHAR(200),
    input_hash      VARCHAR(64),            -- SHA-256 of input (not storing secrets)
    output_summary  TEXT,
    token_count     INT,
    gate_name       VARCHAR(100),           -- which deterministic gate
    gate_result     VARCHAR(20),            -- 'pass', 'fail', 'skip'
    risk_level      VARCHAR(20) DEFAULT 'low'
);

-- Sandbox templates
CREATE TABLE sandbox_templates (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name            VARCHAR(200) NOT NULL,
    repo_url        TEXT,
    description     TEXT,
    base_image      VARCHAR(500) NOT NULL,  -- e.g., 'ubuntu:22.04'
    dockerfile      TEXT NOT NULL,           -- generated Dockerfile
    setup_script    TEXT,                    -- post-boot setup
    languages       TEXT[] DEFAULT '{}',
    build_tools     TEXT[] DEFAULT '{}',
    pre_warm_count  INT DEFAULT 2,
    resource_limits JSONB DEFAULT '{"cpu": 2, "memory_mb": 4096, "disk_mb": 10240}',
    status          VARCHAR(20) DEFAULT 'draft',
    created_at      TIMESTAMPTZ DEFAULT now(),
    updated_at      TIMESTAMPTZ DEFAULT now(),
    scan_results    JSONB                   -- raw scan output for traceability
);

-- Document ingestion tracking
CREATE TABLE ingestion_jobs (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    source_type     VARCHAR(50) NOT NULL,
    source_uri      TEXT NOT NULL,
    status          VARCHAR(20) DEFAULT 'pending',
    chunks_created  INT DEFAULT 0,
    graph_nodes     INT DEFAULT 0,
    error_message   TEXT,
    started_at      TIMESTAMPTZ,
    completed_at    TIMESTAMPTZ
);
```

### 1.3 MCP Tools Exposed by Context Server

| Tool | Description |
|------|-------------|
| `context__graph_query` | Run Cypher query against the codebase graph (AGE) |
| `context__graph_neighbors` | Get related entities for a file/service/function |
| `context__graph_path` | Find dependency path between two entities |
| `context__vector_search` | Semantic search across all ingested documents |
| `context__vector_search_code` | Semantic search scoped to code files only |
| `context__vector_search_docs` | Semantic search scoped to docs (Confluence, PDF) |
| `context__sql_query` | Read-only SQL against relational tables |
| `context__audit_log` | Append to audit trail (called by gates) |
| `context__get_owners` | Who owns this file/service? (graph + CODEOWNERS) |
| `context__get_dependencies` | What does this service/file depend on? |
| `context__get_test_coverage` | Which tests cover this file/function? |

### 1.4 Embedding Strategy (Air-Gap Compatible)

- **Primary**: Local model — `e5-small-v2` (384 dims, 33M params, runs on CPU)
- **Optional**: If network is available, use API-based embeddings (OpenAI, Cohere)
- **Chunking**: 512-token chunks with 64-token overlap
- **Storage**: pgvector with IVFFlat index (100 lists for ~1M vectors)
- Embeddings are generated at ingestion time and cached

---

## Phase 2: Source Connectors & Document Ingestion

**Goal**: Build connectors for all enterprise document sources that populate the
three-tier context store. Expose upload endpoints for manual document ingestion.

### 2.1 New Crate: `crates/goose-connectors/`

```
crates/goose-connectors/
├── Cargo.toml
├── src/
│   ├── lib.rs
│   ├── traits.rs              # SourceConnector trait
│   ├── git/
│   │   ├── mod.rs
│   │   ├── scanner.rs         # Repo structure analysis
│   │   ├── language_detect.rs # Language/framework detection
│   │   ├── dep_parser.rs      # Parse package.json, pom.xml, build.gradle, etc.
│   │   ├── ci_parser.rs       # Parse Jenkinsfile, .github/workflows, GitLab CI
│   │   ├── docker_parser.rs   # Parse Dockerfile, docker-compose.yml
│   │   ├── k8s_parser.rs      # Parse k8s manifests, Helm charts
│   │   ├── codeowners.rs      # Parse CODEOWNERS
│   │   └── tree_sitter.rs     # AST analysis for function/class extraction
│   ├── confluence/
│   │   ├── mod.rs
│   │   ├── client.rs          # Confluence REST API client
│   │   ├── crawler.rs         # Space/page crawler with depth limit
│   │   └── parser.rs          # HTML-to-text extraction
│   ├── jira/
│   │   ├── mod.rs
│   │   ├── client.rs          # Jira REST API client
│   │   └── parser.rs          # Extract structured ticket data
│   ├── sharepoint/
│   │   ├── mod.rs
│   │   ├── client.rs          # Microsoft Graph API client
│   │   └── parser.rs          # Document extraction
│   ├── servicenow/
│   │   ├── mod.rs
│   │   ├── client.rs          # ServiceNow REST API
│   │   └── parser.rs          # CMDB, incident, change record extraction
│   ├── document/
│   │   ├── mod.rs
│   │   ├── pdf.rs             # PDF parsing (pdf-extract or lopdf)
│   │   ├── docx.rs            # DOCX parsing
│   │   ├── xlsx.rs            # Spreadsheet parsing
│   │   └── markdown.rs        # Markdown processing
│   └── ingestion/
│       ├── mod.rs
│       ├── pipeline.rs        # Ingestion orchestration
│       ├── chunker.rs         # Text chunking strategies
│       └── graph_builder.rs   # Build graph nodes/edges from parsed data
```

### 2.2 SourceConnector Trait

```rust
#[async_trait]
pub trait SourceConnector: Send + Sync {
    /// Unique name for this connector
    fn name(&self) -> &str;

    /// Validate connection / credentials
    async fn validate(&self) -> Result<()>;

    /// Discover available resources (repos, spaces, projects)
    async fn discover(&self) -> Result<Vec<DiscoveredResource>>;

    /// Ingest a specific resource into the context store
    async fn ingest(
        &self,
        resource: &DiscoveredResource,
        ctx: &ContextStore,
    ) -> Result<IngestionResult>;

    /// Incremental sync (only changed items since last sync)
    async fn sync_since(
        &self,
        resource: &DiscoveredResource,
        since: DateTime<Utc>,
        ctx: &ContextStore,
    ) -> Result<IngestionResult>;
}
```

### 2.3 Git Scanner Detail

The git scanner is the most critical connector — it produces:

1. **Graph nodes**: Files, functions, classes, imports, dependencies
2. **Graph edges**: IMPORTS, CALLS, DEPENDS_ON, DEFINED_IN
3. **Embeddings**: Code chunks with file path + function context
4. **Template inputs**: Language, build tool, runtime, CI config, Docker config

**Parsing pipeline**:
```
Git clone/fetch
    │
    ├─► tree-sitter AST parse ──► function/class/import extraction ──► Graph
    ├─► Cargo.toml / pom.xml / package.json ──► dependency edges ──► Graph
    ├─► Dockerfile / docker-compose.yml ──► base image, deps ──► Template input
    ├─► Jenkinsfile / .github/workflows ──► CI steps, test commands ──► Template
    ├─► k8s manifests / Helm charts ──► service topology ──► Graph
    ├─► CODEOWNERS ──► ownership edges ──► Graph
    └─► All source files ──► chunk + embed ──► pgvector
```

### 2.4 Upload Endpoints (new routes in goose-server)

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/ingest/git` | POST | `{repo_url, branch, credentials}` — trigger git scan |
| `/ingest/confluence` | POST | `{base_url, space_key, credentials}` — crawl space |
| `/ingest/jira` | POST | `{base_url, project_key, credentials}` — ingest project |
| `/ingest/sharepoint` | POST | `{site_url, library, credentials}` — crawl library |
| `/ingest/servicenow` | POST | `{instance_url, table, credentials}` — ingest records |
| `/ingest/upload` | POST | Multipart file upload (PDF, DOCX, XLSX, MD) |
| `/ingest/status/{job_id}` | GET | Check ingestion job progress |
| `/ingest/jobs` | GET | List all ingestion jobs |
| `/context/search` | POST | Unified search across all tiers |
| `/context/graph` | POST | Run graph query |
| `/templates` | GET | List generated sandbox templates |
| `/templates/{id}` | GET | Get template details |
| `/templates/{id}/approve` | POST | Approve template for use |

---

## Phase 3: E2B Sandbox Integration (Firecracker microVMs)

**Goal**: Build the sandbox execution layer using E2B's self-hosted infrastructure
on bare-metal servers with Firecracker microVMs. Each agent run gets an isolated,
pre-warmed VM with no internet or production access.

### 3.1 New Crate: `crates/goose-sandbox/`

```
crates/goose-sandbox/
├── Cargo.toml
├── src/
│   ├── lib.rs
│   ├── mcp_server.rs          # MCP server for sandbox operations
│   ├── manager.rs             # Sandbox pool manager
│   ├── firecracker/
│   │   ├── mod.rs
│   │   ├── vm.rs              # Firecracker VM lifecycle
│   │   ├── network.rs         # Network isolation (no internet policy)
│   │   ├── rootfs.rs          # Root filesystem from template
│   │   └── snapshot.rs        # VM snapshot for pre-warming
│   ├── e2b/
│   │   ├── mod.rs
│   │   ├── client.rs          # E2B self-hosted API client
│   │   ├── template.rs        # Template management
│   │   └── sandbox.rs         # Sandbox instance operations
│   ├── pool/
│   │   ├── mod.rs
│   │   ├── warm_pool.rs       # Pre-warmed VM pool management
│   │   ├── allocator.rs       # Assign sandbox to agent run
│   │   └── cleaner.rs         # Destroy sandboxes after completion
│   └── policy/
│       ├── mod.rs
│       ├── network.rs         # Network policies (block all external)
│       ├── resource.rs        # CPU/memory/disk/time limits
│       └── filesystem.rs      # Read-only mounts, allowed write paths
```

### 3.2 Sandbox Lifecycle

```
Template (Dockerfile + rootfs snapshot)
    │
    ▼
Pre-warm Pool (N hot VMs per template)
    │
    ▼ agent run starts
Allocate VM from pool
    │
    ├─► Mount repo (copy-on-write from snapshot)
    ├─► Inject agent credentials (short-lived, scoped tokens)
    ├─► Start MCP tool server inside VM
    ├─► Connect Goose agent to in-VM MCP server via unix socket
    │
    │   ┌──────────────────────────────────┐
    │   │  Agent runs inside sandbox       │
    │   │  • Write code                    │
    │   │  • Run linters                   │
    │   │  • Execute tests                 │
    │   │  • All file I/O contained        │
    │   └──────────────────────────────────┘
    │
    ▼ agent run completes
Extract artifacts (branch, diff, test results)
    │
    ▼
Destroy VM (zero residual state)
```

### 3.3 Network Isolation Model

```
┌────────────────────────────────────────┐
│ Firecracker microVM                    │
│                                        │
│  Network: veth pair to host            │
│  iptables rules:                       │
│  • ALLOW: host-local MCP server port   │
│  • ALLOW: internal git mirror          │
│  • ALLOW: internal artifact registry   │
│  • DENY:  ALL other outbound           │
│  • DENY:  ALL production endpoints     │
│  • DENY:  ALL internet                 │
│                                        │
│  DNS: resolved only for allowed hosts  │
└────────────────────────────────────────┘
```

### 3.4 MCP Tools for Sandbox

| Tool | Description |
|------|-------------|
| `sandbox__create` | Create sandbox from template |
| `sandbox__exec` | Execute command in sandbox |
| `sandbox__write_file` | Write file to sandbox filesystem |
| `sandbox__read_file` | Read file from sandbox |
| `sandbox__upload` | Upload file/directory to sandbox |
| `sandbox__download` | Download file/directory from sandbox |
| `sandbox__snapshot` | Take filesystem snapshot |
| `sandbox__destroy` | Tear down sandbox |
| `sandbox__status` | Get sandbox resource usage |
| `sandbox__list_processes` | List running processes |

---

## Phase 4: Template Auto-Generation

**Goal**: Automatically generate E2B sandbox templates by scanning git repos,
deployment configs, and documentation. Output a Dockerfile + setup script that
creates a sandbox pre-loaded with the right tools and dependencies.

### 4.1 Template Generation Pipeline

```
Input Sources
    │
    ├─► Git repo scan (Phase 2 git scanner)
    │   • Languages detected (Java 17, Python 3.11, Node 20)
    │   • Build tools (Maven 3.9, Gradle 8.x, npm)
    │   • Dependencies (pom.xml, requirements.txt, package.json)
    │   • CI config (test commands, lint commands)
    │   • Dockerfiles (existing base images, system deps)
    │   • k8s manifests (resource requirements)
    │
    ├─► Deployment files
    │   • Docker base images → inherit into template
    │   • System packages → include in template
    │   • Environment variables → template defaults
    │
    ├─► Confluence/docs (optional enrichment)
    │   • Setup guides → extract install steps
    │   • Architecture docs → service dependencies
    │
    └─► PDF/uploaded docs
        • Onboarding guides → extract dev setup steps
        • Runbooks → extract tool requirements

    ▼

Template Generator (crates/goose-connectors/src/template/)
    │
    ├─► Select base image (ubuntu:22.04 + language runtime)
    ├─► Generate Dockerfile
    │   • Install system packages
    │   • Install language runtimes
    │   • Install build tools
    │   • Install linters (from CI config)
    │   • Install security scanners
    │   • Pre-install dependencies (mvn dependency:go-offline, npm ci)
    │   • Clone repo & build
    │   • Install bank-specific CLI tools
    │
    ├─► Generate setup script (post-boot)
    │   • Git config
    │   • Credential helper setup
    │   • Environment variable defaults
    │   • Pre-compile / warm caches
    │
    ├─► Generate resource limits
    │   • CPU: inferred from CI config or defaults
    │   • Memory: inferred from JVM heap, test suite size
    │   • Disk: repo size + 2x headroom
    │   • Timeout: 30 min default
    │
    └─► Store template in DB with status='draft'
        (requires human approval before production use)
```

### 4.2 Template Structure

```
templates/
├── {template-id}/
│   ├── Dockerfile           # Generated, human-reviewable
│   ├── setup.sh             # Post-boot initialization
│   ├── policy.json          # Network & resource policies
│   ├── tools.json           # Which MCP tools to enable
│   ├── gates.json           # Deterministic gate configuration
│   │   {
│   │     "gates": [
│   │       {"name": "lint",     "command": "mvn checkstyle:check", "timeout": 30},
│   │       {"name": "security", "command": "semgrep --config=auto", "timeout": 60},
│   │       {"name": "test",     "command": "mvn test -pl {changed}", "timeout": 300},
│   │       {"name": "comply",   "command": "/bank/compliance-check.sh", "timeout": 30}
│   │     ]
│   │   }
│   ├── scan_report.json     # What the scanner found (traceability)
│   └── README.md            # Auto-generated template docs
```

### 4.3 Add to `crates/goose-connectors/`

```
crates/goose-connectors/src/
└── template/
    ├── mod.rs
    ├── generator.rs           # Main template generation logic
    ├── dockerfile_builder.rs  # Programmatic Dockerfile construction
    ├── language_profiles.rs   # Known configs per language (Java, Python, Go, JS, Ruby, C#)
    ├── build_tool_profiles.rs # Known configs per build tool
    └── resource_estimator.rs  # Estimate CPU/mem/disk requirements
```

---

## Phase 5: Orchestration Layer & Deterministic Gates

**Goal**: Wrap the Goose agent loop with an orchestration layer that handles
context hydration, sandbox assignment, and deterministic gates between LLM steps.
This is the "six-layer system" that makes Stripe's approach work.

### 5.1 New Crate: `crates/goose-orchestrator/`

```
crates/goose-orchestrator/
├── Cargo.toml
├── src/
│   ├── lib.rs
│   ├── orchestrator.rs        # Main orchestration loop
│   ├── hydrator.rs            # Context hydration before agent run
│   ├── gates/
│   │   ├── mod.rs
│   │   ├── trait.rs           # Gate trait (check, autofix, report)
│   │   ├── lint_gate.rs       # Run linters, autofix if possible
│   │   ├── security_gate.rs   # Semgrep/Snyk scan
│   │   ├── test_gate.rs       # Selective test execution
│   │   ├── compliance_gate.rs # Bank-specific compliance rules
│   │   └── pr_gate.rs         # PR template, branch creation
│   ├── retry.rs               # Retry strategy (max 2 CI rounds like Stripe)
│   ├── routing/
│   │   ├── mod.rs
│   │   ├── trigger_parser.rs  # Parse incoming trigger (Slack, Jira, etc.)
│   │   ├── recipe_selector.rs # Choose recipe based on trigger type
│   │   └── tool_curator.rs    # Select relevant tool subset (15-20 tools)
│   └── reporting/
│       ├── mod.rs
│       ├── pr_builder.rs      # Build PR with bank template
│       ├── slack_reporter.rs  # Report results back to Slack
│       └── audit_reporter.rs  # Compliance audit report
```

### 5.2 Orchestration Flow

```rust
pub struct OrchestratedRun {
    pub trigger: Trigger,           // What initiated this run
    pub context: HydratedContext,   // Pre-fetched context
    pub sandbox: SandboxInstance,   // Assigned sandbox
    pub gates: Vec<Box<dyn Gate>>,  // Deterministic gates
    pub agent_session: String,      // Goose session ID
    pub max_ci_rounds: usize,       // Default: 2 (like Stripe)
}

impl Orchestrator {
    pub async fn run(&self, trigger: Trigger) -> Result<RunResult> {
        // 1. Parse trigger & determine intent
        let intent = self.routing.parse_trigger(&trigger).await?;

        // 2. Hydrate context (pre-fetch before agent starts)
        let context = self.hydrator.hydrate(&intent).await?;
        //    - Resolve links in the trigger message
        //    - Pull Jira ticket details
        //    - Search codebase graph for related files
        //    - Retrieve relevant docs from vector store
        //    - Select ~15 most relevant MCP tools

        // 3. Select & provision sandbox
        let template = self.select_template(&intent).await?;
        let sandbox = self.sandbox_pool.allocate(template).await?;

        // 4. Configure deterministic gates from template
        let gates = self.load_gates(&template).await?;

        // 5. Create Goose session with hydrated context
        let session = self.create_agent_session(
            &context,
            &sandbox,
            &gates,
        ).await?;

        // 6. Run agent with gate pipeline
        let mut ci_round = 0;
        loop {
            // Agent writes code
            let agent_result = self.run_agent_turn(&session).await?;

            // Run deterministic gates sequentially
            let mut all_passed = true;
            for gate in &gates {
                let gate_result = gate.check(&sandbox).await?;
                self.audit_log(&gate_result).await?;

                if !gate_result.passed {
                    if let Some(autofix) = gate_result.autofix {
                        // Apply autofix and re-run gate
                        sandbox.exec(&autofix).await?;
                        continue;
                    }
                    // Feed error back to agent
                    self.feed_gate_error(&session, &gate_result).await?;
                    all_passed = false;
                    break;
                }
            }

            if all_passed {
                break; // All gates passed, proceed to PR
            }

            ci_round += 1;
            if ci_round >= self.max_ci_rounds {
                // Escalate to human with partial results
                break;
            }
        }

        // 7. Create PR and report
        let pr = self.create_pr(&sandbox, &intent).await?;
        self.report_results(&trigger, &pr).await?;

        // 8. Cleanup
        self.sandbox_pool.destroy(sandbox).await?;

        Ok(RunResult { pr, ci_rounds: ci_round })
    }
}
```

### 5.3 Gate Trait

```rust
#[async_trait]
pub trait Gate: Send + Sync {
    fn name(&self) -> &str;
    fn order(&self) -> u32;  // execution order

    /// Run the gate check inside the sandbox
    async fn check(&self, sandbox: &SandboxInstance) -> Result<GateResult>;
}

pub struct GateResult {
    pub gate_name: String,
    pub passed: bool,
    pub output: String,
    pub autofix: Option<String>,     // command to auto-fix
    pub duration: Duration,
    pub risk_level: RiskLevel,
}
```

### 5.4 Modify Agent Loop (minimal changes to agents/agent.rs)

The core agent loop stays intact. The orchestrator wraps it:

```rust
// In goose-orchestrator, NOT modifying agent.rs directly:

// Before agent.reply():
agent.load_extensions_with_sandbox(&sandbox_extensions).await?;

// The sandbox MCP server is added as an extension, so tools like
// sandbox__exec, sandbox__write_file are available to the agent.
// The agent thinks it's just calling tools — it doesn't know
// it's running in an isolated VM.

// After each agent.reply() turn that produces file changes:
for gate in &gates {
    gate.check(&sandbox).await?;
}
```

---

## Phase 6: Entry Points & Integration

**Goal**: Build the multi-channel entry points (Slack, Teams, Jira webhooks,
CI webhooks, Web UI) that trigger orchestrated agent runs.

### 6.1 New Crate: `crates/goose-triggers/`

```
crates/goose-triggers/
├── Cargo.toml
├── src/
│   ├── lib.rs
│   ├── slack/
│   │   ├── mod.rs
│   │   ├── bot.rs             # Slack bot (bolt-rs or manual Events API)
│   │   ├── thread_reader.rs   # Read full Slack thread for context
│   │   └── reporter.rs        # Post results back to thread
│   ├── teams/
│   │   ├── mod.rs
│   │   └── bot.rs             # MS Teams bot
│   ├── jira/
│   │   ├── mod.rs
│   │   └── webhook.rs         # Jira webhook handler
│   ├── ci/
│   │   ├── mod.rs
│   │   ├── jenkins.rs         # Jenkins webhook
│   │   └── github_actions.rs  # GH Actions webhook (if used internally)
│   ├── web/
│   │   ├── mod.rs
│   │   └── routes.rs          # Web UI trigger endpoints
│   └── trigger.rs             # Unified Trigger enum
```

### 6.2 Trigger Model

```rust
pub enum Trigger {
    Slack {
        channel_id: String,
        thread_ts: String,
        user_id: String,
        message: String,
        thread_context: Vec<SlackMessage>,  // full thread
    },
    Teams {
        conversation_id: String,
        user_id: String,
        message: String,
    },
    Jira {
        ticket_key: String,      // e.g., "CORE-1234"
        event_type: String,      // "issue_updated", "comment_created"
        comment: Option<String>,
    },
    CI {
        repo: String,
        branch: String,
        build_id: String,
        failure_type: String,    // "test_failure", "lint_error", "build_error"
        failure_details: String,
    },
    Web {
        user_id: String,
        message: String,
        attachments: Vec<Attachment>,
    },
    CLI {
        message: String,
        working_dir: PathBuf,
    },
}
```

---

## Phase 7: Banking-Specific Compliance

### 7.1 Compliance Gate Checks

- **Secret detection**: Scan for API keys, credentials, PII in code changes
- **License compliance**: Check new dependencies against approved license list
- **Regulatory markers**: Ensure required comments/annotations for SOX, PCI-DSS
- **Change classification**: Auto-classify change risk (low/medium/high/critical)
- **Four-eyes principle**: High-risk changes require 2 human reviewers
- **Audit completeness**: Every agent action logged with timestamp, input hash, output

### 7.2 Approval Workflow

```
Agent completes PR
    │
    ├─► Low risk: 1 reviewer, auto-assign from CODEOWNERS
    ├─► Medium risk: 1 reviewer + security scan pass
    ├─► High risk: 2 reviewers + security + compliance sign-off
    └─► Critical risk: Block, escalate to engineering lead
```

---

## Implementation Order & Estimates

| Phase | Deliverable | Depends On |
|-------|-------------|------------|
| **1** | Context store (PostgreSQL + AGE + pgvector + MCP server) | — |
| **2** | Source connectors (Git scanner, Confluence, Jira, SharePoint, ServiceNow, PDF upload) | Phase 1 |
| **3** | E2B sandbox (Firecracker VMs, pool manager, network isolation) | — (parallel with 2) |
| **4** | Template auto-generation (scan repos → generate Dockerfile + config) | Phase 2 + 3 |
| **5** | Orchestration layer (hydrator, deterministic gates, retry logic) | Phase 1 + 3 |
| **6** | Entry points (Slack, Teams, Jira, CI webhooks) | Phase 5 |
| **7** | Banking compliance (secret detection, license check, audit, approval workflow) | Phase 5 |

```
         Phase 1 ─────► Phase 2 ─────┐
           │                          ├──► Phase 4 ──► Phase 5 ──► Phase 6 ──► Phase 7
           │         Phase 3 ─────────┘        │
           │           │                       │
           └───────────┴───────────────────────┘
                    (can parallelize)
```

---

## New Crate Dependency Graph

```
goose-server
    ├── goose (core)
    ├── goose-mcp
    ├── goose-orchestrator (NEW)
    │   ├── goose (core)
    │   ├── goose-context (NEW)
    │   ├── goose-sandbox (NEW)
    │   └── goose-connectors (NEW)
    ├── goose-context (NEW)
    │   └── PostgreSQL (AGE + pgvector)
    ├── goose-connectors (NEW)
    │   ├── goose-context
    │   └── tree-sitter, pdf-extract, reqwest
    ├── goose-sandbox (NEW)
    │   └── Firecracker SDK / E2B SDK
    └── goose-triggers (NEW)
        └── goose-orchestrator
```

---

## Key Design Decisions

1. **All-in-one PostgreSQL** — AGE for graphs + pgvector for embeddings + relational
   tables. Minimizes infrastructure in air-gapped bank environments.

2. **Local embeddings** — e5-small runs on CPU, no GPU required, no external API
   calls. Critical for air-gapped deployments.

3. **Firecracker microVMs** — Sub-second boot times, strong isolation via KVM,
   no container escape risks. Each agent run is fully isolated.

4. **Template approval workflow** — Auto-generated templates require human approval
   before production use. Banks need this for change management.

5. **Max 2 CI rounds** — Like Stripe, diminishing returns after 2 retries.
   Escalate to human rather than burn tokens.

6. **Audit everything** — Every LLM call, tool invocation, gate result, and file
   change is logged with timestamps and input hashes. Non-negotiable for banking.

7. **Minimal agent.rs changes** — The orchestrator wraps the agent loop rather than
   modifying it. Keeps the Goose fork maintainable and mergeable with upstream.
