use anyhow::{Context, Result};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// MCP transport configuration.
///
/// Each variant carries the fields specific to that transport type.
/// Serialized with `#[serde(tag = "type")]` so TOML uses `type = "stdio"` etc.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "type")]
pub enum McpTransport {
    /// Spawn a child process communicating over stdio (JSON-RPC on stdin/stdout).
    #[serde(rename = "stdio")]
    Stdio {
        command: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        args: Vec<String>,
        #[serde(default, skip_serializing_if = "HashMap::is_empty")]
        env: HashMap<String, String>,
    },
    /// Connect to a remote MCP server via Streamable HTTP (MCP 2025-03-26 spec).
    #[serde(rename = "http")]
    Http {
        url: String,
        #[serde(default, skip_serializing_if = "HashMap::is_empty")]
        headers: HashMap<String, String>,
        /// Allow insecure `http://` connections (default: false, HTTPS enforced).
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        allow_insecure: bool,
    },
    /// Connect to a remote MCP server via legacy SSE transport.
    #[serde(rename = "sse")]
    Sse {
        url: String,
        #[serde(default, skip_serializing_if = "HashMap::is_empty")]
        headers: HashMap<String, String>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        allow_insecure: bool,
    },
}

/// MCP server entry in `.csa/mcp.toml` or `~/.config/cli-sub-agent/config.toml`.
///
/// # TOML formats
///
/// **Tagged (canonical):**
/// ```toml
/// [[servers]]
/// name = "repomix"
/// type = "stdio"
/// command = "npx"
/// args = ["-y", "repomix@latest", "--mcp"]
///
/// [[servers]]
/// name = "deepwiki"
/// type = "http"
/// url = "https://mcp.deepwiki.com/mcp"
/// ```
///
/// **Legacy (backward-compatible, auto-detected as stdio):**
/// ```toml
/// [[servers]]
/// name = "repomix"
/// command = "npx"
/// args = ["-y", "repomix@latest", "--mcp"]
/// ```
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct McpServerConfig {
    pub name: String,
    #[serde(flatten)]
    pub transport: McpTransport,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stateful: bool,
    /// Per-server memory limit override (MB).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_max_mb: Option<u64>,
}

impl McpTransport {
    /// Short human-readable label for the transport type.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Stdio { .. } => "stdio",
            Self::Http { .. } => "http",
            Self::Sse { .. } => "sse",
        }
    }
}

impl McpServerConfig {
    /// Returns true if this server uses stdio transport.
    pub fn is_stdio(&self) -> bool {
        matches!(&self.transport, McpTransport::Stdio { .. })
    }

    /// Returns true if this server uses a remote transport (HTTP or SSE).
    pub fn is_remote(&self) -> bool {
        matches!(
            &self.transport,
            McpTransport::Http { .. } | McpTransport::Sse { .. }
        )
    }
}

/// Custom deserializer for backward-compatible config parsing.
///
/// Handles three cases:
/// 1. Explicit `type` field → deserialize the matching transport variant.
/// 2. No `type` field + has `command` → auto-detect as `Stdio` (legacy format).
/// 3. No `type` field + no `command` → error with helpful message.
impl<'de> Deserialize<'de> for McpServerConfig {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Raw {
            name: String,
            #[serde(rename = "type")]
            transport_type: Option<String>,
            // Stdio fields
            command: Option<String>,
            #[serde(default)]
            args: Vec<String>,
            #[serde(default)]
            env: HashMap<String, String>,
            // Http/Sse fields
            url: Option<String>,
            #[serde(default)]
            headers: HashMap<String, String>,
            #[serde(default)]
            allow_insecure: bool,
            // Common
            #[serde(default)]
            stateful: bool,
            memory_max_mb: Option<u64>,
        }

        let raw = Raw::deserialize(deserializer)?;

