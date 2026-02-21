use std::path::PathBuf;

use serde_json::{Map, Value, json};

use crate::session_config::{McpServerConfig, SessionConfig};

const PROXY_SERVER_NAME: &str = "csa-mcp-hub";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpProxyClient {
    socket_path: PathBuf,
}

impl McpProxyClient {
    pub fn from_session_config(config: &SessionConfig) -> Option<Self> {
        config
            .mcp_proxy_socket
            .as_deref()
            .map(PathBuf::from)
            .map(|socket_path| Self { socket_path })
    }

    pub fn socket_exists(&self) -> bool {
        self.socket_path.exists()
    }

    pub fn socket_path(&self) -> &PathBuf {
        &self.socket_path
    }

    pub fn build_proxy_meta_entry(&self) -> Value {
        json!({
            "transport": "unix",
            "socketPath": self.socket_path,
        })
    }
}

/// Resolve MCP server metadata for ACP session/new meta injection.
///
/// Behavior:
/// - If `mcp_proxy_socket` is configured and exists, inject a single proxy entry.
/// - Otherwise, fall back to direct MCP server entries from `mcp_servers`.
pub fn resolve_mcp_meta_servers(config: &SessionConfig) -> Value {
    if let Some(proxy) = McpProxyClient::from_session_config(config)
        && proxy.socket_exists()
    {
        let mut proxy_map = Map::new();
        proxy_map.insert(
            PROXY_SERVER_NAME.to_string(),
            proxy.build_proxy_meta_entry(),
        );
        return Value::Object(proxy_map);
    }

    direct_mcp_meta_servers(&config.mcp_servers)
}

pub fn direct_mcp_meta_servers(servers: &[McpServerConfig]) -> Value {
    let mut map = Map::new();
    for server in servers {
        map.insert(
            server.name.clone(),
            json!({
                "command": server.command,
                "args": server.args,
                "env": server.env,
            }),
        );
    }
    Value::Object(map)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use tempfile::tempdir;

    use crate::mcp_proxy_client::{McpProxyClient, resolve_mcp_meta_servers};
    use crate::session_config::{McpServerConfig, SessionConfig};

    #[test]
    fn resolve_uses_proxy_when_socket_exists() {
        let temp = tempdir().expect("tempdir");
        let socket_path = temp.path().join("mcp-hub.sock");
        std::fs::write(&socket_path, "").expect("create fake socket marker");

        let cfg = SessionConfig {
            mcp_proxy_socket: Some(socket_path.to_string_lossy().into_owned()),
            ..Default::default()
        };

        let result = resolve_mcp_meta_servers(&cfg);
        let keys = result
            .as_object()
            .expect("result should be object")
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(keys, vec!["csa-mcp-hub".to_string()]);
    }

    #[test]
    fn resolve_falls_back_to_direct_servers_when_socket_missing() {
        let cfg = SessionConfig {
            mcp_proxy_socket: Some("/tmp/non-existent-mcp-hub.sock".to_string()),
            mcp_servers: vec![McpServerConfig {
                name: "memory".to_string(),
                command: "npx".to_string(),
                args: vec!["-y".to_string(), "@anthropic/claude-mem-mcp".to_string()],
                env: HashMap::new(),
            }],
            ..Default::default()
        };

        let result = resolve_mcp_meta_servers(&cfg);
        assert!(result.get("memory").is_some());
        assert_eq!(result.get("csa-mcp-hub"), None);
    }

    #[test]
    fn proxy_client_from_session_config_round_trip_path() {
        let cfg = SessionConfig {
            mcp_proxy_socket: Some("/tmp/cli-sub-agent-1000/mcp-hub.sock".to_string()),
            ..Default::default()
        };

        let proxy = McpProxyClient::from_session_config(&cfg).expect("proxy should exist");
        assert_eq!(
            proxy.socket_path().to_string_lossy(),
            "/tmp/cli-sub-agent-1000/mcp-hub.sock"
        );
    }
}
