use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use axum::extract::DefaultBodyLimit;
use rmcp::transport::{SseServer, sse_server::SseServerConfig};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{Semaphore, mpsc};
use tokio_util::sync::CancellationToken;

use crate::config::{HubConfig, default_socket_path};
use crate::proxy::ProxyRouter;
use crate::registry::McpRegistry;
use crate::skill_writer::{
    SkillRefreshSignal, parse_tools_list_changed_signal, regenerate_routing_skill_once,
    spawn_skill_sync_task,
};
use crate::socket;

const SSE_PATH: &str = "/";
const SSE_POST_PATH: &str = "/message";

pub async fn handle_serve_command(
    background: bool,
    foreground: bool,
    socket_override: Option<String>,
    http_bind_override: Option<String>,
    http_port_override: Option<u16>,
    systemd_activation: bool,
) -> Result<()> {
    if background && !foreground {
        let pid = spawn_background(
            socket_override.as_deref(),
            http_bind_override.as_deref(),
            http_port_override,
            systemd_activation,
        )?;
        println!("mcp-hub started in background (pid={pid})");
        return Ok(());
    }

    let cfg = HubConfig::load(
        socket_override.map(PathBuf::from),
        http_bind_override,
        http_port_override,
    )?;
    run_hub(cfg, systemd_activation).await
}

pub async fn handle_status_command(socket_override: Option<String>) -> Result<()> {
    let socket_path = socket_override
        .map(PathBuf::from)
        .unwrap_or_else(default_socket_path);

    match send_control_request(&socket_path, "hub/status").await {
        Ok(response) => {
            if let Some(result) = response.get("result") {
                let servers = result.get("servers").cloned().unwrap_or_else(|| json!([]));
                println!(
                    "mcp-hub is running at {} (servers={})",
                    socket_path.display(),
                    servers
                );
            } else {
                println!(
                    "mcp-hub responded at {}, but status payload was empty",
                    socket_path.display()
                );
            }
        }
        Err(_) => {
            println!("mcp-hub is not running at {}", socket_path.display());
        }
    }

    Ok(())
}

pub async fn handle_stop_command(socket_override: Option<String>) -> Result<()> {
    let socket_path = socket_override
        .map(PathBuf::from)
        .unwrap_or_else(default_socket_path);

    let response = send_control_request(&socket_path, "hub/stop")
        .await
        .with_context(|| format!("failed to stop mcp-hub at {}", socket_path.display()))?;

    if response.get("error").is_some() {
        bail!("mcp-hub returned an error while stopping: {response}");
    }

    println!("mcp-hub stop signal sent to {}", socket_path.display());
    Ok(())
}

pub async fn handle_gen_skill_command(socket_override: Option<String>) -> Result<()> {
    let socket_path = socket_override
        .map(PathBuf::from)
        .unwrap_or_else(default_socket_path);

    match send_control_request(&socket_path, "hub/gen-skill").await {
        Ok(response) => {
            if response.get("error").is_some() {
                bail!("mcp-hub returned an error while regenerating skill: {response}");
            }
            println!(
                "requested routing-guide skill regeneration via running hub at {}",
                socket_path.display()
            );
            Ok(())
        }
        Err(_) => {
            let cfg = HubConfig::load(None, None, None)?;
            regenerate_routing_skill_once(cfg).await?;
            println!("generated routing-guide skill via one-shot mcp-hub run");
            Ok(())
        }
    }
}

