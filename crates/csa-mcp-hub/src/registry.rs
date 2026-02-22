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
use std::time::{Duration, Instant};
use tokio::process::Command;
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio_util::sync::CancellationToken;

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
}

enum ServerEntry {
    Stateless(Arc<ServerQueueHandle>),
    Stateful(Arc<StatefulServerPool>),
}

impl McpRegistry {
    pub(crate) fn new(configs: Vec<McpServerConfig>) -> Self {
        let mut servers = HashMap::new();
        for config in configs {
            let entry = if config.stateful {
                ServerEntry::Stateful(Arc::new(StatefulServerPool::new(config)))
            } else {
                ServerEntry::Stateless(Arc::new(ServerQueueHandle::spawn(config, None)))
            };
            servers.insert(entry.name().to_string(), entry);
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
    fn name(&self) -> &str {
        match self {
            Self::Stateless(queue) => &queue.server_name,
            Self::Stateful(pool) => &pool.server_name,
        }
    }

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

struct StatefulServerPool {
    server_name: String,
    config: McpServerConfig,
    max_warm_pools: usize,
    max_active_pools: usize,
    inner: Mutex<StatefulPoolInner>,
}

struct StatefulPoolInner {
    queues: HashMap<PoolKey, Arc<ServerQueueHandle>>,
    leases: LeaseTracker,
}

impl StatefulServerPool {
    fn new(config: McpServerConfig) -> Self {
        let warm_ttl = Duration::from_secs(DEFAULT_WARM_TTL_SECS);
        Self {
            server_name: config.name.clone(),
            config,
            max_warm_pools: DEFAULT_MAX_WARM_POOLS,
            max_active_pools: DEFAULT_MAX_ACTIVE_POOLS,
            inner: Mutex::new(StatefulPoolInner {
                queues: HashMap::new(),
                leases: LeaseTracker::new(warm_ttl),
            }),
        }
    }

    async fn list_tools(&self, cancellation: CancellationToken) -> Result<Vec<Tool>> {
        let queue = self.default_queue().await;
        queue.list_tools(cancellation).await
    }

    async fn default_queue(&self) -> Arc<ServerQueueHandle> {
        let default_key = PoolKey {
            project_root: PathBuf::from("/"),
            toolchain_hash: 0,
        };

        let mut inner = self.inner.lock().await;
        if let Some(existing) = inner.queues.get(&default_key) {
            return existing.clone();
        }

        let queue = Arc::new(ServerQueueHandle::spawn(
            self.config.clone(),
            Some(default_key.clone()),
        ));
        inner.leases.acquire(&default_key, Instant::now());
        inner.leases.release(&default_key, Instant::now());
        inner.queues.insert(default_key, queue.clone());
        queue
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParam,
        route: ToolCallRoute,
        cancellation: CancellationToken,
    ) -> Result<CallToolResult> {
        let key = PoolKey::from_route(route)?;

        let (queue, reclaim_handles) = {
            let mut inner = self.inner.lock().await;
            let now = Instant::now();
            let mut reclaim_keys = inner.leases.expire(now);
            let mut reclaim_handles = Vec::new();

            if reclaim_keys.iter().any(|expired_key| expired_key == &key) {
                reclaim_keys.retain(|expired_key| expired_key != &key);
                if let Some(stale_queue) = inner.queues.remove(&key) {
                    reclaim_handles.push(stale_queue);
                }
            }

            let queue = if let Some(existing) = inner.queues.get(&key).cloned() {
                inner.leases.acquire(&key, now);
                existing
            } else {
                if inner.leases.active_pool_count() >= self.max_active_pools {
                    return Err(anyhow!(
                        "stateful MCP pool limit reached: max_active_pools={} server={}",
                        self.max_active_pools,
                        self.server_name
                    ));
                }

                let queue = Arc::new(ServerQueueHandle::spawn(
                    self.config.clone(),
                    Some(key.clone()),
                ));
                inner.queues.insert(key.clone(), queue.clone());
                inner.leases.acquire(&key, now);
                queue
            };

            let pool_count = inner.queues.len();
            reclaim_keys.extend(inner.leases.reclaim_for_pressure(
                pool_count,
                self.max_warm_pools,
                &key,
            ));

            reclaim_handles.extend(inner.take_handles(&reclaim_keys));
            Ok::<_, anyhow::Error>((queue, reclaim_handles))
        }?;

        for handle in reclaim_handles {
            let _ = handle.shutdown().await;
        }

        let call_result = queue.call_tool(request, cancellation).await;

        let expire_handles = {
            let mut inner = self.inner.lock().await;
            inner.leases.release(&key, Instant::now());
            let expire_keys = inner.leases.expire(Instant::now());
            inner.take_handles(&expire_keys)
        };

        for handle in expire_handles {
            let _ = handle.shutdown().await;
        }

        call_result
    }

    async fn shutdown(&self) -> Result<()> {
        let handles = {
            let mut inner = self.inner.lock().await;
            inner.leases.clear();
            inner
                .queues
                .drain()
                .map(|(_, handle)| handle)
                .collect::<Vec<_>>()
        };

        for handle in handles {
            let _ = handle.shutdown().await;
        }

        Ok(())
    }
}

impl StatefulPoolInner {
    fn take_handles(&mut self, keys: &[PoolKey]) -> Vec<Arc<ServerQueueHandle>> {
        let mut handles = Vec::new();
        for key in keys {
            if let Some(handle) = self.queues.remove(key) {
                handles.push(handle);
            }
        }
        handles
    }
}

struct LeaseTracker {
    warm_ttl: Duration,
    leases: HashMap<PoolKey, LeaseState>,
}

#[derive(Clone, Copy)]
struct LeaseState {
    active_leases: usize,
    last_release: Instant,
}

impl LeaseTracker {
    fn new(warm_ttl: Duration) -> Self {
        Self {
            warm_ttl,
            leases: HashMap::new(),
        }
    }

    fn acquire(&mut self, key: &PoolKey, now: Instant) {
        let lease = self.leases.entry(key.clone()).or_insert(LeaseState {
            active_leases: 0,
            last_release: now,
        });
        lease.active_leases = lease.active_leases.saturating_add(1);
    }

    fn release(&mut self, key: &PoolKey, now: Instant) {
        if let Some(lease) = self.leases.get_mut(key) {
            if lease.active_leases > 0 {
                lease.active_leases -= 1;
            }
            if lease.active_leases == 0 {
                lease.last_release = now;
            }
        }
    }

    fn active_pool_count(&self) -> usize {
        self.leases
            .values()
            .filter(|lease| lease.active_leases > 0)
            .count()
    }

    #[cfg(test)]
    fn active_leases(&self, key: &PoolKey) -> usize {
        self.leases
            .get(key)
            .map(|lease| lease.active_leases)
            .unwrap_or_default()
    }

    fn expire(&mut self, now: Instant) -> Vec<PoolKey> {
        let expired = self
            .leases
            .iter()
            .filter_map(|(key, lease)| {
                if lease.active_leases == 0
                    && now.saturating_duration_since(lease.last_release) >= self.warm_ttl
                {
                    Some(key.clone())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();

        for key in &expired {
            self.leases.remove(key);
        }

        expired
    }

    fn reclaim_for_pressure(
        &mut self,
        pool_count: usize,
        max_warm_pools: usize,
        protected_key: &PoolKey,
    ) -> Vec<PoolKey> {
        if pool_count <= max_warm_pools {
            return Vec::new();
        }

        let mut candidates = self
            .leases
            .iter()
            .filter_map(|(key, lease)| {
                if key == protected_key || lease.active_leases > 0 {
                    return None;
                }
                Some((key.clone(), lease.last_release))
            })
            .collect::<Vec<_>>();

        candidates.sort_by_key(|(_, last_release)| *last_release);

        let reclaim_count = pool_count.saturating_sub(max_warm_pools);
        let reclaimed = candidates
            .into_iter()
            .take(reclaim_count)
            .map(|(key, _)| key)
            .collect::<Vec<_>>();

        for key in &reclaimed {
            self.leases.remove(key);
        }

        reclaimed
    }

    fn clear(&mut self) {
        self.leases.clear();
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
        let (command, args, env) = match &config.transport {
            csa_config::McpTransport::Stdio {
                command, args, env, ..
            } => (command.as_str(), args.as_slice(), env),
            _ => {
                anyhow::bail!(
                    "ServerProcess::spawn() only supports stdio transport, \
                     but server '{}' uses a remote transport",
                    config.name
                );
            }
        };
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
