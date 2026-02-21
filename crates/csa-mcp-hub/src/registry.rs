use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use csa_config::McpServerConfig;
use rmcp::RoleClient;
use rmcp::model::{CallToolRequestParam, CallToolResult, Tool};
use rmcp::service::{RunningService, ServiceExt};
use rmcp::transport::TokioChildProcess;
use tokio::process::Command;
use tokio::sync::Mutex;

const RESTART_BACKOFF_INITIAL_MS: u64 = 100;
const RESTART_BACKOFF_MAX_MS: u64 = 30_000;

#[derive(Debug)]
pub(crate) struct McpRegistry {
    servers: HashMap<String, Arc<Mutex<ManagedServer>>>,
}

impl McpRegistry {
    pub(crate) fn new(configs: Vec<McpServerConfig>) -> Self {
        let mut servers = HashMap::new();
        for config in configs {
            servers.insert(
                config.name.clone(),
                Arc::new(Mutex::new(ManagedServer::new(config))),
            );
        }
        Self { servers }
    }

    pub(crate) fn server_names(&self) -> Vec<String> {
        self.servers.keys().cloned().collect()
    }

    pub(crate) async fn list_tools(&self, server_name: &str) -> Result<Vec<Tool>> {
        let server = self
            .servers
            .get(server_name)
            .with_context(|| format!("unknown MCP server: {server_name}"))?
            .clone();
        let mut guard = server.lock().await;
        guard.list_tools().await
    }

    pub(crate) async fn call_tool(
        &self,
        server_name: &str,
        request: CallToolRequestParam,
    ) -> Result<CallToolResult> {
        let server = self
            .servers
            .get(server_name)
            .with_context(|| format!("unknown MCP server: {server_name}"))?
            .clone();
        let mut guard = server.lock().await;
        guard.call_tool(request).await
    }

    pub(crate) async fn shutdown_all(&self) -> Result<()> {
        for server in self.servers.values() {
            let mut guard = server.lock().await;
            guard.shutdown().await?;
        }
        Ok(())
    }
}

#[derive(Debug)]
struct ManagedServer {
    config: McpServerConfig,
    process: Option<ServerProcess>,
    restart_backoff: Duration,
}

impl ManagedServer {
    fn new(config: McpServerConfig) -> Self {
        Self {
            config,
            process: None,
            restart_backoff: Duration::from_millis(RESTART_BACKOFF_INITIAL_MS),
        }
    }

    async fn list_tools(&mut self) -> Result<Vec<Tool>> {
        let mut last_err: Option<anyhow::Error> = None;

        for _ in 0..2 {
            if let Err(error) = self.ensure_running().await {
                tracing::warn!(
                    server = %self.config.name,
                    error = %error,
                    "MCP spawn/list_tools failed, restarting"
                );
                last_err = Some(error);
                self.restart_after_failure().await?;
                continue;
            }
            if let Some(process) = self.process.as_ref() {
                match process.service.list_tools(None).await {
                    Ok(response) => {
                        self.restart_backoff = Duration::from_millis(RESTART_BACKOFF_INITIAL_MS);
                        return Ok(response.tools);
                    }
                    Err(error) => {
                        tracing::warn!(
                            server = %self.config.name,
                            error = %error,
                            "MCP list_tools failed, restarting"
                        );
                        last_err = Some(anyhow!(error));
                        self.restart_after_failure().await?;
                    }
                }
            }
        }

        Err(last_err.unwrap_or_else(|| anyhow!("MCP list_tools failed without explicit error")))
    }

    async fn call_tool(&mut self, request: CallToolRequestParam) -> Result<CallToolResult> {
        let mut last_err: Option<anyhow::Error> = None;

        for _ in 0..2 {
            if let Err(error) = self.ensure_running().await {
                tracing::warn!(
                    server = %self.config.name,
                    error = %error,
                    "MCP spawn/call_tool failed, restarting"
                );
                last_err = Some(error);
                self.restart_after_failure().await?;
                continue;
            }
            if let Some(process) = self.process.as_ref() {
                match process.service.call_tool(request.clone()).await {
                    Ok(response) => {
                        self.restart_backoff = Duration::from_millis(RESTART_BACKOFF_INITIAL_MS);
                        return Ok(response);
                    }
                    Err(error) => {
                        tracing::warn!(
                            server = %self.config.name,
                            error = %error,
                            "MCP call_tool failed, restarting"
                        );
                        last_err = Some(anyhow!(error));
                        self.restart_after_failure().await?;
                    }
                }
            }
        }

        Err(last_err.unwrap_or_else(|| anyhow!("MCP call_tool failed without explicit error")))
    }

