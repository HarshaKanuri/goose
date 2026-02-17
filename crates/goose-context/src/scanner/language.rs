use std::path::Path;

/// Detect the programming language from a file extension.
pub fn detect_language(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?;
    match ext {
        "java" => Some("java"),
        "kt" | "kts" => Some("kotlin"),
        "py" => Some("python"),
        "rs" => Some("rust"),
        "go" => Some("go"),
        "js" | "mjs" | "cjs" => Some("javascript"),
        "ts" | "mts" | "cts" => Some("typescript"),
        "tsx" => Some("typescript"),
        "jsx" => Some("javascript"),
        "rb" => Some("ruby"),
        "cs" => Some("csharp"),
        "cpp" | "cc" | "cxx" => Some("cpp"),
        "c" | "h" => Some("c"),
        "swift" => Some("swift"),
        "scala" => Some("scala"),
        "php" => Some("php"),
        "sh" | "bash" => Some("shell"),
        "sql" => Some("sql"),
        "yaml" | "yml" => Some("yaml"),
        "json" => Some("json"),
        "xml" => Some("xml"),
        "toml" => Some("toml"),
        "md" | "markdown" => Some("markdown"),
        "tf" | "hcl" => Some("terraform"),
        "proto" => Some("protobuf"),
        "graphql" | "gql" => Some("graphql"),
        _ => None,
    }
}

/// Detect the config type of a file by its name/path.
pub fn detect_config_type(path: &Path) -> Option<&'static str> {
    let name = path.file_name()?.to_str()?;
    let name_lower = name.to_lowercase();

    match name_lower.as_str() {
        "dockerfile" => Some("dockerfile"),
        "docker-compose.yml" | "docker-compose.yaml" | "compose.yml" | "compose.yaml" => {
            Some("docker-compose")
        }
        "jenkinsfile" => Some("ci"),
        ".gitlab-ci.yml" => Some("ci"),
        "makefile" => Some("build"),
        "codeowners" | ".codeowners" => Some("codeowners"),
        _ => {
            // Check path components
            let path_str = path.to_str().unwrap_or("");
            if path_str.contains(".github/workflows/") && (name.ends_with(".yml") || name.ends_with(".yaml")) {
                Some("ci")
            } else if path_str.contains("k8s/") || path_str.contains("kubernetes/") || path_str.contains("deploy/") {
                if name.ends_with(".yml") || name.ends_with(".yaml") {
                    Some("k8s")
                } else {
                    None
                }
            } else if name_lower.starts_with("helm") || path_str.contains("charts/") {
                Some("helm")
            } else if name.ends_with(".tf") {
                Some("terraform")
            } else {
                None
            }
        }
    }
}

/// Detect build tool from a manifest file name.
pub fn detect_build_tool(path: &Path) -> Option<&'static str> {
    let name = path.file_name()?.to_str()?;
    match name {
        "pom.xml" => Some("maven"),
        "build.gradle" | "build.gradle.kts" => Some("gradle"),
        "package.json" => Some("npm"),
        "Cargo.toml" => Some("cargo"),
        "go.mod" => Some("go"),
        "requirements.txt" | "setup.py" | "pyproject.toml" | "Pipfile" => Some("pip"),
        "Gemfile" => Some("bundler"),
        "build.sbt" => Some("sbt"),
        "CMakeLists.txt" => Some("cmake"),
        "Makefile" => Some("make"),
        _ if name.ends_with(".csproj") || name.ends_with(".sln") => Some("dotnet"),
        _ => None,
    }
}
