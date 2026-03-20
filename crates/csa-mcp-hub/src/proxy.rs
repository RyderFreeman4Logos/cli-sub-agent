use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use rmcp::model::{
    CallToolRequestParams, CallToolResult, ListToolsResult, PaginatedRequestParams,
    ServerCapabilities, ServerInfo,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::RwLock;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

use crate::registry::{McpRegistry, ToolCallRoute};

/// Cached metadata for a single MCP tool, stored alongside its routing info.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ToolDescriptor {
    pub(crate) server_name: String,
    pub(crate) description: Option<String>,
    pub(crate) input_schema: Value,
}

/// Lightweight summary returned by [`ProxyRouter::tool_search`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ToolSummary {
    pub(crate) name: String,
    pub(crate) description_oneliner: Option<String>,
    pub(crate) server_name: String,
}

#[derive(Clone)]
pub(crate) struct ProxyRouter {
    registry: Arc<McpRegistry>,
    pub(crate) tool_cache: Arc<RwLock<HashMap<String, ToolDescriptor>>>,
    request_timeout: Duration,
}

impl ProxyRouter {
    pub(crate) fn new(registry: Arc<McpRegistry>, request_timeout: Duration) -> Self {
        Self {
            registry,
            tool_cache: Arc::new(RwLock::new(HashMap::new())),
            request_timeout,
        }
    }

    pub(crate) async fn status_payload(&self) -> Value {
        let servers = self.registry.server_names();
        let tools_cached = self.tool_cache.read().await.len();
        json!({
            "running": true,
            "servers": servers,
            "toolsCached": tools_cached,
        })
    }

    async fn list_tools_internal(&self) -> Result<ListToolsResult, McpError> {
        let mut tools = Vec::new();
        let mut cache = HashMap::new();

        for server in self.registry.server_names() {
            let cancellation = CancellationToken::new();
            match timeout(
                self.request_timeout,
                self.registry.list_tools(&server, cancellation.clone()),
            )
            .await
            {
                Ok(Ok(server_tools)) => {
                    for tool in server_tools {
                        let name = tool.name.to_string();
                        if let Some(existing) = cache.get(&name) {
                            let existing: &ToolDescriptor = existing;
                            tracing::warn!(
                                tool = %name,
                                previous_server = %existing.server_name,
                                new_server = %server,
                                "duplicate tool name: later registration overrides previous"
                            );
                        }
                        cache.insert(
                            name,
                            ToolDescriptor {
                                server_name: server.clone(),
                                description: tool.description.as_ref().map(|d| d.to_string()),
                                input_schema: Value::Object(tool.input_schema.as_ref().clone()),
                            },
                        );
                        tools.push(tool);
                    }
                }
                Ok(Err(error)) => {
                    tracing::warn!(server = %server, error = %error, "tools/list forwarding failed");
                }
                Err(_) => {
                    cancellation.cancel();
                    tracing::warn!(
                        server = %server,
                        timeout_secs = self.request_timeout.as_secs(),
                        "tools/list forwarding timed out"
                    );
                }
            }
        }

        *self.tool_cache.write().await = cache;
        Ok(ListToolsResult::with_all_items(tools))
    }