pub(crate) async fn run_hub(cfg: HubConfig, systemd_activation: bool) -> Result<()> {
    let mut activated_by_systemd = false;
    let listener = if systemd_activation {
        if let Some(listener) = socket::bind_systemd_activated_listener()? {
            activated_by_systemd = true;
            listener
        } else {
            socket::bind_listener(&cfg.socket_path).await?
        }
    } else {
        socket::bind_listener(&cfg.socket_path).await?
    };

    write_pid_file(&cfg.pid_path).await?;

    let registry = Arc::new(McpRegistry::new(cfg.mcp_servers.clone()));
    let router = Arc::new(ProxyRouter::new(registry.clone(), cfg.request_timeout()));
    let http_endpoint = HttpEndpoint::start(&cfg, router.clone()).await?;
    let skill_sync = spawn_skill_sync_task(cfg.clone(), registry.clone());
    let skill_notify_tx = skill_sync.notifier();
    let next_client_id = Arc::new(AtomicU64::new(1));
    let max_connections = cfg.max_connections.max(1);
    let connection_slots = Arc::new(Semaphore::new(max_connections));
    let connection_policy = ConnectionPolicy::from_config(&cfg);
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);

    println!(
        "mcp-hub listening on unix://{} and http://{}{}",
        cfg.socket_path.display(),
        http_endpoint.addr,
        SSE_PATH
    );
    println!(
        "claude mcp add --transport http csa-hub http://{}",
        http_endpoint.addr
    );

    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                let _ = shutdown_tx.send(true);
            }
            changed = shutdown_rx.changed() => {
                if changed.is_ok() && *shutdown_rx.borrow() {
                    break;
                }
            }
            accept_result = listener.accept() => {
                let (stream, _addr) = accept_result.context("failed to accept mcp-hub client")?;
                let permit = match connection_slots.clone().try_acquire_owned() {
                    Ok(permit) => permit,
                    Err(_) => {
                        tracing::warn!(
                            max_connections,
                            "rejecting mcp-hub connection: connection limit reached"
                        );
                        continue;
                    }
                };

                let client_id = next_client_id.fetch_add(1, Ordering::Relaxed);
                let client_router = router.clone();
                let client_shutdown_tx = shutdown_tx.clone();
                let client_skill_notify_tx = skill_notify_tx.clone();
                tokio::spawn(async move {
                    let _permit = permit;
                    if let Err(error) = handle_client_connection(
                        stream,
                        client_id,
                        client_router,
                        client_shutdown_tx,
                        connection_policy,
                        client_skill_notify_tx,
                    )
                    .await
                    {
                        tracing::warn!(client_id, error = %error, "mcp-hub client connection failed");
                    }
                });
            }
        }
    }

    skill_sync.shutdown().await;
    http_endpoint.shutdown().await;
    registry.shutdown_all().await?;
    cleanup_pid_file(&cfg.pid_path).await?;
    if !activated_by_systemd {
        socket::cleanup_socket_file(&cfg.socket_path).await?;
    }

    Ok(())
}

#[derive(Debug)]
struct HttpEndpoint {
    addr: SocketAddr,
    shutdown: CancellationToken,
    server_task: tokio::task::JoinHandle<()>,
}

