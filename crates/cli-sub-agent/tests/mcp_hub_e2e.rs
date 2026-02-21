use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde_json::Value;

fn write_mock_mcp_script(dir: &Path) -> Result<PathBuf> {
    let script_path = dir.join("mock-mcp.sh");
    fs::write(
        &script_path,
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

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&script_path)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script_path, perms)?;
    }

    Ok(script_path)
}

fn write_global_config(config_home: &Path, script_path: &Path) -> Result<()> {
    let cfg_dir = config_home.join("cli-sub-agent");
    fs::create_dir_all(&cfg_dir)?;
    let cfg_path = cfg_dir.join("config.toml");
    fs::write(
        cfg_path,
        format!(
            r#"[[mcp.servers]]
name = "echo"
command = "sh"
args = ["{}"]
"#,
            script_path.display()
        ),
    )?;
    Ok(())
}

fn wait_for_socket(socket_path: &Path, timeout: Duration) -> Result<()> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if socket_path.exists() {
            if std::os::unix::net::UnixStream::connect(socket_path).is_ok() {
                return Ok(());
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    bail!("timed out waiting for socket {}", socket_path.display())
}

fn connect_and_request(socket_path: &Path, request: &Value) -> Result<Value> {
    let mut stream = std::os::unix::net::UnixStream::connect(socket_path)
        .with_context(|| format!("connect hub socket {}", socket_path.display()))?;
    let payload = serde_json::to_string(request)?;
    writeln!(stream, "{payload}")?;

    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    if line.trim().is_empty() {
        bail!("empty response from hub")
    }
    serde_json::from_str(line.trim()).context("parse hub response")
}

fn open_direct_client(script_path: &Path) -> Result<(Child, ChildStdin, BufReader<ChildStdout>)> {
    let mut child = Command::new("sh")
        .arg(script_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("spawn direct mock MCP: {}", script_path.display()))?;

    let stdin = child
        .stdin
        .take()
        .context("capture direct mock MCP stdin")?;
    let stdout = child
        .stdout
        .take()
        .context("capture direct mock MCP stdout")?;
    Ok((child, stdin, BufReader::new(stdout)))
}

fn direct_request(
    stdin: &mut ChildStdin,
    stdout: &mut BufReader<ChildStdout>,
    request: &Value,
) -> Result<Value> {
    let payload = serde_json::to_string(request)?;
    writeln!(stdin, "{payload}")?;
    stdin.flush()?;

    let mut line = String::new();
    stdout.read_line(&mut line)?;
    if line.trim().is_empty() {
        bail!("empty response from direct MCP")
    }
    serde_json::from_str(line.trim()).context("parse direct MCP response")
}

fn p95_ms(samples: &[Duration]) -> f64 {
    let mut sorted = samples
        .iter()
        .map(|d| d.as_secs_f64() * 1000.0)
        .collect::<Vec<_>>();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let idx = ((sorted.len() as f64) * 0.95).ceil() as usize;
    let idx = idx.saturating_sub(1).min(sorted.len().saturating_sub(1));
    sorted[idx]
}

fn reserve_local_port() -> Result<u16> {
    let listener =
        std::net::TcpListener::bind(("127.0.0.1", 0)).context("bind localhost ephemeral port")?;
    let port = listener.local_addr()?.port();
    drop(listener);
    Ok(port)
}

fn start_sse_stream(base_url: &str) -> Result<(Child, std::sync::mpsc::Receiver<String>)> {
    let mut child = Command::new("curl")
        .args(["-sS", "-N", base_url])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("spawn curl SSE stream: {base_url}"))?;

    let stdout = child.stdout.take().context("capture curl SSE stdout")?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        loop {
            line.clear();
            let Ok(bytes) = reader.read_line(&mut line) else {
                break;
            };
            if bytes == 0 {
                break;
            }
            let _ = tx.send(line.trim().to_string());
        }
    });

    Ok((child, rx))
}

fn recv_line_until<F>(
    rx: &std::sync::mpsc::Receiver<String>,
    timeout: Duration,
    predicate: F,
) -> Result<String>
where
    F: Fn(&str) -> bool,
{
    let start = Instant::now();
    while start.elapsed() < timeout {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(line) if predicate(&line) => return Ok(line),
            Ok(_) => continue,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(error) => bail!("failed to read SSE stream line: {error}"),
        }
    }
    bail!("timed out waiting for expected SSE line")
}

