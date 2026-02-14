use std::{collections::HashMap, fs, path::Path};

use serde::{Deserialize, Serialize};

use crate::error::{AcpError, AcpResult};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct McpServerConfig {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionConfig {
    #[serde(default)]
    pub no_load: Vec<String>,
    #[serde(default)]
    pub extra_load: Vec<String>,
    pub tier: Option<String>,
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default)]
    pub mcp_servers: Vec<McpServerConfig>,
}

impl SessionConfig {
    pub fn from_toml_file(path: impl AsRef<Path>) -> AcpResult<Self> {
        let path_ref = path.as_ref();
        let raw = fs::read_to_string(path_ref).map_err(AcpError::SpawnFailed)?;
        toml::from_str::<Self>(&raw).map_err(|err| {
            AcpError::ConfigError(format!("failed to parse {}: {}", path_ref.display(), err))
        })
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::NamedTempFile;

    use crate::error::AcpError;

    use super::{McpServerConfig, SessionConfig};

    fn write_temp_toml(contents: &str) -> NamedTempFile {
        let file = NamedTempFile::new().expect("temp file");
        fs::write(file.path(), contents).expect("write toml");
        file
    }

    #[test]
    fn test_parse_full_session_config() {
        let file = write_temp_toml(
            r#"
no_load = ["skills/foo", "skills/bar"]
extra_load = ["skills/baz"]
tier = "tier-2-standard"
models = ["codex/openai/o3/medium"]

[[mcp_servers]]
name = "github"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-github"]
[mcp_servers.env]
GITHUB_TOKEN = "token"
"#,
        );

        let cfg = SessionConfig::from_toml_file(file.path()).expect("parse config");
        let expected = SessionConfig {
            no_load: vec!["skills/foo".into(), "skills/bar".into()],
            extra_load: vec!["skills/baz".into()],
            tier: Some("tier-2-standard".into()),
            models: vec!["codex/openai/o3/medium".into()],
            mcp_servers: vec![McpServerConfig {
                name: "github".into(),
                command: "npx".into(),
                args: vec!["-y".into(), "@modelcontextprotocol/server-github".into()],
                env: [("GITHUB_TOKEN".to_string(), "token".to_string())]
                    .into_iter()
                    .collect(),
            }],
        };

        assert_eq!(cfg, expected);
    }

    #[test]
    fn test_parse_minimal_session_config_uses_defaults() {
        let file = write_temp_toml("");
        let cfg = SessionConfig::from_toml_file(file.path()).expect("parse config");

        assert_eq!(cfg, SessionConfig::default());
    }

    #[test]
    fn test_parse_invalid_toml_returns_config_error() {
        let file = write_temp_toml("no_load = [\"ok\"");
        let err = SessionConfig::from_toml_file(file.path()).expect_err("should fail");
        assert!(err.to_string().contains("Configuration error"));
    }

    #[test]
    fn test_parse_missing_file_returns_spawn_failed() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let path = dir.path().join("nonexistent.toml");
        let err = SessionConfig::from_toml_file(path).expect_err("should fail");

        match err {
            AcpError::SpawnFailed(inner) => assert_eq!(inner.kind(), std::io::ErrorKind::NotFound),
            other => panic!("expected SpawnFailed, got {other:?}"),
        }
    }
}
