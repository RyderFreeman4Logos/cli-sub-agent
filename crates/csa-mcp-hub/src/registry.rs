#[cfg(feature = "transport-http-client")]
#[path = "registry_http.rs"]
mod registry_http;
#[path = "registry_pool.rs"]
mod registry_pool;

use anyhow::{Context, Result, anyhow};
use csa_config::McpServerConfig;
use csa_process::{SandboxHandle, SpawnOptions, spawn_tool_sandboxed};
use csa_resource::{SandboxCapability, SandboxConfig, apply_rlimits, detect_sandbox_capability};
use rmcp::RoleClient;
use rmcp::model::{CallToolRequestParam, CallToolResult, Tool};
use rmcp::service::{RunningService, ServiceExt};
use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

// Re-export HTTP safety functions for tests and internal use.
#[cfg(feature = "transport-http-client")]
pub(crate) use registry_http::{is_ssrf_dangerous_ip, parse_host_port, validate_http_url};

#[cfg(test)]
use registry_pool::LeaseTracker;
use registry_pool::StatefulServerPool;

const RESTART_BACKOFF_INITIAL_MS: u64 = 100;
const RESTART_BACKOFF_MAX_MS: u64 = 30_000;
const MCP_SANDBOX_MEMORY_MAX_MB: u64 = 2048;
const MCP_SANDBOX_MEMORY_SWAP_MAX_MB: Option<u64> = Some(0);
const MCP_SANDBOX_PIDS_MAX: Option<u32> = None;
const MCP_SANDBOX_SESSION_ID: &str = "mcp-hub";
const SHUTDOWN_GRACE_SECS: u64 = 3;
const REQUEST_QUEUE_CAPACITY: usize = 64;
const DEFAULT_WARM_TTL_SECS: u64 = 10 * 60;
const DEFAULT_MAX_WARM_POOLS: usize = 16;
const DEFAULT_MAX_ACTIVE_POOLS: usize = 64;
#[derive(Debug, Clone, Default)]
pub(crate) struct ToolCallRoute {
    pub(crate) project_root: Option<PathBuf>,
    pub(crate) toolchain_hash: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct PoolKey {
    pub(crate) project_root: PathBuf,
    pub(crate) toolchain_hash: u64,
}

impl PoolKey {
    fn from_route(route: ToolCallRoute) -> Result<Self> {
        let project_root = route
            .project_root
            .ok_or_else(|| anyhow!("stateful MCP call missing required parameter: project_root"))?;
        Ok(Self {
            project_root,
            toolchain_hash: route.toolchain_hash.unwrap_or_default(),
        })
    }
}

pub(crate) struct McpRegistry {
    servers: HashMap<String, ServerEntry>,
    transport_labels: HashMap<String, String>,
}

enum ServerEntry {
    Stateless(Arc<ServerQueueHandle>),
    Stateful(Arc<StatefulServerPool>),
}

impl McpRegistry {
    pub(crate) fn new(configs: Vec<McpServerConfig>) -> Self {
        let mut servers = HashMap::new();
        let mut transport_labels = HashMap::new();
        for config in configs {
            let label = config.transport.label().to_string();
            let name = config.name.clone();
            let entry = if config.stateful {
                ServerEntry::Stateful(Arc::new(StatefulServerPool::new(config)))
            } else {
                ServerEntry::Stateless(Arc::new(ServerQueueHandle::spawn(config, None)))
            };
            servers.insert(name.clone(), entry);
            transport_labels.insert(name, label);
        }
        Self {
            servers,
            transport_labels,
        }
    }

    pub(crate) fn server_names(&self) -> Vec<String> {
        self.servers.keys().cloned().collect()
    }

    /// Returns the transport label (stdio/http/sse) for a server.
    pub(crate) fn transport_label(&self, server_name: &str) -> &str {
        self.transport_labels
            .get(server_name)
            .map(String::as_str)
            .unwrap_or("stdio")
    }

    pub(crate) async fn list_tools(
        &self,
        server_name: &str,
        cancellation: CancellationToken,
    ) -> Result<Vec<Tool>> {
        let entry = self
            .servers
            .get(server_name)
            .with_context(|| format!("unknown MCP server: {server_name}"))?;

        match entry {
            ServerEntry::Stateless(queue) => queue.list_tools(cancellation).await,
            ServerEntry::Stateful(pool) => pool.list_tools(cancellation).await,
        }
    }

    pub(crate) async fn call_tool(
        &self,
        server_name: &str,
        request: CallToolRequestParam,
        route: ToolCallRoute,
        cancellation: CancellationToken,
    ) -> Result<CallToolResult> {
        let entry = self
            .servers
            .get(server_name)
            .with_context(|| format!("unknown MCP server: {server_name}"))?;

        match entry {
            ServerEntry::Stateless(queue) => queue.call_tool(request, cancellation).await,
            ServerEntry::Stateful(pool) => pool.call_tool(request, route, cancellation).await,
        }
    }

