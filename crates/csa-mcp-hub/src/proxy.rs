use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::{RwLock, watch};

use crate::registry::McpRegistry;

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct JsonRpcRequest {
    #[serde(default = "default_jsonrpc")]
    pub(crate) jsonrpc: String,
    pub(crate) method: String,
    #[serde(default)]
    pub(crate) params: Option<Value>,
    #[serde(default)]
    pub(crate) id: Option<Value>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct JsonRpcResponse {
    pub(crate) jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) error: Option<JsonRpcError>,
    pub(crate) id: Option<Value>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct JsonRpcError {
    pub(crate) code: i64,
    pub(crate) message: String,
}

pub(crate) struct ProxyRouter {
    registry: Arc<McpRegistry>,
    tool_routes: Arc<RwLock<HashMap<String, String>>>,
    next_forward_id: AtomicU64,
}

impl ProxyRouter {
    pub(crate) fn new(registry: Arc<McpRegistry>) -> Self {
        Self {
            registry,
            tool_routes: Arc::new(RwLock::new(HashMap::new())),
            next_forward_id: AtomicU64::new(1),
        }
    }

    pub(crate) async fn handle_request(
        &self,
        client_id: u64,
        request: JsonRpcRequest,
        shutdown_tx: &watch::Sender<bool>,
    ) -> Option<JsonRpcResponse> {
        if request.jsonrpc != "2.0" {
            return Some(error_response(
                request.id,
                -32600,
                "invalid JSON-RPC version".to_string(),
            ));
        }

        match request.method.as_str() {
            "initialize" => Some(result_response(
                request.id,
                json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {"tools": {}},
                    "serverInfo": {
                        "name": "csa-mcp-hub",
                        "version": env!("CARGO_PKG_VERSION")
                    }
                }),
            )),
            "notifications/initialized" => None,
            "hub/status" => {
                let servers = self.registry.server_names();
                let tools_cached = self.tool_routes.read().await.len();
                Some(result_response(
                    request.id,
                    json!({
                        "running": true,
                        "servers": servers,
                        "toolsCached": tools_cached,
                    }),
                ))
            }
            "hub/stop" => {
                let _ = shutdown_tx.send(true);
                Some(result_response(request.id, json!({"stopping": true})))
            }
            "shutdown" => Some(result_response(request.id, json!({}))),
            "tools/list" => Some(self.handle_tools_list(client_id, request.id).await),
            "tools/call" => Some(
                self.handle_tools_call(client_id, request.id, request.params)
                    .await,
            ),
            _ => Some(error_response(
                request.id,
                -32601,
                format!("method not found: {}", request.method),
            )),
        }
    }

    async fn handle_tools_list(
        &self,
        client_id: u64,
        request_id: Option<Value>,
    ) -> JsonRpcResponse {
        let mut tools = Vec::new();
        let mut routes = HashMap::new();

        for server in self.registry.server_names() {
            let forwarded = json!({
                "jsonrpc": "2.0",
                "id": self.forwarded_id(client_id),
                "method": "tools/list"
            });

            let response = match self.registry.request(&server, &forwarded).await {
                Ok(resp) => resp,
                Err(error) => {
                    tracing::warn!(server = %server, error = %error, "tools/list forwarding failed");
                    continue;
                }
            };

            let Some(list) = response
                .get("result")
                .and_then(|v| v.get("tools"))
                .and_then(Value::as_array)
            else {
                continue;
            };

            for tool in list {
                if let Some(name) = tool.get("name").and_then(Value::as_str) {
                    routes.insert(name.to_string(), server.clone());
                }
                tools.push(tool.clone());
            }
        }

        *self.tool_routes.write().await = routes;
        result_response(request_id, json!({"tools": tools}))
    }

    async fn handle_tools_call(
        &self,
        client_id: u64,
        request_id: Option<Value>,
        params: Option<Value>,
    ) -> JsonRpcResponse {
        let Some(params) = params else {
            return error_response(request_id, -32602, "missing tools/call params".to_string());
        };

        let Some(tool_name) = params.get("name").and_then(Value::as_str) else {
            return error_response(
                request_id,
                -32602,
                "tools/call requires params.name".to_string(),
            );
        };

        let mut server = self.lookup_tool_owner(tool_name).await;
        if server.is_none() {
            let refresh = self.handle_tools_list(client_id, None).await;
            if refresh.error.is_some() {
                return error_response(
                    request_id,
                    -32000,
                    "failed to refresh tool routes".to_string(),
                );
            }
            server = self.lookup_tool_owner(tool_name).await;
        }

        let Some(server_name) = server else {
            return error_response(request_id, -32602, format!("unknown MCP tool: {tool_name}"));
        };

        let forwarded = json!({
            "jsonrpc": "2.0",
            "id": self.forwarded_id(client_id),
            "method": "tools/call",
            "params": params,
        });

        match self.registry.request(&server_name, &forwarded).await {
            Ok(response) => {
                if let Some(error) = response.get("error") {
                    let code = error.get("code").and_then(Value::as_i64).unwrap_or(-32000);
                    let message = error
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("upstream MCP error")
                        .to_string();
                    error_response(request_id, code, message)
                } else {
                    result_response(
                        request_id,
                        response.get("result").cloned().unwrap_or_else(|| json!({})),
                    )
                }
            }
            Err(error) => error_response(
                request_id,
                -32000,
                format!("forwarding to MCP server '{server_name}' failed: {error}"),
            ),
        }
    }

    async fn lookup_tool_owner(&self, tool_name: &str) -> Option<String> {
        self.tool_routes.read().await.get(tool_name).cloned()
    }

    fn forwarded_id(&self, client_id: u64) -> Value {
        let seq = self.next_forward_id.fetch_add(1, Ordering::Relaxed);
        Value::String(format!("c{client_id}-{seq}"))
    }
}

