use std::collections::HashMap;
use std::io;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use csa_config::McpServerConfig;
use csa_process::{SandboxHandle, SpawnOptions, spawn_tool_sandboxed};
use csa_resource::{SandboxCapability, SandboxConfig, apply_rlimits, detect_sandbox_capability};
use rmcp::RoleClient;
use rmcp::model::{CallToolRequestParam, CallToolResult, Tool};
use rmcp::service::{RunningService, ServiceExt};
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

const RESTART_BACKOFF_INITIAL_MS: u64 = 100;
const RESTART_BACKOFF_MAX_MS: u64 = 30_000;
const MCP_SANDBOX_MEMORY_MAX_MB: u64 = 2048;
const MCP_SANDBOX_MEMORY_SWAP_MAX_MB: Option<u64> = Some(0);
const MCP_SANDBOX_PIDS_MAX: Option<u32> = None;
const MCP_SANDBOX_SESSION_ID: &str = "mcp-hub";
const SHUTDOWN_GRACE_SECS: u64 = 3;
const REQUEST_QUEUE_CAPACITY: usize = 64;

pub(crate) struct McpRegistry {
    servers: HashMap<String, Arc<ServerQueueHandle>>,
}

impl McpRegistry {
    pub(crate) fn new(configs: Vec<McpServerConfig>) -> Self {
        let mut servers = HashMap::new();
        for config in configs {
            servers.insert(
                config.name.clone(),
                Arc::new(ServerQueueHandle::spawn(config)),
            );
        }
        Self { servers }
    }

    pub(crate) fn server_names(&self) -> Vec<String> {
        self.servers.keys().cloned().collect()
    }

    pub(crate) async fn list_tools(
        &self,
        server_name: &str,
        cancellation: CancellationToken,
    ) -> Result<Vec<Tool>> {
        let server = self
            .servers
            .get(server_name)
            .with_context(|| format!("unknown MCP server: {server_name}"))?
            .clone();
        server.list_tools(cancellation).await
    }

    pub(crate) async fn call_tool(
        &self,
        server_name: &str,
        request: CallToolRequestParam,
        cancellation: CancellationToken,
    ) -> Result<CallToolResult> {
        let server = self
            .servers
            .get(server_name)
            .with_context(|| format!("unknown MCP server: {server_name}"))?
            .clone();
        server.call_tool(request, cancellation).await
    }

    pub(crate) async fn shutdown_all(&self) -> Result<()> {
        for server in self.servers.values() {
            server.shutdown().await?;
        }
        Ok(())
    }
}

#[derive(Clone)]
struct ServerQueueHandle {
    server_name: String,
    sender: mpsc::Sender<QueueCommand>,
}

enum QueueCommandKind {
    ListTools,
    CallTool(CallToolRequestParam),
    Shutdown,
}

struct QueueCommand {
    kind: QueueCommandKind,
    cancellation: CancellationToken,
    response: oneshot::Sender<Result<QueueResponse>>,
}

enum QueueResponse {
    ListTools(Vec<Tool>),
    CallTool(CallToolResult),
    Shutdown,
}

impl ServerQueueHandle {
    fn spawn(config: McpServerConfig) -> Self {
        let server_name = config.name.clone();
        let (sender, mut receiver) = mpsc::channel::<QueueCommand>(REQUEST_QUEUE_CAPACITY);
        let queue_server_name = server_name.clone();

        tokio::spawn(async move {
            let mut server = ManagedServer::new(config);

            while let Some(command) = receiver.recv().await {
                match command.kind {
                    QueueCommandKind::Shutdown => {
                        let _ = command.response.send(Ok(QueueResponse::Shutdown));
                        break;
                    }
                    QueueCommandKind::ListTools => {
                        let result = Self::run_queue_dispatch(command.cancellation, async {
                            server.list_tools().await.map(QueueResponse::ListTools)
                        })
                        .await;
                        let _ = command.response.send(result);
                    }
                    QueueCommandKind::CallTool(request) => {
                        let result = Self::run_queue_dispatch(command.cancellation, async {
                            server.call_tool(request).await.map(QueueResponse::CallTool)
                        })
                        .await;
                        let _ = command.response.send(result);
                    }
                }
            }

            if let Err(error) = server.shutdown().await {
                tracing::warn!(server = %queue_server_name, error = %error, "failed to shutdown MCP server queue");
            }
        });

        Self {
            server_name,
            sender,
        }
    }

    async fn run_queue_dispatch<F>(
        cancellation: CancellationToken,
        action: F,
    ) -> Result<QueueResponse>
    where
        F: std::future::Future<Output = Result<QueueResponse>>,
    {
        tokio::select! {
            _ = cancellation.cancelled() => Err(anyhow!("MCP request cancelled before dispatch")),
            response = action => response,
        }
    }

    async fn list_tools(&self, cancellation: CancellationToken) -> Result<Vec<Tool>> {
        match self
            .request(QueueCommandKind::ListTools, cancellation)
            .await?
        {
            QueueResponse::ListTools(tools) => Ok(tools),
            QueueResponse::CallTool(_) => Err(anyhow!("unexpected queue response: call_tool")),
            QueueResponse::Shutdown => Err(anyhow!("unexpected queue response: shutdown")),
        }
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParam,
        cancellation: CancellationToken,
    ) -> Result<CallToolResult> {
        match self
            .request(QueueCommandKind::CallTool(request), cancellation)
            .await?
        {
            QueueResponse::CallTool(response) => Ok(response),
            QueueResponse::ListTools(_) => Err(anyhow!("unexpected queue response: list_tools")),
            QueueResponse::Shutdown => Err(anyhow!("unexpected queue response: shutdown")),
        }
    }