    pub(crate) async fn shutdown_all(&self) -> Result<()> {
        for entry in self.servers.values() {
            entry.shutdown().await?;
        }
        Ok(())
    }
}

impl ServerEntry {
    async fn shutdown(&self) -> Result<()> {
        match self {
            Self::Stateless(queue) => queue.shutdown().await,
            Self::Stateful(pool) => pool.shutdown().await,
        }
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
    fn spawn(config: McpServerConfig, pool_key: Option<PoolKey>) -> Self {
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

            if let Some(key) = pool_key {
                tracing::debug!(
                    server = %queue_server_name,
                    project_root = %key.project_root.display(),
                    toolchain_hash = key.toolchain_hash,
                    "stateful MCP pool worker stopped"
                );
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
    transport: Option<BackendTransport>,
    restart_backoff: Duration,
}

impl ManagedServer {
    fn new(config: McpServerConfig) -> Self {
        Self {
            config,
            transport: None,
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
            if let Some(transport) = self.transport.as_ref() {
                match transport.service().list_tools(None).await {
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
            if let Some(transport) = self.transport.as_ref() {
                match transport.service().call_tool(request.clone()).await {
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
        if self.transport.is_some() {
            return Ok(());
        }

        self.transport = Some(BackendTransport::connect(&self.config).await?);
        Ok(())
    }

    async fn restart_after_failure(&mut self) -> Result<()> {
        if let Some(transport) = self.transport.take() {
            transport.shutdown().await;
        }

        tokio::time::sleep(self.restart_backoff).await;
        self.restart_backoff =
            (self.restart_backoff * 2).min(Duration::from_millis(RESTART_BACKOFF_MAX_MS));
        Ok(())
    }

    async fn shutdown(&mut self) -> Result<()> {
        if let Some(transport) = self.transport.take() {
            transport.shutdown().await;
        }
        Ok(())
    }
}

/// Unified backend connection to an MCP server.
///
/// Each variant owns its lifecycle independently. The common surface
/// (`service()`, `shutdown()`) delegates to variant-specific behavior.
enum BackendTransport {
    /// Child process communicating over stdio (JSON-RPC on stdin/stdout).
    Stdio {
        service: RunningService<RoleClient, ()>,
        child: Box<tokio::process::Child>,
        _sandbox: Option<SandboxHandle>,
    },
    /// Remote MCP server via Streamable HTTP transport.
    #[cfg(feature = "transport-http-client")]
    Http {
        service: RunningService<RoleClient, ()>,
    },
}

impl BackendTransport {
    /// Connect to an MCP server based on the config transport type.
    async fn connect(config: &McpServerConfig) -> Result<Self> {
        match &config.transport {
            csa_config::McpTransport::Stdio {
                command, args, env, ..
            } => Self::spawn_stdio(config, command, args, env).await,
            #[cfg(feature = "transport-http-client")]
            csa_config::McpTransport::Http {
                url,
                allow_insecure,
                ..
            }
            | csa_config::McpTransport::Sse {
                url,
                allow_insecure,
                ..
            } => Self::connect_http(config, url, *allow_insecure).await,
            #[cfg(not(feature = "transport-http-client"))]
            csa_config::McpTransport::Http { .. } | csa_config::McpTransport::Sse { .. } => {
                anyhow::bail!(
                    "server '{}' requires HTTP transport, but csa-mcp-hub was built \
                     without the 'transport-http-client' feature",
                    config.name
                );
            }
        }
    }

    /// Transport-agnostic accessor for the rmcp service.
    fn service(&self) -> &RunningService<RoleClient, ()> {
        match self {
            Self::Stdio { service, .. } => service,
            #[cfg(feature = "transport-http-client")]
            Self::Http { service, .. } => service,
        }
    }

    /// Graceful shutdown adapting to transport type.
    async fn shutdown(self) {
        match self {
            Self::Stdio {
                service, mut child, ..
            } => {
                let _ = service.cancel().await;
                match tokio::time::timeout(Duration::from_secs(SHUTDOWN_GRACE_SECS), child.wait())
                    .await
                {
                    Ok(Ok(_)) => {}
                    Ok(Err(error)) => {
                        tracing::debug!(error = %error, "failed to wait MCP child process");
                    }
                    Err(_) => {
                        let _ = child.kill().await;
                    }
                }
            }
            #[cfg(feature = "transport-http-client")]
            Self::Http { service, .. } => {
                let _ = service.cancel().await;
            }
        }
    }

    /// Spawn a stdio child process and negotiate MCP handshake.
    async fn spawn_stdio(
        config: &McpServerConfig,
        command: &str,
        args: &[String],
        env: &HashMap<String, String>,
    ) -> Result<Self> {
        let mut cmd = Command::new(command);
        cmd.args(args);
        for (key, value) in env {
            cmd.env(key, value);
        }

        let sandbox_config = SandboxConfig {
            memory_max_mb: config.memory_max_mb.unwrap_or(MCP_SANDBOX_MEMORY_MAX_MB),
            memory_swap_max_mb: MCP_SANDBOX_MEMORY_SWAP_MAX_MB,
            pids_max: MCP_SANDBOX_PIDS_MAX,
        };

        let capability = detect_sandbox_capability();
        let (mut child, sandbox) = match capability {
            SandboxCapability::CgroupV2 => {
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

        Ok(Self::Stdio {
            service,
            child: Box::new(child),
            _sandbox: sandbox,
        })
    }

    /// Connect to a remote MCP server via Streamable HTTP.
    ///
    /// Performs URL safety validation before establishing the connection:
    /// - Scheme whitelist: only `http` and `https` are allowed
    /// - HTTPS enforcement: `http://` is rejected unless `allow_insecure` is set
    /// - SSRF protection: loopback, RFC1918, link-local, and cloud metadata IPs are blocked
    #[cfg(feature = "transport-http-client")]
    async fn connect_http(
        config: &McpServerConfig,
        url: &str,
        allow_insecure: bool,
    ) -> Result<Self> {
        use rmcp::transport::StreamableHttpClientTransport;

        validate_http_url(url, allow_insecure, &config.name)?;
        preflight_ssrf_check(url, &config.name)?;

        tracing::info!(server = %config.name, url = %url, "connecting to HTTP MCP server");

        let transport = StreamableHttpClientTransport::from_uri(url);

        let service: RunningService<RoleClient, ()> = ().serve(transport).await.with_context(|| {
            format!(
                "failed to connect to HTTP MCP server '{}' at {url}",
                config.name
            )
        })?;

        Ok(Self::Http { service })
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
