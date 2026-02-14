use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Project-level MCP registry loaded from `.csa/mcp.toml`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct McpRegistry {
    #[serde(default)]
    pub servers: Vec<McpServerConfig>,
}

/// MCP server entry in `.csa/mcp.toml`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct McpServerConfig {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
}

impl McpRegistry {
    /// Load `.csa/mcp.toml` from the project root.
    ///
    /// Returns `Ok(None)` when the file does not exist.
    pub fn load(project_root: &Path) -> Result<Option<Self>> {
        let path = Self::config_path(project_root);
        if !path.exists() {
            return Ok(None);
        }

        Self::load_from_path(&path).map(Some)
    }

    /// Load MCP registry from an explicit path.
    pub fn load_from_path(path: &Path) -> Result<Self> {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read MCP config: {}", path.display()))?;
        toml::from_str::<Self>(&raw)
            .with_context(|| format!("Failed to parse MCP config: {}", path.display()))
    }

    /// Path to project-level MCP registry.
    pub fn config_path(project_root: &Path) -> PathBuf {
        project_root.join(".csa").join("mcp.toml")
    }

    /// Merge global MCP servers with project-level overrides.
    ///
    /// Project servers with the same name replace global servers.
    /// Servers unique to either source are included as-is.
    pub fn merge(global_servers: &[McpServerConfig], project: &Self) -> Self {
        let mut merged: Vec<McpServerConfig> = Vec::new();
        let project_names: std::collections::HashSet<&str> =
            project.servers.iter().map(|s| s.name.as_str()).collect();

        // Add global servers that aren't overridden by project
        for server in global_servers {
            if !project_names.contains(server.name.as_str()) {
                merged.push(server.clone());
            }
        }

        // Add all project servers (these take precedence)
        merged.extend(project.servers.iter().cloned());

        Self { servers: merged }
    }
}

/// MCP filter for per-skill server selection.
///
/// Used by skill manifests to control which MCP servers are available.
/// Full implementation in Task #265; this defines the interface.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct McpFilter {
    /// Only include these servers (empty = include all).
    #[serde(default)]
    pub include: Vec<String>,
    /// Exclude these servers (applied after include).
    #[serde(default)]
    pub exclude: Vec<String>,
}