fn result_response(id: Option<Value>, result: Value) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: "2.0".to_string(),
        result: Some(result),
        error: None,
        id,
    }
}

pub(crate) fn error_response(id: Option<Value>, code: i64, message: String) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: "2.0".to_string(),
        result: None,
        error: Some(JsonRpcError { code, message }),
        id,
    }
}

fn default_jsonrpc() -> String {
    "2.0".to_string()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::fs;
    use std::sync::Arc;

    use anyhow::Result;
    use csa_config::McpServerConfig;
    use serde_json::json;
    use tokio::sync::watch;

    use crate::proxy::{JsonRpcRequest, ProxyRouter};
    use crate::registry::McpRegistry;

    fn write_script(dir: &std::path::Path) -> Result<std::path::PathBuf> {
        let path = dir.join("mock-mcp.sh");
        fs::write(
            &path,
            r#"#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s\n' "$line" | sed -n 's/.*"id"[ ]*:[ ]*\([^,}]*\).*/\1/p')
  case "$line" in
    *\"tools/list\"*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"echo_tool"}]}}\n' "$id"
      ;;
    *\"tools/call\"*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"pong"}]}}\n' "$id"
      ;;
    *)
      printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id"
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
        let router = ProxyRouter::new(registry.clone());
        let (shutdown_tx, _shutdown_rx) = watch::channel(false);

        let list_response = router
            .handle_request(
                7,
                JsonRpcRequest {
                    jsonrpc: "2.0".to_string(),
                    method: "tools/list".to_string(),
                    params: None,
                    id: Some(json!(1)),
                },
                &shutdown_tx,
            )
            .await
            .expect("tools/list should produce response");

        assert!(list_response.error.is_none());
        assert_eq!(
            list_response
                .result
                .as_ref()
                .and_then(|r| r["tools"][0]["name"].as_str()),
            Some("echo_tool")
        );

        let call_response = router
            .handle_request(
                7,
                JsonRpcRequest {
                    jsonrpc: "2.0".to_string(),
                    method: "tools/call".to_string(),
                    params: Some(json!({"name":"echo_tool","arguments":{}})),
                    id: Some(json!(2)),
                },
                &shutdown_tx,
            )
            .await
            .expect("tools/call should produce response");

        assert!(call_response.error.is_none());
        assert_eq!(
            call_response
                .result
                .as_ref()
                .and_then(|r| r["content"][0]["text"].as_str()),
            Some("pong")
        );

        registry.shutdown_all().await?;
        Ok(())
    }

    #[tokio::test]
    async fn stop_request_flips_shutdown_signal() -> Result<()> {
        let registry = Arc::new(McpRegistry::new(Vec::new()));
        let router = ProxyRouter::new(registry);
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        let response = router
            .handle_request(
                1,
                JsonRpcRequest {
                    jsonrpc: "2.0".to_string(),
                    method: "hub/stop".to_string(),
                    params: None,
                    id: Some(json!(99)),
                },
                &shutdown_tx,
            )
            .await
            .expect("hub/stop should respond");

        assert!(response.error.is_none());
        assert!(*shutdown_rx.borrow());
        Ok(())
    }
}