impl HttpEndpoint {
    async fn start(cfg: &HubConfig, router: Arc<ProxyRouter>) -> Result<Self> {
        let bind_addr = format!("{}:{}", cfg.http_bind, cfg.http_port)
            .parse::<SocketAddr>()
            .with_context(|| {
                format!(
                    "invalid mcp-hub HTTP bind address '{}:{}'",
                    cfg.http_bind, cfg.http_port
                )
            })?;

        let listener = tokio::net::TcpListener::bind(bind_addr)
            .await
            .with_context(|| format!("failed to bind mcp-hub HTTP endpoint at {}", bind_addr))?;
        let local_addr = listener
            .local_addr()
            .context("failed to resolve local mcp-hub HTTP address")?;

        let shutdown = CancellationToken::new();
        let (sse_server, sse_router) = SseServer::new(SseServerConfig {
            bind: local_addr,
            sse_path: SSE_PATH.to_string(),
            post_path: SSE_POST_PATH.to_string(),
            ct: shutdown.clone(),
            sse_keep_alive: None,
        });
        let _server_ct = sse_server.with_service_directly({
            let hub_service = (*router).clone();
            move || hub_service.clone()
        });

        let app = sse_router.layer(DefaultBodyLimit::max(cfg.max_request_body_bytes));
        let server_shutdown = shutdown.clone();
        let server_task = tokio::spawn(async move {
            if let Err(error) = axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    server_shutdown.cancelled().await;
                })
                .await
            {
                tracing::warn!(error = %error, "mcp-hub HTTP server stopped with error");
            }
        });

        Ok(Self {
            addr: local_addr,
            shutdown,
            server_task,
        })
    }

    async fn shutdown(self) {
        self.shutdown.cancel();
        if let Err(error) = self.server_task.await {
            tracing::debug!(error = %error, "mcp-hub HTTP server join failed");
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct ConnectionPolicy {
    max_requests_per_sec: u32,
    max_request_body_bytes: usize,
    request_timeout: Duration,
    current_uid: u32,
}

impl ConnectionPolicy {
    fn from_config(cfg: &HubConfig) -> Self {
        Self {
            max_requests_per_sec: cfg.max_requests_per_sec.max(1),
            max_request_body_bytes: cfg.max_request_body_bytes.max(1),
            request_timeout: cfg.request_timeout(),
            current_uid: current_uid(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct TokenBucket {
    capacity: f64,
    tokens: f64,
    refill_per_sec: f64,
    last_refill: Instant,
}

impl TokenBucket {
    fn new(max_requests_per_sec: u32) -> Self {
        let refill_per_sec = f64::from(max_requests_per_sec.max(1));
        Self {
            capacity: refill_per_sec,
            tokens: refill_per_sec,
            refill_per_sec,
            last_refill: Instant::now(),
        }
    }

    fn try_consume(&mut self) -> bool {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        self.last_refill = now;
        self.tokens = (self.tokens + elapsed * self.refill_per_sec).min(self.capacity);
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

async fn handle_client_connection(
    stream: tokio::net::UnixStream,
    client_id: u64,
    router: Arc<ProxyRouter>,
    shutdown_tx: tokio::sync::watch::Sender<bool>,
    policy: ConnectionPolicy,
    skill_notify_tx: mpsc::UnboundedSender<SkillRefreshSignal>,
) -> Result<()> {
    let peer_uid = stream
        .peer_cred()
        .context("failed to read peer credentials")?
        .uid();
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);
    let mut limiter = TokenBucket::new(policy.max_requests_per_sec);
    let mut first_line = String::new();

    let bytes =
        match tokio::time::timeout(policy.request_timeout, reader.read_line(&mut first_line)).await
        {
            Ok(Ok(bytes)) => bytes,
            Ok(Err(error)) => return Err(error).context("failed to read client request line"),
            Err(_) => {
                write_json_line(
                    &mut write_half,
                    &jsonrpc_error(None, -32001, "request timed out".to_string()),
                )
                .await?;
                return Ok(());
            }
        };

    if bytes == 0 || first_line.trim().is_empty() {
        return Ok(());
    }

    if first_line.len() > policy.max_request_body_bytes {
        write_json_line(
            &mut write_half,
            &jsonrpc_error(None, -32002, "request body too large".to_string()),
        )
        .await?;
        return Ok(());
    }

    if !limiter.try_consume() {
        write_json_line(
            &mut write_half,
            &jsonrpc_error(None, -32003, "rate limit exceeded".to_string()),
        )
        .await?;
        return Ok(());
    }

    let first_message: Value = match serde_json::from_str(first_line.trim()) {
        Ok(value) => value,
        Err(error) => {
            write_json_line(
                &mut write_half,
                &jsonrpc_error(None, -32700, format!("invalid JSON-RPC request: {error}")),
            )
            .await?;
            return Ok(());
        }
    };

    let method = first_message.get("method").and_then(Value::as_str);
    let request_id = first_message.get("id").cloned();
    maybe_notify_tools_list_changed(first_line.trim(), &skill_notify_tx);

    if method == Some("hub/status") {
        let result = router.status_payload().await;
        write_json_line(&mut write_half, &jsonrpc_result(request_id, result)).await?;
        return Ok(());
    }

    if method == Some("hub/stop") {
        if peer_uid != policy.current_uid {
            write_json_line(
                &mut write_half,
                &jsonrpc_error(
                    request_id,
                    -32004,
                    "permission denied: peer uid does not match hub uid".to_string(),
                ),
            )
            .await?;
            return Ok(());
        }
        let _ = shutdown_tx.send(true);
        write_json_line(
            &mut write_half,
            &jsonrpc_result(request_id, json!({"stopping": true})),
        )
        .await?;
        return Ok(());
    }

    if method == Some("hub/gen-skill") {
        if peer_uid != policy.current_uid {
            write_json_line(
                &mut write_half,
                &jsonrpc_error(
                    request_id,
                    -32004,
                    "permission denied: peer uid does not match hub uid".to_string(),
                ),
            )
            .await?;
            return Ok(());
        }

        let _ = skill_notify_tx.send(SkillRefreshSignal::RegenerateAll);
        write_json_line(
            &mut write_half,
            &jsonrpc_result(request_id, json!({"queued": true})),
        )
        .await?;
        return Ok(());
    }

    let mut forwarding_reader = reader;
    let first_forward_line = first_line;
    let (prefill_read, mut prefill_write) = tokio::io::duplex(64 * 1024);
    let notify_tx = skill_notify_tx.clone();

    let copy_task = tokio::spawn(async move {
        if prefill_write
            .write_all(first_forward_line.as_bytes())
            .await
            .is_err()
        {
            let _ = prefill_write.shutdown().await;
            return;
        }

        let mut line = String::new();
        loop {
            line.clear();
            let read_result = tokio::time::timeout(
                policy.request_timeout,
                forwarding_reader.read_line(&mut line),
            )
            .await;

            let bytes = match read_result {
                Ok(Ok(bytes)) => bytes,
                Ok(Err(error)) => {
                    tracing::debug!(client_id, error = %error, "failed to read MCP request frame");
                    break;
                }
                Err(_) => {
                    tracing::warn!(client_id, "closing connection due to request timeout");
                    break;
                }
            };

            if bytes == 0 {
                break;
            }
            maybe_notify_tools_list_changed(line.trim(), &notify_tx);
            if line.len() > policy.max_request_body_bytes {
                tracing::warn!(client_id, "closing connection: request body too large");
                break;
            }
            if !limiter.try_consume() {
                tracing::warn!(client_id, "closing connection: rate limit exceeded");
                break;
            }
            if prefill_write.write_all(line.as_bytes()).await.is_err() {
                break;
            }
        }

        let _ = prefill_write.shutdown().await;
    });

    let running =
        rmcp::service::serve_directly((*router).clone(), (prefill_read, write_half), None);
    let waiting_result = running.waiting().await;
    copy_task
        .await
        .context("failed to join MCP stream forwarding task")?;
    let _ = waiting_result.context("failed to join rmcp server task")?;
    Ok(())
}

async fn write_json_line<W: AsyncWrite + Unpin>(writer: &mut W, value: &Value) -> Result<()> {
    let payload = serde_json::to_string(value).context("failed to serialize JSON-RPC payload")?;
    writer
        .write_all(payload.as_bytes())
        .await
        .context("failed to write JSON-RPC payload")?;
    writer
        .write_all(b"\n")
        .await
        .context("failed to write JSON-RPC delimiter")?;
    writer
        .flush()
        .await
        .context("failed to flush JSON-RPC payload")
}

fn jsonrpc_result(id: Option<Value>, result: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result,
    })
}

fn jsonrpc_error(id: Option<Value>, code: i64, message: String) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": code,
            "message": message,
        }
    })
}

fn maybe_notify_tools_list_changed(
    payload: &str,
    notify_tx: &mpsc::UnboundedSender<SkillRefreshSignal>,
) {
    if let Some(signal) = parse_tools_list_changed_signal(payload) {
        let _ = notify_tx.send(signal);
    }
}

async fn send_control_request(socket_path: &Path, method: &str) -> Result<Value> {
    let mut stream = socket::connect(socket_path).await?;
    let request = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
    });

    let payload = serde_json::to_string(&request).context("failed to serialize control request")?;
    stream
        .write_all(payload.as_bytes())
        .await
        .context("failed to write control request")?;
    stream
        .write_all(b"\n")
        .await
        .context("failed to write control request delimiter")?;
    stream
        .flush()
        .await
        .context("failed to flush control request")?;

    let mut line = String::new();
    let mut reader = BufReader::new(stream);
    let bytes = reader
        .read_line(&mut line)
        .await
        .context("failed to read control response")?;
    if bytes == 0 {
        bail!("mcp-hub closed connection before responding");
    }

    serde_json::from_str(line.trim()).context("failed to parse control response")
}