impl McpFilter {
    /// Apply filter to a list of MCP server configs.
    ///
    /// - If `include` is non-empty, only servers with matching names are kept.
    /// - Then `exclude` removes any remaining matches.
    pub fn apply(&self, servers: &[McpServerConfig]) -> Vec<McpServerConfig> {
        servers
            .iter()
            .filter(|s| {
                if !self.include.is_empty() && !self.include.contains(&s.name) {
                    return false;
                }
                !self.exclude.contains(&s.name)
            })
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::{McpFilter, McpRegistry, McpServerConfig};
    use std::collections::HashMap;
    use tempfile::tempdir;

    #[test]
    fn test_load_missing_returns_none() {
        let dir = tempdir().unwrap();
        let loaded = McpRegistry::load(dir.path()).unwrap();
        assert!(loaded.is_none());
    }

    #[test]
    fn test_load_parses_servers() {
        let dir = tempdir().unwrap();
        let csa_dir = dir.path().join(".csa");
        std::fs::create_dir_all(&csa_dir).unwrap();
        let path = csa_dir.join("mcp.toml");
        std::fs::write(
            &path,
            r#"
[[servers]]
name = "repomix"
command = "npx"
args = ["-y", "repomix-mcp"]

[[servers]]
name = "memory"
command = "npx"
args = ["-y", "@anthropic/claude-mem-mcp"]
env = { MEMORY_DIR = "~/.claude/memory" }
"#,
        )
        .unwrap();

        let loaded = McpRegistry::load(dir.path()).unwrap().unwrap();
        let expected = McpRegistry {
            servers: vec![
                McpServerConfig {
                    name: "repomix".to_string(),
                    command: "npx".to_string(),
                    args: vec!["-y".to_string(), "repomix-mcp".to_string()],
                    env: HashMap::new(),
                },
                McpServerConfig {
                    name: "memory".to_string(),
                    command: "npx".to_string(),
                    args: vec!["-y".to_string(), "@anthropic/claude-mem-mcp".to_string()],
                    env: [("MEMORY_DIR".to_string(), "~/.claude/memory".to_string())]
                        .into_iter()
                        .collect(),
                },
            ],
        };
        assert_eq!(loaded, expected);
    }

    #[test]
    fn test_load_empty_file_defaults() {
        let dir = tempdir().unwrap();
        let csa_dir = dir.path().join(".csa");
        std::fs::create_dir_all(&csa_dir).unwrap();
        let path = csa_dir.join("mcp.toml");
        std::fs::write(&path, "").unwrap();

        let loaded = McpRegistry::load(dir.path()).unwrap().unwrap();
        assert_eq!(loaded, McpRegistry::default());
    }

    #[test]
    fn test_load_invalid_toml_fails() {
        let dir = tempdir().unwrap();
        let csa_dir = dir.path().join(".csa");
        std::fs::create_dir_all(&csa_dir).unwrap();
        let path = csa_dir.join("mcp.toml");
        std::fs::write(&path, "[[servers]").unwrap();

        let err = McpRegistry::load(dir.path()).unwrap_err();
        assert!(err.to_string().contains("Failed to parse MCP config"));
    }

    fn server(name: &str) -> McpServerConfig {
        McpServerConfig {
            name: name.to_string(),
            command: "npx".to_string(),
            args: vec![format!("-y {name}")],
            env: HashMap::new(),
        }
    }

    #[test]
    fn test_merge_global_and_project_dedup_by_name() {
        let global = vec![server("repomix"), server("deepwiki")];
        let project = McpRegistry {
            servers: vec![server("repomix"), server("memory")],
        };

        let merged = McpRegistry::merge(&global, &project);
        let names: Vec<&str> = merged.servers.iter().map(|s| s.name.as_str()).collect();
        // deepwiki from global, repomix+memory from project (project takes precedence)
        assert_eq!(names.len(), 3);
        assert!(names.contains(&"deepwiki"));
        assert!(names.contains(&"repomix"));
        assert!(names.contains(&"memory"));
    }

    #[test]
    fn test_merge_empty_global_returns_project() {
        let project = McpRegistry {
            servers: vec![server("a")],
        };
        let merged = McpRegistry::merge(&[], &project);
        assert_eq!(merged.servers.len(), 1);
        assert_eq!(merged.servers[0].name, "a");
    }

    #[test]
    fn test_merge_empty_project_returns_global() {
        let global = vec![server("a"), server("b")];
        let project = McpRegistry::default();
        let merged = McpRegistry::merge(&global, &project);
        assert_eq!(merged.servers.len(), 2);
    }

    #[test]
    fn test_filter_include_only() {
        let servers = vec![server("a"), server("b"), server("c")];
        let filter = McpFilter {
            include: vec!["a".to_string(), "c".to_string()],
            exclude: vec![],
        };
        let result = filter.apply(&servers);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].name, "a");
        assert_eq!(result[1].name, "c");
    }

    #[test]
    fn test_filter_exclude_only() {
        let servers = vec![server("a"), server("b"), server("c")];
        let filter = McpFilter {
            include: vec![],
            exclude: vec!["b".to_string()],
        };
        let result = filter.apply(&servers);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].name, "a");
        assert_eq!(result[1].name, "c");
    }

    #[test]
    fn test_filter_include_and_exclude() {
        let servers = vec![server("a"), server("b"), server("c")];
        let filter = McpFilter {
            include: vec!["a".to_string(), "b".to_string()],
            exclude: vec!["b".to_string()],
        };
        let result = filter.apply(&servers);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "a");
    }

    #[test]
    fn test_filter_empty_passes_all() {
        let servers = vec![server("a"), server("b")];
        let filter = McpFilter::default();
        let result = filter.apply(&servers);
        assert_eq!(result.len(), 2);
    }
}
