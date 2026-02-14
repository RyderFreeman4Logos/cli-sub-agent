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
}

#[cfg(test)]
mod tests {
    use super::{McpRegistry, McpServerConfig};
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
}
