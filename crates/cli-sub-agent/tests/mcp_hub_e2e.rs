use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde_json::Value;

// TODO(manual-e2e): Run a live Haiku end-to-end validation with real API credentials.
// This suite intentionally uses local mock MCP backends and does not hit external APIs.

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
        if socket_path.exists() && std::os::unix::net::UnixStream::connect(socket_path).is_ok() {
            return Ok(());
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

// macOS CI: hub spawns the mock MCP backend but sh/sed differences prevent
// the backend from registering tools.  Hub itself is Linux-first (UDS + systemd),
// so restrict the E2E test to Linux.  Unit tests still cover logic on all platforms.
#[test]
#[cfg_attr(not(target_os = "linux"), ignore)]
fn hub_forwards_requests_and_proxy_latency_budget_is_within_environment_budget() -> Result<()> {
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
                bail!("hub never registered MCP backend after 5s; last response: {list_response}");
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
        let max_allowed_overhead = (direct_p95 * 3.0).clamp(5.0, 15.0);
        eprintln!(
            "mcp_hub_latency_ms direct_p95={direct_p95:.3} proxy_p95={proxy_p95:.3} overhead={overhead:.3}"
        );

        assert!(
            overhead <= max_allowed_overhead,
            "proxy p95 overhead must be <= min(max(direct_p95*3, 5ms), 15ms)={max_allowed_overhead:.3}ms, got overhead={overhead:.3}ms (direct={direct_p95:.3}ms, proxy={proxy_p95:.3}ms)"
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

/// Post a JSON-RPC request to the Streamable HTTP endpoint.
/// Returns (response_json, session_id_if_present).
/// Handles both plain JSON and SSE (`data:` prefixed) response formats.
fn http_mcp_post(
    url: &str,
    payload: &Value,
    session_id: Option<&str>,
) -> Result<(Value, Option<String>)> {
    let payload_str = serde_json::to_string(payload)?;
    let header_file = tempfile::NamedTempFile::new().context("create header temp file")?;
    let header_path = header_file.path().to_str().context("header file path")?;

    let mut cmd = Command::new("curl");
    cmd.args([
        "-sS",
        "-D",
        header_path,
        "-X",
        "POST",
        "-H",
        "content-type: application/json",
        "-H",
        "accept: application/json, text/event-stream",
    ]);
    if let Some(sid) = session_id {
        let hdr = format!("mcp-session-id: {sid}");
        cmd.args(["-H", &hdr]);
    }
    cmd.args(["--data", &payload_str, url]);

    let output = cmd.output().with_context(|| format!("POST {url}"))?;
    if !output.status.success() {
        bail!(
            "curl POST failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    // Extract Mcp-Session-Id from dumped headers.
    let headers = fs::read_to_string(header_file.path()).unwrap_or_default();
    let sid = headers.lines().find_map(|line| {
        let lower = line.to_lowercase();
        if lower.starts_with("mcp-session-id:") {
            Some(line.split_once(':')?.1.trim().to_string())
        } else {
            None
        }
    });

    let body = String::from_utf8_lossy(&output.stdout);

    // Try plain JSON first, then extract from SSE `data:` lines.
    if let Ok(val) = serde_json::from_str::<Value>(body.trim()) {
        return Ok((val, sid));
    }
    for line in body.lines() {
        if let Some(data) = line.strip_prefix("data: ")
            && let Ok(val) = serde_json::from_str::<Value>(data)
        {
            return Ok((val, sid));
        }
    }
    bail!("no JSON-RPC response found in body: {body}")
}

#[test]
#[cfg_attr(not(target_os = "linux"), ignore)]
fn hub_http_streamable_transport_forwards_requests() -> Result<()> {
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

        let mcp_url = format!("http://127.0.0.1:{http_port}/mcp");

        // Streamable HTTP requires MCP initialization handshake first.
        // Streamable HTTP requires MCP initialization handshake first.
        let mut last_err = String::new();
        let mut initialized = false;
        for attempt in 0..20 {
            std::thread::sleep(Duration::from_millis(250));
            match http_mcp_post(
                &mcp_url,
                &serde_json::json!({
                    "jsonrpc":"2.0",
                    "id":0,
                    "method":"initialize",
                    "params":{
                        "protocolVersion":"2024-11-05",
                        "capabilities":{},
                        "clientInfo":{"name":"test","version":"0.1.0"}
                    }
                }),
                None,
            ) {
                Ok((resp, _)) if resp["result"]["serverInfo"].is_object() => {
                    initialized = true;
                    break;
                }
                Ok((resp, _)) => {
                    last_err = format!("unexpected init response: {resp}");
                }
                Err(error) if attempt < 19 => {
                    last_err = error.to_string();
                    continue;
                }
                Err(error) => bail!("initialize failed: {error}"),
            }
            if attempt == 19 {
                bail!("hub never initialized after 5s; last error: {last_err}");
            }
        }
        assert!(initialized, "MCP initialization failed: {last_err}");

        // Wait for backend MCP to register via Streamable HTTP POST.
        let mut tools_response = Value::Null;
        for attempt in 0..20 {
            std::thread::sleep(Duration::from_millis(250));
            match http_mcp_post(
                &mcp_url,
                &serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
                None,
            ) {
                Ok((resp, _)) if resp["result"]["tools"][0]["name"] == "echo_tool" => {
                    tools_response = resp;
                    break;
                }
                Ok((resp, _)) => {
                    tools_response = resp;
                }
                Err(_) if attempt < 19 => continue,
                Err(error) => bail!("tools/list failed: {error}"),
            }
            if attempt == 19 {
                bail!("hub never registered MCP backend after 5s; last response: {tools_response}");
            }
        }
        assert_eq!(tools_response["result"]["tools"][0]["name"], "echo_tool");

        let (call_response, _) = http_mcp_post(
            &mcp_url,
            &serde_json::json!({
                "jsonrpc":"2.0",
                "id":2,
                "method":"tools/call",
                "params":{"name":"echo_tool","arguments":{}}
            }),
            None,
        )?;
        assert_eq!(call_response["result"]["content"][0]["text"], "pong");

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