        let transport = match raw.transport_type.as_deref() {
            Some("stdio") => {
                let command = raw.command.ok_or_else(|| {
                    serde::de::Error::custom(format!(
                        "server '{}': type = \"stdio\" requires 'command' field",
                        raw.name
                    ))
                })?;
                McpTransport::Stdio {
                    command,
                    args: raw.args,
                    env: raw.env,
                }
            }
            Some("http") => {
                let url = raw.url.ok_or_else(|| {
                    serde::de::Error::custom(format!(
                        "server '{}': type = \"http\" requires 'url' field",
                        raw.name
                    ))
                })?;
                McpTransport::Http {
                    url,
                    headers: raw.headers,
                    allow_insecure: raw.allow_insecure,
                }
            }
            Some("sse") => {
                let url = raw.url.ok_or_else(|| {
                    serde::de::Error::custom(format!(
                        "server '{}': type = \"sse\" requires 'url' field",
                        raw.name
                    ))
                })?;
                McpTransport::Sse {
                    url,
                    headers: raw.headers,
                    allow_insecure: raw.allow_insecure,
                }
            }
            Some(other) => {
                return Err(serde::de::Error::custom(format!(
                    "server '{}': unknown transport type '{}' (expected: stdio, http, sse)",
                    raw.name, other
                )));
            }
            None => {
                // Legacy format: no type field.
                if let Some(command) = raw.command {
                    McpTransport::Stdio {
                        command,
                        args: raw.args,
                        env: raw.env,
                    }
                } else {
                    return Err(serde::de::Error::custom(format!(
                        "server '{}': missing 'type' field; \
                         add type = \"stdio\" (with 'command') or \
                         type = \"http\" (with 'url')",
                        raw.name
                    )));
                }
            }
        };

        Ok(McpServerConfig {
            name: raw.name,
            transport,
            stateful: raw.stateful,
            memory_max_mb: raw.memory_max_mb,
        })
    }
}

