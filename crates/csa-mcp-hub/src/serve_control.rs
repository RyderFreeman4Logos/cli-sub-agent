//! CLI control-plane helpers for the MCP hub server.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::config::{HubConfig, default_socket_path};
use crate::skill_writer::regenerate_routing_skill_once;
use crate::socket;

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
    super::run_hub(cfg, systemd_activation).await
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

pub(super) async fn send_control_request(socket_path: &Path, method: &str) -> Result<Value> {
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