    async fn call_tool_internal(
        &self,
        request: CallToolRequestParams,
    ) -> Result<CallToolResult, McpError> {
        let tool_name = request.name.as_ref();
        let mut server = self.lookup_tool_owner(tool_name).await;

        if server.is_none() {
            self.list_tools_internal().await?;
            server = self.lookup_tool_owner(tool_name).await;
        }

        let Some(server_name) = server else {
            return Err(McpError::invalid_params(
                format!("unknown MCP tool: {tool_name}"),
                None,
            ));
        };

        let route = call_route_from_request(&request);
        let cancellation = CancellationToken::new();
        match timeout(
            self.request_timeout,
            self.registry
                .call_tool(&server_name, request, route, cancellation.clone()),
        )
        .await
        {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(error)) => Err(McpError::internal_error(
                format!("forwarding to MCP server '{server_name}' failed: {error}"),
                None,
            )),
            Err(_) => {
                cancellation.cancel();
                Err(McpError::internal_error(
                    format!(
                        "forwarding to MCP server '{server_name}' timed out after {}s",
                        self.request_timeout.as_secs()
                    ),
                    None,
                ))
            }
        }
    }

    async fn lookup_tool_owner(&self, tool_name: &str) -> Option<String> {
        self.tool_cache
            .read()
            .await
            .get(tool_name)
            .map(|desc| desc.server_name.clone())
    }

    /// Look up the full cached descriptor for a tool by name.
    pub(crate) async fn get_tool_descriptor(&self, tool_name: &str) -> Option<ToolDescriptor> {
        self.tool_cache.read().await.get(tool_name).cloned()
    }

    /// Case-insensitive substring search over cached tools.
    ///
    /// `query` is truncated to 256 chars, `limit` is capped at 50.
    /// Only searches already-cached data — never triggers server connections.
    pub(crate) async fn tool_search(&self, query: &str, limit: usize) -> Vec<ToolSummary> {
        const MAX_QUERY_LEN: usize = 256;
        const MAX_LIMIT: usize = 50;

        let limit = limit.min(MAX_LIMIT);
        let query_truncated: String = query.chars().take(MAX_QUERY_LEN).collect();
        let query_lower = query_truncated.to_lowercase();

        let cache = self.tool_cache.read().await;
        let mut results = Vec::new();

        for (name, descriptor) in cache.iter() {
            if results.len() >= limit {
                break;
            }
            let name_lower = name.to_lowercase();
            let desc_lower = descriptor
                .description
                .as_ref()
                .map(|d| d.to_lowercase())
                .unwrap_or_default();

            if name_lower.contains(&query_lower) || desc_lower.contains(&query_lower) {
                results.push(ToolSummary {
                    name: name.clone(),
                    description_oneliner: descriptor.description.clone(),
                    server_name: descriptor.server_name.clone(),
                });
            }
        }

        results
    }
}

fn call_route_from_request(request: &CallToolRequestParams) -> ToolCallRoute {
    let Some(arguments) = request.arguments.as_ref() else {
        return ToolCallRoute::default();
    };

    ToolCallRoute {
        project_root: get_string_argument(arguments, &["project_root", "projectRoot"])
            .map(PathBuf::from),
        toolchain_hash: get_u64_argument(arguments, &["toolchain_hash", "toolchainHash"]),
    }
}

fn get_string_argument(
    arguments: &serde_json::Map<String, Value>,
    keys: &[&str],
) -> Option<String> {
    keys.iter().find_map(|key| {
        arguments
            .get(*key)
            .and_then(Value::as_str)
            .map(str::to_string)
    })
}

