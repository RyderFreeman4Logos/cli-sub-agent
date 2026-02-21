use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::config::{HubConfig, default_socket_path};
use crate::proxy::{JsonRpcRequest, ProxyRouter, error_response};
use crate::registry::McpRegistry;
use crate::socket;

pub async fn handle_serve_command(
    background: bool,
    foreground: bool,
    socket_override: Option<String>,
    systemd_activation: bool,
) -> Result<()> {
    if background && !foreground {
        let pid = spawn_background(socket_override.as_deref(), systemd_activation)?;
        println!("mcp-hub started in background (pid={pid})");
        return Ok(());
    }

    let cfg = HubConfig::load(socket_override.map(PathBuf::from))?;
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

    let registry = Arc::new(McpRegistry::new(cfg.mcp_servers));
    let router = Arc::new(ProxyRouter::new(registry.clone()));
    let next_client_id = Arc::new(AtomicU64::new(1));
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);

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
                let client_id = next_client_id.fetch_add(1, Ordering::Relaxed);
                let client_router = router.clone();
                let client_shutdown_tx = shutdown_tx.clone();
                tokio::spawn(async move {
                    if let Err(error) = handle_client_connection(stream, client_id, client_router, client_shutdown_tx).await {
                        tracing::warn!(client_id, error = %error, "mcp-hub client connection failed");
                    }
                });
            }
        }
    }

    registry.shutdown_all().await?;
    cleanup_pid_file(&cfg.pid_path).await?;
    if !activated_by_systemd {
        socket::cleanup_socket_file(&cfg.socket_path).await?;
    }

    Ok(())
}

async fn handle_client_connection(
    stream: tokio::net::UnixStream,
    client_id: u64,
    router: Arc<ProxyRouter>,
    shutdown_tx: tokio::sync::watch::Sender<bool>,
) -> Result<()> {
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);
    let mut line = String::new();

    loop {
        line.clear();
        let bytes = reader
            .read_line(&mut line)
            .await
            .context("failed to read client request line")?;
        if bytes == 0 {
            return Ok(());
        }
        if line.trim().is_empty() {
            continue;
        }

        let request: JsonRpcRequest = match serde_json::from_str(line.trim()) {
            Ok(req) => req,
            Err(error) => {
                let response =
                    error_response(None, -32700, format!("invalid JSON-RPC request: {error}"));
                let payload = serde_json::to_string(&response)
                    .context("failed to serialize parse-error response")?;
                write_half
                    .write_all(payload.as_bytes())
                    .await
                    .context("failed to write parse-error response")?;
                write_half.write_all(b"\n").await?;
                continue;
            }
        };

        if let Some(response) = router
            .handle_request(client_id, request, &shutdown_tx)
            .await
        {
            let payload = serde_json::to_string(&response)
                .context("failed to serialize JSON-RPC response")?;
            write_half
                .write_all(payload.as_bytes())
                .await
                .context("failed to write JSON-RPC response")?;
            write_half
                .write_all(b"\n")
                .await
                .context("failed to write response delimiter")?;
            write_half
                .flush()
                .await
                .context("failed to flush response")?;
        }

        if *shutdown_tx.borrow() {
            return Ok(());
        }
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

fn spawn_background(socket_override: Option<&str>, systemd_activation: bool) -> Result<u32> {
    let exe = std::env::current_exe().context("failed to resolve current executable")?;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("mcp-hub").arg("serve").arg("--foreground");
    if let Some(socket_path) = socket_override {
        cmd.arg("--socket").arg(socket_path);
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

#[cfg(test)]
mod tests {
    use anyhow::Result;
    use serde_json::json;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    use super::send_control_request;

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
}