fn spawn_background(
    socket_override: Option<&str>,
    http_bind_override: Option<&str>,
    http_port_override: Option<u16>,
    systemd_activation: bool,
) -> Result<u32> {
    let exe = std::env::current_exe().context("failed to resolve current executable")?;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("mcp-hub").arg("serve").arg("--foreground");
    if let Some(socket_path) = socket_override {
        cmd.arg("--socket").arg(socket_path);
    }
    if let Some(http_bind) = http_bind_override {
        cmd.arg("--http-bind").arg(http_bind);
    }
    if let Some(http_port) = http_port_override {
        cmd.arg("--http-port").arg(http_port.to_string());
    }
    if systemd_activation {
        cmd.arg("--systemd-activation");
    }

    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    let child = cmd.spawn().context("failed to spawn background mcp-hub")?;
    Ok(child.id())
}

async fn write_pid_file(pid_path: &Path) -> Result<()> {
    if let Some(parent) = pid_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create pid directory: {}", parent.display()))?;
    }

    tokio::fs::write(pid_path, format!("{}\n", std::process::id()))
        .await
        .with_context(|| format!("failed to write pid file: {}", pid_path.display()))
}

async fn cleanup_pid_file(pid_path: &Path) -> Result<()> {
    if pid_path.exists() {
        tokio::fs::remove_file(pid_path)
            .await
            .with_context(|| format!("failed to remove pid file: {}", pid_path.display()))?;
    }
    Ok(())
}