    async fn ensure_running(&mut self) -> Result<()> {
        if self.process.is_some() {
            return Ok(());
        }

        self.process = Some(ServerProcess::spawn(&self.config).await?);
        Ok(())
    }

    async fn restart_after_failure(&mut self) -> Result<()> {
        if let Some(process) = self.process.take() {
            process.shutdown().await;
        }

        tokio::time::sleep(self.restart_backoff).await;
        self.restart_backoff =
            (self.restart_backoff * 2).min(Duration::from_millis(RESTART_BACKOFF_MAX_MS));
        Ok(())
    }

    async fn shutdown(&mut self) -> Result<()> {
        if let Some(process) = self.process.take() {
            process.shutdown().await;
        }
        Ok(())
    }
}

#[derive(Debug)]
struct ServerProcess {
    service: RunningService<RoleClient, ()>,
}

impl ServerProcess {
    async fn spawn(config: &McpServerConfig) -> Result<Self> {
        let mut cmd = Command::new(&config.command);
        cmd.args(&config.args).stderr(Stdio::null());
        for (key, value) in &config.env {
            cmd.env(key, value);
        }

        let (transport, _stderr) = TokioChildProcess::builder(cmd)
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("failed to spawn MCP server '{}'", config.name))?;
        let service = ()
            .serve(transport)
            .await
            .with_context(|| format!("failed to initialize MCP server '{}'", config.name))?;
        Ok(Self { service })
    }

    async fn shutdown(self) {
        let _ = self.service.cancel().await;
    }
}

#[cfg(test)]
mod tests {
    use anyhow::Result;
    use csa_config::McpServerConfig;
    use rmcp::model::CallToolRequestParam;
    use serde_json::json;
    use std::collections::HashMap;
    use std::fs;

    use super::McpRegistry;

    fn write_script(dir: &std::path::Path, body: &str) -> Result<std::path::PathBuf> {
        let path = dir.join("mock-mcp.sh");
        fs::write(&path, body)?;
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
    async fn registry_forwards_tools_list_and_call_tool() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let script_path = write_script(
            temp.path(),
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
    *)
      printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id"
      ;;
  esac
done
"#,
        )?;

        let registry = McpRegistry::new(vec![McpServerConfig {
            name: "mock".to_string(),
            command: "sh".to_string(),
            args: vec![script_path.to_string_lossy().into_owned()],
            env: HashMap::new(),
        }]);

        let tools = registry.list_tools("mock").await?;
        assert_eq!(tools[0].name.as_ref(), "echo_tool");

        let response = registry
            .call_tool(
                "mock",
                CallToolRequestParam {
                    name: "echo_tool".into(),
                    arguments: Some(
                        json!({
                            "value": "hello"
                        })
                        .as_object()
                        .cloned()
                        .unwrap_or_default(),
                    ),
                },
            )
            .await?;

        assert_eq!(
            response.content[0].as_text().map(|t| t.text.as_str()),
            Some("pong")
        );
        registry.shutdown_all().await?;
        Ok(())
    }

    #[tokio::test]
    async fn registry_restarts_server_after_crash() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let stamp = temp.path().join("first-run.stamp");
        let script_path = write_script(
            temp.path(),
            &format!(
                r#"#!/bin/sh
stamp="{}"
if [ ! -f "$stamp" ]; then
  touch "$stamp"
  exit 1
fi
while IFS= read -r line; do
  id=$(printf '%s\n' "$line" | sed -n 's/.*"id"[ ]*:[ ]*\([^,}}]*\).*/\1/p')
  case "$line" in
    *\"initialize\"*)
      printf '{{"jsonrpc":"2.0","id":%s,"result":{{"protocolVersion":"2024-11-05","capabilities":{{"tools":{{}}}},"serverInfo":{{"name":"mock","version":"0.1.0"}}}}}}\n' "$id"
      ;;
    *\"notifications/initialized\"*)
      ;;
    *\"tools/list\"*)
      printf '{{"jsonrpc":"2.0","id":%s,"result":{{"tools":[{{"name":"echo_tool","description":"echo","inputSchema":{{"type":"object","properties":{{}}}}}}]}}}}\n' "$id"
      ;;
  esac
done
"#,
                stamp.to_string_lossy()
            ),
        )?;

        let registry = McpRegistry::new(vec![McpServerConfig {
            name: "flaky".to_string(),
            command: "sh".to_string(),
            args: vec![script_path.to_string_lossy().into_owned()],
            env: HashMap::new(),
        }]);

        let tools = registry.list_tools("flaky").await?;
        assert_eq!(tools[0].name.as_ref(), "echo_tool");

        registry.shutdown_all().await?;
        Ok(())
    }
}