/// Project-level MCP registry loaded from `.csa/mcp.toml`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct McpRegistry {
    #[serde(default)]
    pub servers: Vec<McpServerConfig>,
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
    use super::{McpFilter, McpRegistry, McpServerConfig, McpTransport};
    use std::collections::HashMap;
    use tempfile::tempdir;

    #[test]
    fn test_load_missing_returns_none() {
        let dir = tempdir().unwrap();
        let loaded = McpRegistry::load(dir.path()).unwrap();
        assert!(loaded.is_none());
    }

    #[test]
    fn test_load_parses_legacy_stdio_servers() {
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
                    transport: McpTransport::Stdio {
                        command: "npx".to_string(),
                        args: vec!["-y".to_string(), "repomix-mcp".to_string()],
                        env: HashMap::new(),
                    },
                    stateful: false,
                    memory_max_mb: None,
                },
                McpServerConfig {
                    name: "memory".to_string(),
                    transport: McpTransport::Stdio {
                        command: "npx".to_string(),
                        args: vec!["-y".to_string(), "@anthropic/claude-mem-mcp".to_string()],
                        env: [("MEMORY_DIR".to_string(), "~/.claude/memory".to_string())]
                            .into_iter()
                            .collect(),
                    },
                    stateful: false,
                    memory_max_mb: None,
                },
            ],
        };
        assert_eq!(loaded, expected);
    }

    #[test]
    fn test_load_parses_tagged_transport() {
        let dir = tempdir().unwrap();
        let csa_dir = dir.path().join(".csa");
        std::fs::create_dir_all(&csa_dir).unwrap();
        let path = csa_dir.join("mcp.toml");
        std::fs::write(
            &path,
            r#"
[[servers]]
name = "repomix"
type = "stdio"
command = "npx"
args = ["-y", "repomix-mcp"]

[[servers]]
name = "deepwiki"
type = "http"
url = "https://mcp.deepwiki.com/mcp"
"#,
        )
        .unwrap();

        let loaded = McpRegistry::load(dir.path()).unwrap().unwrap();
        assert_eq!(loaded.servers.len(), 2);
        assert!(loaded.servers[0].is_stdio());
        assert!(loaded.servers[1].is_remote());

        match &loaded.servers[1].transport {
            McpTransport::Http {
                url,
                allow_insecure,
                ..
            } => {
                assert_eq!(url, "https://mcp.deepwiki.com/mcp");
                assert!(!allow_insecure);
            }
            other => panic!("expected Http, got {other:?}"),
        }
    }

    #[test]
    fn test_load_parses_sse_transport() {
        let dir = tempdir().unwrap();
        let csa_dir = dir.path().join(".csa");
        std::fs::create_dir_all(&csa_dir).unwrap();
        let path = csa_dir.join("mcp.toml");
        std::fs::write(
            &path,
            r#"
[[servers]]
name = "legacy"
type = "sse"
url = "https://example.com/sse"
headers = { Authorization = "Bearer token123" }
"#,
        )
        .unwrap();

        let loaded = McpRegistry::load(dir.path()).unwrap().unwrap();
        assert_eq!(loaded.servers.len(), 1);
        match &loaded.servers[0].transport {
            McpTransport::Sse { url, headers, .. } => {
                assert_eq!(url, "https://example.com/sse");
                assert_eq!(headers.get("Authorization").unwrap(), "Bearer token123");
            }
            other => panic!("expected Sse, got {other:?}"),
        }
    }

    #[test]
    fn test_load_mixed_legacy_and_tagged() {
        let dir = tempdir().unwrap();
        let csa_dir = dir.path().join(".csa");
        std::fs::create_dir_all(&csa_dir).unwrap();
        let path = csa_dir.join("mcp.toml");
        std::fs::write(
            &path,
            r#"
[[servers]]
name = "local"
command = "npx"
args = ["-y", "repomix-mcp"]
stateful = true

[[servers]]
name = "remote"
type = "http"
url = "https://mcp.deepwiki.com/mcp"
memory_max_mb = 512
"#,
        )
        .unwrap();

        let loaded = McpRegistry::load(dir.path()).unwrap().unwrap();
        assert_eq!(loaded.servers.len(), 2);
        assert!(loaded.servers[0].is_stdio());
        assert!(loaded.servers[0].stateful);
        assert!(loaded.servers[1].is_remote());
        assert_eq!(loaded.servers[1].memory_max_mb, Some(512));
    }

    #[test]
    fn test_load_missing_command_and_type_fails() {
        let dir = tempdir().unwrap();
        let csa_dir = dir.path().join(".csa");
        std::fs::create_dir_all(&csa_dir).unwrap();
        let path = csa_dir.join("mcp.toml");
        std::fs::write(
            &path,
            r#"
[[servers]]
name = "bad"
url = "https://example.com"
"#,
        )
        .unwrap();

        let err = McpRegistry::load(dir.path()).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("missing 'type' field"), "got: {msg}");
    }

    #[test]
    fn test_load_unknown_type_fails() {
        let dir = tempdir().unwrap();
        let csa_dir = dir.path().join(".csa");
        std::fs::create_dir_all(&csa_dir).unwrap();
        let path = csa_dir.join("mcp.toml");
        std::fs::write(
            &path,
            r#"
[[servers]]
name = "bad"
type = "websocket"
url = "wss://example.com"
"#,
        )
        .unwrap();

        let err = McpRegistry::load(dir.path()).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("unknown transport type"), "got: {msg}");
    }

    #[test]
    fn test_http_missing_url_fails() {
        let dir = tempdir().unwrap();
        let csa_dir = dir.path().join(".csa");
        std::fs::create_dir_all(&csa_dir).unwrap();
        let path = csa_dir.join("mcp.toml");
        std::fs::write(
            &path,
            r#"
[[servers]]
name = "bad"
type = "http"
"#,
        )
        .unwrap();

        let err = McpRegistry::load(dir.path()).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("requires 'url' field"), "got: {msg}");
    }

    #[test]
    fn test_http_allow_insecure() {
        let dir = tempdir().unwrap();
        let csa_dir = dir.path().join(".csa");
        std::fs::create_dir_all(&csa_dir).unwrap();
        let path = csa_dir.join("mcp.toml");
        std::fs::write(
            &path,
            r#"
[[servers]]
name = "local-dev"
type = "http"
url = "http://localhost:8080/mcp"
allow_insecure = true
"#,
        )
        .unwrap();

        let loaded = McpRegistry::load(dir.path()).unwrap().unwrap();
        match &loaded.servers[0].transport {
            McpTransport::Http {
                url,
                allow_insecure,
                ..
            } => {
                assert_eq!(url, "http://localhost:8080/mcp");
                assert!(allow_insecure);
            }
            other => panic!("expected Http, got {other:?}"),
        }
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

    fn stdio_server(name: &str) -> McpServerConfig {
        McpServerConfig {
            name: name.to_string(),
            transport: McpTransport::Stdio {
                command: "npx".to_string(),
                args: vec![format!("-y {name}")],
                env: HashMap::new(),
            },
            stateful: false,
            memory_max_mb: None,
        }
    }

    #[test]
    fn test_merge_global_and_project_dedup_by_name() {
        let global = vec![stdio_server("repomix"), stdio_server("deepwiki")];
        let project = McpRegistry {
            servers: vec![stdio_server("repomix"), stdio_server("memory")],
        };

        let merged = McpRegistry::merge(&global, &project);
        let names: Vec<&str> = merged.servers.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names.len(), 3);
        assert!(names.contains(&"deepwiki"));
        assert!(names.contains(&"repomix"));
        assert!(names.contains(&"memory"));
    }

    #[test]
    fn test_merge_empty_global_returns_project() {
        let project = McpRegistry {
            servers: vec![stdio_server("a")],
        };
        let merged = McpRegistry::merge(&[], &project);
        assert_eq!(merged.servers.len(), 1);
        assert_eq!(merged.servers[0].name, "a");
    }

    #[test]
    fn test_merge_empty_project_returns_global() {
        let global = vec![stdio_server("a"), stdio_server("b")];
        let project = McpRegistry::default();
        let merged = McpRegistry::merge(&global, &project);
        assert_eq!(merged.servers.len(), 2);
    }

    #[test]
    fn test_filter_include_only() {
        let servers = vec![stdio_server("a"), stdio_server("b"), stdio_server("c")];
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
        let servers = vec![stdio_server("a"), stdio_server("b"), stdio_server("c")];
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
        let servers = vec![stdio_server("a"), stdio_server("b"), stdio_server("c")];
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
        let servers = vec![stdio_server("a"), stdio_server("b")];
        let filter = McpFilter::default();
        let result = filter.apply(&servers);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_roundtrip_serialize_tagged_stdio() {
        let config = McpServerConfig {
            name: "test".to_string(),
            transport: McpTransport::Stdio {
                command: "npx".to_string(),
                args: vec!["-y".to_string(), "test-mcp".to_string()],
                env: HashMap::new(),
            },
            stateful: false,
            memory_max_mb: None,
        };

        let serialized = toml::to_string(&config).unwrap();
        assert!(serialized.contains("type = \"stdio\""));
        assert!(serialized.contains("command = \"npx\""));
    }

    #[test]
    fn test_roundtrip_serialize_http() {
        let config = McpServerConfig {
            name: "remote".to_string(),
            transport: McpTransport::Http {
                url: "https://mcp.example.com".to_string(),
                headers: HashMap::new(),
                allow_insecure: false,
            },
            stateful: false,
            memory_max_mb: Some(1024),
        };

        let serialized = toml::to_string(&config).unwrap();
        assert!(serialized.contains("type = \"http\""));
        assert!(serialized.contains("url = \"https://mcp.example.com\""));
        assert!(serialized.contains("memory_max_mb = 1024"));
    }
}