fn get_u64_argument(arguments: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<u64> {
    keys.iter().find_map(|key| {
        arguments.get(*key).and_then(|value| match value {
            Value::Number(number) => number.as_u64(),
            Value::String(text) => text.parse::<u64>().ok(),
            _ => None,
        })
    })
}

impl ServerHandler for ProxyRouter {
    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        self.list_tools_internal().await
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        self.call_tool_internal(request).await
    }

    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::default();
        info.server_info.name = "csa-mcp-hub".to_string();
        info.server_info.version = env!("CARGO_PKG_VERSION").to_string();
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::fs;
    use std::sync::Arc;
    use std::time::Duration;

    use anyhow::Result;
    use csa_config::{McpServerConfig, McpTransport};
    use rmcp::model::CallToolRequestParams;
    use serde_json::json;

    use crate::proxy::{ProxyRouter, ToolDescriptor};
    use crate::registry::McpRegistry;

    fn write_script(dir: &std::path::Path) -> Result<std::path::PathBuf> {
        let path = dir.join("mock-mcp.sh");
        fs::write(
            &path,
            r#"#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s\n' "$line" | sed -n 's/.*"id"[ ]*:[ ]*\([^,}]*\).*/\1/p')
  case "$line" in
    *\"initialize\"*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"mock","version":"0.1.0"}}}\n' "$id"
      ;;
    *\"notifications/initialized\"*)
      ;;
    *\"tools/list\"*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"echo_tool","description":"echo","inputSchema":{"type":"object","properties":{}}}]}}\n' "$id"
      ;;
    *\"tools/call\"*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"pong"}]}}\n' "$id"
      ;;
  esac
done
"#,
        )?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&path)?.permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&path, perms)?;
        }

        Ok(path)
    }

    /// Write a mock MCP server that registers two tools, one with a duplicate name.

    #[tokio::test]
    async fn tools_list_and_call_are_forwarded() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let script = write_script(temp.path())?;

        let registry = Arc::new(McpRegistry::new(vec![McpServerConfig {
            name: "mock".to_string(),
            transport: McpTransport::Stdio {
                command: "sh".to_string(),
                args: vec![script.to_string_lossy().into_owned()],
                env: HashMap::new(),
            },
            stateful: false,
            memory_max_mb: None,
        }]));
        let router = ProxyRouter::new(registry.clone(), Duration::from_secs(5));

        let list_response = router.list_tools_internal().await?;
        assert_eq!(list_response.tools[0].name.as_ref(), "echo_tool");

        let call_response = router
            .call_tool_internal(
                CallToolRequestParams::new("echo_tool").with_arguments(
                    json!({"value":"ping"})
                        .as_object()
                        .cloned()
                        .unwrap_or_default(),
                ),
            )
            .await?;

        assert_eq!(
            call_response.content[0].as_text().map(|t| t.text.as_str()),
            Some("pong")
        );

        registry.shutdown_all().await?;
        Ok(())
    }

    #[tokio::test]
    async fn tool_descriptor_cache_populated_after_list() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let script = write_script(temp.path())?;

        let registry = Arc::new(McpRegistry::new(vec![McpServerConfig {
            name: "mock".to_string(),
            transport: McpTransport::Stdio {
                command: "sh".to_string(),
                args: vec![script.to_string_lossy().into_owned()],
                env: HashMap::new(),
            },
            stateful: false,
            memory_max_mb: None,
        }]));
        let router = ProxyRouter::new(registry.clone(), Duration::from_secs(5));

        // Cache should be empty before list
        assert!(router.get_tool_descriptor("echo_tool").await.is_none());

        router.list_tools_internal().await?;

        // Cache should be populated after list
        let descriptor = router
            .get_tool_descriptor("echo_tool")
            .await
            .expect("echo_tool should be cached");
        assert_eq!(descriptor.server_name, "mock");
        assert_eq!(descriptor.description.as_deref(), Some("echo"));
        assert_eq!(
            descriptor.input_schema,
            json!({"type": "object", "properties": {}})
        );

        registry.shutdown_all().await?;
        Ok(())
    }

    #[tokio::test]
    async fn tool_descriptor_duplicate_name_last_wins() {
        // Test cache overwrite behavior directly — avoids flaky server_names()
        // iteration order from McpRegistry (HashMap-backed, non-deterministic).
        let registry = Arc::new(McpRegistry::new(Vec::new()));
        let router = ProxyRouter::new(registry, Duration::from_secs(5));

        {
            let mut cache = router.tool_cache.write().await;
            cache.insert(
                "echo_tool".to_string(),
                ToolDescriptor {
                    server_name: "first-server".to_string(),
                    description: Some("original echo".to_string()),
                    input_schema: json!({"type": "object"}),
                },
            );
            // Overwrite with second server — last insert wins
            cache.insert(
                "echo_tool".to_string(),
                ToolDescriptor {
                    server_name: "second-server".to_string(),
                    description: Some("duplicate echo".to_string()),
                    input_schema: json!({"type": "object"}),
                },
            );
        }

        let descriptor = router
            .get_tool_descriptor("echo_tool")
            .await
            .expect("echo_tool should be cached");
        assert_eq!(descriptor.server_name, "second-server");
        assert_eq!(descriptor.description.as_deref(), Some("duplicate echo"));
    }

    #[tokio::test]
    async fn tool_descriptor_resolve_returns_server_name() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let script = write_script(temp.path())?;

        let registry = Arc::new(McpRegistry::new(vec![McpServerConfig {
            name: "mock".to_string(),
            transport: McpTransport::Stdio {
                command: "sh".to_string(),
                args: vec![script.to_string_lossy().into_owned()],
                env: HashMap::new(),
            },
            stateful: false,
            memory_max_mb: None,
        }]));
        let router = ProxyRouter::new(registry.clone(), Duration::from_secs(5));

        router.list_tools_internal().await?;

        // resolve_tool (via lookup_tool_owner) should still return server_name
        let owner = router.lookup_tool_owner("echo_tool").await;
        assert_eq!(owner.as_deref(), Some("mock"));

        // Unknown tool should return None
        let unknown = router.lookup_tool_owner("nonexistent").await;
        assert!(unknown.is_none());

        registry.shutdown_all().await?;
        Ok(())
    }

    #[tokio::test]
    async fn tool_search_empty_cache_returns_empty() {
        let registry = Arc::new(McpRegistry::new(Vec::new()));
        let router = ProxyRouter::new(registry, Duration::from_secs(5));

        let results = router.tool_search("anything", 10).await;
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn tool_search_matches_name_case_insensitive() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let script = write_script(temp.path())?;

        let registry = Arc::new(McpRegistry::new(vec![McpServerConfig {
            name: "mock".to_string(),
            transport: McpTransport::Stdio {
                command: "sh".to_string(),
                args: vec![script.to_string_lossy().into_owned()],
                env: HashMap::new(),
            },
            stateful: false,
            memory_max_mb: None,
        }]));
        let router = ProxyRouter::new(registry.clone(), Duration::from_secs(5));
        router.list_tools_internal().await?;

        // Case-insensitive match on name
        let results = router.tool_search("ECHO", 10).await;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "echo_tool");
        assert_eq!(results[0].server_name, "mock");

        registry.shutdown_all().await?;
        Ok(())
    }

    #[tokio::test]
    async fn tool_search_no_match_returns_empty() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let script = write_script(temp.path())?;

        let registry = Arc::new(McpRegistry::new(vec![McpServerConfig {
            name: "mock".to_string(),
            transport: McpTransport::Stdio {
                command: "sh".to_string(),
                args: vec![script.to_string_lossy().into_owned()],
                env: HashMap::new(),
            },
            stateful: false,
            memory_max_mb: None,
        }]));
        let router = ProxyRouter::new(registry.clone(), Duration::from_secs(5));
        router.list_tools_internal().await?;

        let results = router.tool_search("nonexistent_xyz", 10).await;
        assert!(results.is_empty());

        registry.shutdown_all().await?;
        Ok(())
    }

    #[tokio::test]
    async fn tool_search_query_truncated_at_256() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let script = write_script(temp.path())?;

        let registry = Arc::new(McpRegistry::new(vec![McpServerConfig {
            name: "mock".to_string(),
            transport: McpTransport::Stdio {
                command: "sh".to_string(),
                args: vec![script.to_string_lossy().into_owned()],
                env: HashMap::new(),
            },
            stateful: false,
            memory_max_mb: None,
        }]));
        let router = ProxyRouter::new(registry.clone(), Duration::from_secs(5));
        router.list_tools_internal().await?;

        // A very long query should not panic and should be silently truncated
        let long_query = "x".repeat(1000);
        let results = router.tool_search(&long_query, 10).await;
        // "x" repeated doesn't match "echo_tool" so empty
        assert!(results.is_empty());

        registry.shutdown_all().await?;
        Ok(())
    }
}
