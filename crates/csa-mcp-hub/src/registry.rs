use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use csa_config::McpServerConfig;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
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

    pub(crate) async fn request(&self, server_name: &str, request: &Value) -> Result<Value> {
        let server = self
            .servers
            .get(server_name)
            .with_context(|| format!("unknown MCP server: {server_name}"))?
            .clone();
        let mut guard = server.lock().await;
        guard.request(request).await
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

    async fn request(&mut self, request: &Value) -> Result<Value> {
        let mut last_err: Option<anyhow::Error> = None;

        for _ in 0..2 {
            self.ensure_running().await?;
            if let Some(process) = self.process.as_mut() {
                match process.round_trip(request).await {
                    Ok(response) => {
                        self.restart_backoff = Duration::from_millis(RESTART_BACKOFF_INITIAL_MS);
                        return Ok(response);
                    }
                    Err(error) => {
                        tracing::warn!(
                            server = %self.config.name,
                            error = %error,
                            "MCP server request failed, restarting"
                        );
                        last_err = Some(error);
                        self.restart_after_failure().await?;
                    }
                }
            }
        }

        Err(last_err.unwrap_or_else(|| anyhow!("MCP request failed without explicit error")))
    }

    async fn ensure_running(&mut self) -> Result<()> {
        let is_running = if let Some(process) = self.process.as_mut() {
            process
                .child
                .try_wait()
                .context("failed to query MCP child status")?
                .is_none()
        } else {
            false
        };

        if is_running {
            return Ok(());
        }

        self.process = Some(ServerProcess::spawn(&self.config).await?);
        Ok(())
    }

    async fn restart_after_failure(&mut self) -> Result<()> {
        if let Some(process) = self.process.as_mut() {
            let _ = process.child.start_kill();
            let _ = process.child.wait().await;
        }
        self.process = None;

        tokio::time::sleep(self.restart_backoff).await;
        self.restart_backoff =
            (self.restart_backoff * 2).min(Duration::from_millis(RESTART_BACKOFF_MAX_MS));
        Ok(())
    }

    async fn shutdown(&mut self) -> Result<()> {
        if let Some(process) = self.process.as_mut() {
            let _ = process.child.start_kill();
            let _ = process.child.wait().await;
        }
        self.process = None;
        Ok(())
    }
}

#[derive(Debug)]
struct ServerProcess {
    child: Child,
    stdin: BufWriter<ChildStdin>,
    stdout: BufReader<ChildStdout>,
}

impl ServerProcess {
    async fn spawn(config: &McpServerConfig) -> Result<Self> {
        let mut cmd = Command::new(&config.command);
        cmd.args(&config.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());

        for (key, value) in &config.env {
            cmd.env(key, value);
        }

        let mut child = cmd
            .spawn()
            .with_context(|| format!("failed to spawn MCP server '{}'", config.name))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("failed to capture MCP server stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("failed to capture MCP server stdout"))?;

        Ok(Self {
            child,
            stdin: BufWriter::new(stdin),
            stdout: BufReader::new(stdout),
        })
    }

    async fn round_trip(&mut self, request: &Value) -> Result<Value> {
        let payload = serde_json::to_string(request).context("failed to serialize MCP request")?;
        self.stdin
            .write_all(payload.as_bytes())
            .await
            .context("failed to write MCP request")?;
        self.stdin
            .write_all(b"\n")
            .await
            .context("failed to write MCP request delimiter")?;
        self.stdin
            .flush()
            .await
            .context("failed to flush MCP request")?;

        let mut line = String::new();
        loop {
            line.clear();
            let bytes = self
                .stdout
                .read_line(&mut line)
                .await
                .context("failed to read MCP response")?;
            if bytes == 0 {
                bail!("MCP server closed stdout unexpectedly");
            }
            if line.trim().is_empty() {
                continue;
            }

            let response: Value = serde_json::from_str(line.trim())
                .context("failed to parse MCP response JSON line")?;
            return Ok(response);
        }
    }
}

#[cfg(test)]
mod tests {
    use anyhow::Result;
    use csa_config::McpServerConfig;
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
    async fn registry_forwards_json_rpc_request() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let script_path = write_script(
            temp.path(),
            r#"#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s\n' "$line" | sed -n 's/.*"id"[ ]*:[ ]*\([^,}]*\).*/\1/p')
  case "$line" in
    *\"tools/list\"*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"echo_tool"}]}}\n' "$id"
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

        let response = registry
            .request(
                "mock",
                &json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
            )
            .await?;

        assert_eq!(response["result"]["tools"][0]["name"], "echo_tool");
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
  printf '{{"jsonrpc":"2.0","id":%s,"result":{{"ok":true}}}}\n' "$id"
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

        let response = registry
            .request("flaky", &json!({"jsonrpc":"2.0","id":9,"method":"ping"}))
            .await?;

        assert_eq!(response["result"]["ok"], true);
        registry.shutdown_all().await?;
        Ok(())
    }
}
