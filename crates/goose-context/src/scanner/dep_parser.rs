use serde_json::Value;
use std::path::Path;

/// A parsed dependency.
#[derive(Debug, Clone)]
pub struct ParsedDependency {
    pub name: String,
    pub version: Option<String>,
    pub ecosystem: String,
    pub dep_type: String, // "runtime", "dev", "build", "test"
}

/// Parse dependencies from a manifest file.
pub fn parse_dependencies(path: &Path, content: &str) -> Vec<ParsedDependency> {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

    match name {
        "package.json" => parse_package_json(content),
        "Cargo.toml" => parse_cargo_toml(content),
        "pom.xml" => parse_pom_xml(content),
        "requirements.txt" => parse_requirements_txt(content),
        "go.mod" => parse_go_mod(content),
        "build.gradle" | "build.gradle.kts" => parse_gradle(content),
        _ => vec![],
    }
}

fn parse_package_json(content: &str) -> Vec<ParsedDependency> {
    let mut deps = Vec::new();
    let Ok(json): Result<Value, _> = serde_json::from_str(content) else {
        return deps;
    };

    if let Some(obj) = json.get("dependencies").and_then(|v| v.as_object()) {
        for (name, version) in obj {
            deps.push(ParsedDependency {
                name: name.clone(),
                version: version.as_str().map(String::from),
                ecosystem: "npm".into(),
                dep_type: "runtime".into(),
            });
        }
    }
    if let Some(obj) = json.get("devDependencies").and_then(|v| v.as_object()) {
        for (name, version) in obj {
            deps.push(ParsedDependency {
                name: name.clone(),
                version: version.as_str().map(String::from),
                ecosystem: "npm".into(),
                dep_type: "dev".into(),
            });
        }
    }
    deps
}

fn parse_cargo_toml(content: &str) -> Vec<ParsedDependency> {
    let mut deps = Vec::new();
    // Simple line-based parsing for [dependencies] section
    let mut in_deps = false;
    let mut dep_type = "runtime";

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed == "[dependencies]" {
            in_deps = true;
            dep_type = "runtime";
            continue;
        } else if trimmed == "[dev-dependencies]" {
            in_deps = true;
            dep_type = "dev";
            continue;
        } else if trimmed == "[build-dependencies]" {
            in_deps = true;
            dep_type = "build";
            continue;
        } else if trimmed.starts_with('[') {
            in_deps = false;
            continue;
        }

        if in_deps {
            if let Some((name, rest)) = trimmed.split_once('=') {
                let name = name.trim().to_string();
                if name.is_empty() || name.starts_with('#') {
                    continue;
                }
                let version = rest
                    .trim()
                    .trim_matches('"')
                    .trim_matches('{')
                    .split(',')
                    .next()
                    .map(|s| s.trim().trim_matches('"').to_string());
                deps.push(ParsedDependency {
                    name,
                    version,
                    ecosystem: "cargo".into(),
                    dep_type: dep_type.into(),
                });
            }
        }
    }
    deps
}

fn parse_pom_xml(content: &str) -> Vec<ParsedDependency> {
    let mut deps = Vec::new();
    // Simple regex-based extraction of <dependency> blocks
    let dep_re = regex::Regex::new(
        r"<dependency>\s*<groupId>([^<]+)</groupId>\s*<artifactId>([^<]+)</artifactId>(?:\s*<version>([^<]*)</version>)?(?:\s*<scope>([^<]*)</scope>)?"
    ).unwrap();

    for cap in dep_re.captures_iter(content) {
        let group = cap.get(1).map(|m| m.as_str()).unwrap_or("");
        let artifact = cap.get(2).map(|m| m.as_str()).unwrap_or("");
        let version = cap.get(3).map(|m| m.as_str().to_string());
        let scope = cap.get(4).map(|m| m.as_str()).unwrap_or("compile");

        let dep_type = match scope {
            "test" => "test",
            "provided" => "runtime",
            "runtime" => "runtime",
            _ => "runtime",
        };

        deps.push(ParsedDependency {
            name: format!("{}:{}", group, artifact),
            version,
            ecosystem: "maven".into(),
            dep_type: dep_type.into(),
        });
    }
    deps
}

fn parse_requirements_txt(content: &str) -> Vec<ParsedDependency> {
    content
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#') && !l.starts_with('-'))
        .filter_map(|line| {
            let parts: Vec<&str> = line.splitn(2, |c| c == '=' || c == '>' || c == '<' || c == '~' || c == '!').collect();
            let name = parts.first()?.trim().to_string();
            if name.is_empty() {
                return None;
            }
            let version = if parts.len() > 1 {
                Some(line[name.len()..].trim().to_string())
            } else {
                None
            };
            Some(ParsedDependency {
                name,
                version,
                ecosystem: "pip".into(),
                dep_type: "runtime".into(),
            })
        })
        .collect()
}

fn parse_go_mod(content: &str) -> Vec<ParsedDependency> {
    let mut deps = Vec::new();
    let mut in_require = false;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed == "require (" {
            in_require = true;
            continue;
        } else if trimmed == ")" {
            in_require = false;
            continue;
        }

        if in_require {
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() >= 2 {
                deps.push(ParsedDependency {
                    name: parts[0].to_string(),
                    version: Some(parts[1].to_string()),
                    ecosystem: "go".into(),
                    dep_type: "runtime".into(),
                });
            }
        }
    }
    deps
}

fn parse_gradle(content: &str) -> Vec<ParsedDependency> {
    let mut deps = Vec::new();
    // Match patterns like: implementation 'group:artifact:version'
    let dep_re = regex::Regex::new(
        r#"(implementation|api|compileOnly|runtimeOnly|testImplementation|testRuntimeOnly)\s+['"]([\w.\-]+):([\w.\-]+)(?::([\w.\-]+))?['"]"#
    ).unwrap();

    for cap in dep_re.captures_iter(content) {
        let scope = cap.get(1).map(|m| m.as_str()).unwrap_or("");
        let group = cap.get(2).map(|m| m.as_str()).unwrap_or("");
        let artifact = cap.get(3).map(|m| m.as_str()).unwrap_or("");
        let version = cap.get(4).map(|m| m.as_str().to_string());

        let dep_type = if scope.starts_with("test") { "test" } else { "runtime" };

        deps.push(ParsedDependency {
            name: format!("{}:{}", group, artifact),
            version,
            ecosystem: "gradle".into(),
            dep_type: dep_type.into(),
        });
    }
    deps
}
