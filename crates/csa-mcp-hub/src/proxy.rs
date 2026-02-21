use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use rmcp::model::{
    CallToolRequestParam, CallToolResult, ListToolsResult, PaginatedRequestParam,
    ServerCapabilities, ServerInfo,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler};
use serde_json::{Value, json};
use tokio::sync::RwLock;
use tokio::time::timeout;

use crate::registry::McpRegistry;

#[derive(Clone)]
pub(crate) struct ProxyRouter {
    registry: Arc<McpRegistry>,
    tool_routes: Arc<RwLock<HashMap<String, String>>>,
    request_timeout: Duration,
}

impl ProxyRouter {
    pub(crate) fn new(registry: Arc<McpRegistry>, request_timeout: Duration) -> Self {
        Self {
            registry,
            tool_routes: Arc::new(RwLock::new(HashMap::new())),
            request_timeout,
        }
    }

    pub(crate) async fn status_payload(&self) -> Value {
        let servers = self.registry.server_names();
        let tools_cached = self.tool_routes.read().await.len();
        json!({
            "running": true,
            "servers": servers,
            "toolsCached": tools_cached,
        })
    }

    async fn list_tools_internal(&self) -> Result<ListToolsResult, McpError> {
        let mut tools = Vec::new();
        let mut routes = HashMap::new();

        for server in self.registry.server_names() {
            match timeout(self.request_timeout, self.registry.list_tools(&server)).await {
                Ok(Ok(server_tools)) => {
                    for tool in server_tools {
                        routes.insert(tool.name.to_string(), server.clone());
                        tools.push(tool);
                    }
                }
                Ok(Err(error)) => {
                    tracing::warn!(server = %server, error = %error, "tools/list forwarding failed");
                }
                Err(_) => tracing::warn!(
                    server = %server,
                    timeout_secs = self.request_timeout.as_secs(),
                    "tools/list forwarding timed out"
                ),
            }
        }

        *self.tool_routes.write().await = routes;
        Ok(ListToolsResult::with_all_items(tools))
    }

    async fn call_tool_internal(
        &self,
        request: CallToolRequestParam,
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

        match timeout(
            self.request_timeout,
            self.registry.call_tool(&server_name, request),
        )
        .await
        {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(error)) => Err(McpError::internal_error(
                format!("forwarding to MCP server '{server_name}' failed: {error}"),
                None,
            )),
            Err(_) => Err(McpError::internal_error(
                format!(
                    "forwarding to MCP server '{server_name}' timed out after {}s",
                    self.request_timeout.as_secs()
                ),
                None,
            )),
        }
    }

    async fn lookup_tool_owner(&self, tool_name: &str) -> Option<String> {
        self.tool_routes.read().await.get(tool_name).cloned()
    }
}

impl ServerHandler for ProxyRouter {
    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParam>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        self.list_tools_internal().await
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParam,
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
    use csa_config::McpServerConfig;
    use rmcp::model::CallToolRequestParam;
    use serde_json::json;

    use crate::proxy::ProxyRouter;
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

    #[tokio::test]
    async fn tools_list_and_call_are_forwarded() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let script = write_script(temp.path())?;

        let registry = Arc::new(McpRegistry::new(vec![McpServerConfig {
            name: "mock".to_string(),
            command: "sh".to_string(),
            args: vec![script.to_string_lossy().into_owned()],
            env: HashMap::new(),
        }]));
        let router = ProxyRouter::new(registry.clone(), Duration::from_secs(5));

        let list_response = router.list_tools_internal().await?;
        assert_eq!(list_response.tools[0].name.as_ref(), "echo_tool");

        let call_response = router
            .call_tool_internal(CallToolRequestParam {
                name: "echo_tool".into(),
                arguments: Some(
                    json!({"value":"ping"})
                        .as_object()
                        .cloned()
                        .unwrap_or_default(),
                ),
            })
            .await?;

        assert_eq!(
            call_response.content[0].as_text().map(|t| t.text.as_str()),
            Some("pong")
        );

        registry.shutdown_all().await?;
        Ok(())
    }
}