fn http_post_json(url: &str, payload: &Value) -> Result<()> {
    let payload_str = serde_json::to_string(payload)?;
    let output = Command::new("curl")
        .args([
            "-sS",
            "-o",
            "/dev/null",
            "-w",
            "%{http_code}",
            "-X",
            "POST",
            "-H",
            "content-type: application/json",
            "--data",
            &payload_str,
            url,
        ])
        .output()
        .with_context(|| format!("POST {url}"))?;

    if !output.status.success() {
        bail!(
            "curl POST failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let code = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if code != "202" {
        bail!("expected HTTP 202 from SSE message endpoint, got {code}");
    }
    Ok(())
}

// macOS CI: hub spawns the mock MCP backend but sh/sed differences prevent
// the backend from registering tools.  Hub itself is Linux-first (UDS + systemd),
// so restrict the E2E test to Linux.  Unit tests still cover logic on all platforms.
#[test]
#[cfg_attr(not(target_os = "linux"), ignore)]
fn hub_forwards_requests_and_proxy_latency_budget_is_within_5ms() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let home = temp.path().join("home");
    let config_home = home.join(".config");
    let runtime_dir = temp.path().join("runtime");
    fs::create_dir_all(&config_home)?;
    fs::create_dir_all(&runtime_dir)?;

    let script_path = write_mock_mcp_script(temp.path())?;
    write_global_config(&config_home, &script_path)?;

    let socket_path = runtime_dir.join("mcp-hub.sock");

    let mut hub = Command::new(env!("CARGO_BIN_EXE_csa"))
        .args([
            "mcp-hub",
            "serve",
            "--foreground",
            "--socket",
            socket_path
                .to_str()
                .context("socket path should be valid UTF-8")?,
        ])
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", &config_home)
        .env("XDG_RUNTIME_DIR", &runtime_dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("spawn hub")?;

    let test_result = (|| -> Result<()> {
        wait_for_socket(&socket_path, Duration::from_secs(5))?;

        // Retry tools/list until the hub has connected its MCP backend.
        // On macOS CI runners the hub socket is ready before backends register.
        let mut list_response = Value::Null;
        for attempt in 0..20 {
            std::thread::sleep(Duration::from_millis(250));
            list_response = connect_and_request(
                &socket_path,
                &serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
            )?;
            if list_response["result"]["tools"][0]["name"] == "echo_tool" {
                break;
            }
            if attempt == 19 {
                bail!(
                    "hub never registered MCP backend after 5s; last response: {}",
                    list_response
                );
            }
        }
        assert_eq!(list_response["result"]["tools"][0]["name"], "echo_tool");

        let call_response = connect_and_request(
            &socket_path,
            &serde_json::json!({
                "jsonrpc":"2.0",
                "id":2,
                "method":"tools/call",
                "params":{"name":"echo_tool","arguments":{}}
            }),
        )?;
        assert_eq!(call_response["result"]["content"][0]["text"], "pong");

        let rounds = 60usize;

        let (mut direct_child, mut direct_stdin, mut direct_stdout) =
            open_direct_client(&script_path)?;
        let mut direct_samples = Vec::with_capacity(rounds);
        for i in 0..rounds {
            let request = serde_json::json!({
                "jsonrpc":"2.0",
                "id": i,
                "method":"tools/call",
                "params":{"name":"echo_tool","arguments":{}}
            });
            let started = Instant::now();
            let _response = direct_request(&mut direct_stdin, &mut direct_stdout, &request)?;
            direct_samples.push(started.elapsed());
        }
        let _ = direct_child.kill();
        let _ = direct_child.wait();

        let mut proxy_samples = Vec::with_capacity(rounds);
        for i in 0..rounds {
            let request = serde_json::json!({
                "jsonrpc":"2.0",
                "id": i,
                "method":"tools/call",
                "params":{"name":"echo_tool","arguments":{}}
            });
            let started = Instant::now();
            let _response = connect_and_request(&socket_path, &request)?;
            proxy_samples.push(started.elapsed());
        }

        let direct_p95 = p95_ms(&direct_samples);
        let proxy_p95 = p95_ms(&proxy_samples);
        let overhead = proxy_p95 - direct_p95;
        eprintln!(
            "mcp_hub_latency_ms direct_p95={direct_p95:.3} proxy_p95={proxy_p95:.3} overhead={overhead:.3}"
        );

        assert!(
            overhead <= 5.0,
            "proxy p95 overhead must be <= 5ms, got overhead={overhead:.3}ms (direct={direct_p95:.3}ms, proxy={proxy_p95:.3}ms)"
        );

        Ok(())
    })();

    let _ = connect_and_request(
        &socket_path,
        &serde_json::json!({"jsonrpc":"2.0","id":9999,"method":"hub/stop"}),
    );
    let _ = hub.kill();
    let _ = hub.wait();

    test_result
}

#[test]
#[cfg_attr(not(target_os = "linux"), ignore)]
fn hub_http_sse_transport_forwards_requests() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let home = temp.path().join("home");
    let config_home = home.join(".config");
    let runtime_dir = temp.path().join("runtime");
    fs::create_dir_all(&config_home)?;
    fs::create_dir_all(&runtime_dir)?;

    let script_path = write_mock_mcp_script(temp.path())?;
    write_global_config(&config_home, &script_path)?;

    let socket_path = runtime_dir.join("mcp-hub.sock");
    let http_port = reserve_local_port()?;

    let mut hub = Command::new(env!("CARGO_BIN_EXE_csa"))
        .args([
            "mcp-hub",
            "serve",
            "--foreground",
            "--socket",
            socket_path
                .to_str()
                .context("socket path should be valid UTF-8")?,
            "--http-bind",
            "127.0.0.1",
            "--http-port",
            &http_port.to_string(),
        ])
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", &config_home)
        .env("XDG_RUNTIME_DIR", &runtime_dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("spawn hub")?;

    let test_result = (|| -> Result<()> {
        wait_for_socket(&socket_path, Duration::from_secs(5))?;

        let sse_url = format!("http://127.0.0.1:{http_port}/");
        let base_url = format!("http://127.0.0.1:{http_port}");
        let mut endpoint_line = None;
        let mut sse_child = None;
        let mut sse_rx = None;
        for _ in 0..20 {
            let (mut child, rx) = start_sse_stream(&sse_url)?;
            match recv_line_until(&rx, Duration::from_secs(1), |line| {
                line.starts_with("data: /message?sessionId=")
            }) {
                Ok(line) => {
                    endpoint_line = Some(line);
                    sse_rx = Some(rx);
                    sse_child = Some(child);
                    break;
                }
                Err(_) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        }
        let endpoint_line = endpoint_line.context("timed out waiting for SSE endpoint line")?;
        let mut sse_child = sse_child.context("missing SSE process")?;
        let rx = sse_rx.context("missing SSE output channel")?;

        let post_path = endpoint_line
            .strip_prefix("data: ")
            .context("missing SSE endpoint data prefix")?;
        let post_url = format!("{base_url}{post_path}");

        http_post_json(
            &post_url,
            &serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
        )?;
        let tools_line = recv_line_until(&rx, Duration::from_secs(5), |line| {
            line.contains("\"echo_tool\"")
        })?;
        assert!(
            tools_line.contains("\"echo_tool\""),
            "unexpected tools/list SSE payload: {tools_line}"
        );

        http_post_json(
            &post_url,
            &serde_json::json!({
                "jsonrpc":"2.0",
                "id":2,
                "method":"tools/call",
                "params":{"name":"echo_tool","arguments":{}}
            }),
        )?;
        let call_line = recv_line_until(&rx, Duration::from_secs(5), |line| line.contains("pong"))?;
        assert!(
            call_line.contains("pong"),
            "unexpected tools/call SSE payload: {call_line}"
        );

        let _ = sse_child.kill();
        let _ = sse_child.wait();
        Ok(())
    })();

    let _ = connect_and_request(
        &socket_path,
        &serde_json::json!({"jsonrpc":"2.0","id":9999,"method":"hub/stop"}),
    );
    let _ = hub.kill();
    let _ = hub.wait();

    test_result
}