fn current_uid() -> u32 {
    #[cfg(unix)]
    {
        // SAFETY: `geteuid` has no preconditions and returns caller effective UID.
        unsafe { libc::geteuid() }
    }

    #[cfg(not(unix))]
    {
        0
    }
}

#[cfg(test)]
mod tests {
    use anyhow::Result;
    use serde_json::json;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    use super::send_control_request;
    use crate::proxy::ProxyRouter;
    use crate::registry::McpRegistry;

    #[tokio::test]
    async fn send_control_request_round_trip() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let socket_path = temp.path().join("control.sock");
        let listener = tokio::net::UnixListener::bind(&socket_path)?;

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept control client");
            let (read_half, mut write_half) = stream.into_split();
            let mut reader = BufReader::new(read_half);
            let mut line = String::new();
            reader
                .read_line(&mut line)
                .await
                .expect("read control request");
            let request: serde_json::Value =
                serde_json::from_str(line.trim()).expect("parse request");
            assert_eq!(request["method"], "hub/status");
            write_half
                .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"running\":true}}\n")
                .await
                .expect("write control response");
        });

        let response = send_control_request(&socket_path, "hub/status").await?;
        assert_eq!(
            response,
            json!({"jsonrpc":"2.0","id":1,"result":{"running":true}})
        );

        server.await?;
        Ok(())
    }

    #[test]
    fn token_bucket_refills_over_time() {
        let mut limiter = super::TokenBucket::new(2);
        assert!(limiter.try_consume());
        assert!(limiter.try_consume());
        assert!(!limiter.try_consume());
        std::thread::sleep(Duration::from_millis(600));
        assert!(limiter.try_consume());
    }

    #[tokio::test]
    async fn large_first_frame_is_processed_without_deadlock() -> Result<()> {
        let (client, server) = tokio::net::UnixStream::pair()?;
        let router = Arc::new(ProxyRouter::new(
            Arc::new(McpRegistry::new(Vec::new())),
            Duration::from_secs(2),
        ));
        let (shutdown_tx, _shutdown_rx) = tokio::sync::watch::channel(false);
        let policy = super::ConnectionPolicy {
            max_requests_per_sec: 100,
            max_request_body_bytes: 10 * 1024 * 1024,
            request_timeout: Duration::from_secs(2),
            current_uid: super::current_uid(),
        };
        let (skill_notify_tx, _skill_notify_rx) = tokio::sync::mpsc::unbounded_channel();

        let server_task = tokio::spawn(super::handle_client_connection(
            server,
            1,
            router,
            shutdown_tx,
            policy,
            skill_notify_tx,
        ));

        let (client_read, mut client_write) = client.into_split();
        let request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {
                    "name": "test-client",
                    "version": "0.1.0"
                },
                "padding": "x".repeat(70 * 1024)
            }
        });
        let mut payload = serde_json::to_string(&request)?;
        payload.push('\n');

        tokio::time::timeout(
            Duration::from_secs(2),
            client_write.write_all(payload.as_bytes()),
        )
        .await??;
        tokio::time::timeout(Duration::from_secs(2), client_write.shutdown()).await??;

        let mut reader = BufReader::new(client_read);
        let mut response = String::new();
        let _ =
            tokio::time::timeout(Duration::from_secs(2), reader.read_line(&mut response)).await??;

        tokio::time::timeout(Duration::from_secs(2), server_task).await???;
        Ok(())
    }
}