    async fn shutdown(&self) -> Result<()> {
        let cancellation = CancellationToken::new();
        let _ = self.request(QueueCommandKind::Shutdown, cancellation).await;
        Ok(())
    }

    async fn request(
        &self,
        kind: QueueCommandKind,
        cancellation: CancellationToken,
    ) -> Result<QueueResponse> {
        if cancellation.is_cancelled() {
            return Err(anyhow!("MCP request cancelled before enqueue"));
        }

        let (response_tx, response_rx) = oneshot::channel();
        let command = QueueCommand {
            kind,
            cancellation: cancellation.clone(),
            response: response_tx,
        };

        tokio::select! {
            _ = cancellation.cancelled() => {
                return Err(anyhow!("MCP request cancelled while waiting for queue slot"));
            }
            send_result = self.sender.send(command) => {
                send_result.with_context(|| format!("MCP server queue stopped: {}", self.server_name))?;
            }
        }

        tokio::select! {
            _ = cancellation.cancelled() => Err(anyhow!("MCP request cancelled while waiting for response")),
            response = response_rx => {
                response.context("MCP queue worker dropped response channel")?
            }
        }
    }
}

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

        for _ in 0..3 {
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

        for _ in 0..3 {
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

struct ServerProcess {
    service: RunningService<RoleClient, ()>,
    child: tokio::process::Child,
    _sandbox: Option<SandboxHandle>,
}

impl ServerProcess {
    async fn spawn(config: &McpServerConfig) -> Result<Self> {
        let mut cmd = Command::new(&config.command);
        cmd.args(&config.args);
        for (key, value) in &config.env {
            cmd.env(key, value);
        }

        let sandbox_config = SandboxConfig {
            memory_max_mb: MCP_SANDBOX_MEMORY_MAX_MB,
            memory_swap_max_mb: MCP_SANDBOX_MEMORY_SWAP_MAX_MB,
            pids_max: MCP_SANDBOX_PIDS_MAX,
        };

        let capability = detect_sandbox_capability();
        let (mut child, sandbox) = match capability {
            SandboxCapability::CgroupV2 => {
                // `systemd-run --scope` does not preserve interactive stdio semantics
                // required by MCP child-process transport. Use rlimit sandboxing.
                let child = spawn_with_rlimit_interactive(cmd, &sandbox_config)
                    .with_context(|| format!("failed to sandbox MCP server '{}'", config.name))?;
                (child, None)
            }
            SandboxCapability::Setrlimit | SandboxCapability::None => {
                let (child, sandbox) = spawn_tool_sandboxed(
                    cmd,
                    None,
                    SpawnOptions {
                        stdin_write_timeout: Duration::from_secs(
                            csa_process::DEFAULT_STDIN_WRITE_TIMEOUT_SECS,
                        ),
                        keep_stdin_open: true,
                    },
                    Some(&sandbox_config),
                    &config.name,
                    MCP_SANDBOX_SESSION_ID,
                )
                .await
                .with_context(|| format!("failed to sandbox MCP server '{}'", config.name))?;
                (child, Some(sandbox))
            }
        };

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("failed to capture stdout for MCP server '{}'", config.name))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("failed to capture stdin for MCP server '{}'", config.name))?;
        if let Some(mut stderr) = child.stderr.take() {
            tokio::spawn(async move {
                let mut sink = tokio::io::sink();
                let _ = tokio::io::copy(&mut stderr, &mut sink).await;
            });
        }

        let service = ()
            .serve((stdout, stdin))
            .await
            .with_context(|| format!("failed to spawn MCP server '{}'", config.name))?;

        Ok(Self {
            service,
            child,
            _sandbox: sandbox,
        })
    }

    async fn shutdown(mut self) {
        let _ = self.service.cancel().await;
        match tokio::time::timeout(Duration::from_secs(SHUTDOWN_GRACE_SECS), self.child.wait())
            .await
        {
            Ok(Ok(_)) => {}
            Ok(Err(error)) => {
                tracing::debug!(error = %error, "failed to wait MCP child process");
            }
            Err(_) => {
                let _ = self.child.kill().await;
            }
        }
    }
}

fn spawn_with_rlimit_interactive(
    mut cmd: Command,
    config: &SandboxConfig,
) -> Result<tokio::process::Child> {
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    cmd.stdin(std::process::Stdio::piped());
    cmd.kill_on_drop(true);

    let memory_max_mb = config.memory_max_mb;
    let pids_max = config.pids_max.map(u64::from);
    // SAFETY: setsid() and setrlimit are async-signal-safe and run before exec.
    #[cfg(unix)]
    unsafe {
        cmd.pre_exec(move || {
            libc::setsid();
            apply_rlimits(memory_max_mb, pids_max).map_err(io::Error::other)
        });
    }

    cmd.spawn()
        .context("failed to spawn interactive rlimit child")
}

#[cfg(test)]
#[path = "registry_tests.rs"]
mod tests;
