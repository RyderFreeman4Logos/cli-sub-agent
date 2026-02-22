use anyhow::Result;
use csa_config::{McpServerConfig, McpTransport};
use rmcp::model::CallToolRequestParam;
use serde_json::json;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

use super::{LeaseTracker, McpRegistry, PoolKey, StatefulServerPool, ToolCallRoute};

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

fn stateless_config(script_path: &std::path::Path) -> McpServerConfig {
    McpServerConfig {
        name: "mock".to_string(),
        transport: McpTransport::Stdio {
            command: "sh".to_string(),
            args: vec![script_path.to_string_lossy().into_owned()],
            env: HashMap::new(),
        },
        stateful: false,
        memory_max_mb: None,
    }
}

fn stateful_config(script_path: &std::path::Path) -> McpServerConfig {
    McpServerConfig {
        name: "stateful".to_string(),
        transport: McpTransport::Stdio {
            command: "sh".to_string(),
            args: vec![script_path.to_string_lossy().into_owned()],
            env: HashMap::new(),
        },
        stateful: true,
        memory_max_mb: None,
    }
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

    let registry = McpRegistry::new(vec![stateless_config(&script_path)]);

    let tools = registry
        .list_tools("mock", CancellationToken::new())
        .await?;
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
            ToolCallRoute::default(),
            CancellationToken::new(),
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
    let stamp = temp.path().join("first-list.stamp");
    let script_path = write_script(
        temp.path(),
        &format!(
            r#"#!/bin/sh
stamp="{}"
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
      if [ ! -f "$stamp" ]; then
        touch "$stamp"
        exit 1
      fi
      ;;
  esac
done
"#,
            stamp.to_string_lossy()
        ),
    )?;

    let registry = McpRegistry::new(vec![McpServerConfig {
        name: "flaky".to_string(),
        transport: McpTransport::Stdio {
            command: "sh".to_string(),
            args: vec![script_path.to_string_lossy().into_owned()],
            env: HashMap::new(),
        },
        stateful: false,
        memory_max_mb: None,
    }]);

    let first = registry
        .list_tools("flaky", CancellationToken::new())
        .await?;
    assert_eq!(first[0].name.as_ref(), "echo_tool");

    let second = registry
        .list_tools("flaky", CancellationToken::new())
        .await?;
    assert_eq!(second[0].name.as_ref(), "echo_tool");

    registry.shutdown_all().await?;
    Ok(())
}

#[tokio::test]
async fn queue_cancellation_does_not_wait_for_head_of_line() -> Result<()> {
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
    *\"tools/call\"*)
      sleep 1
      printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"pong"}]}}\n' "$id"
      ;;
    *\"tools/list\"*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"echo_tool","description":"echo","inputSchema":{"type":"object","properties":{}}}]}}\n' "$id"
      ;;
  esac
done
"#,
    )?;

    let registry = Arc::new(McpRegistry::new(vec![stateless_config(&script_path)]));

    let registry_first = registry.clone();
    let first = tokio::spawn(async move {
        registry_first
            .call_tool(
                "mock",
                CallToolRequestParam {
                    name: "echo_tool".into(),
                    arguments: Some(json!({}).as_object().cloned().unwrap_or_default()),
                },
                ToolCallRoute::default(),
                CancellationToken::new(),
            )
            .await
    });

    tokio::time::sleep(Duration::from_millis(50)).await;

    let registry_second = registry.clone();
    let cancellation = CancellationToken::new();
    let cancellation_clone = cancellation.clone();
    let second = tokio::spawn(async move {
        registry_second
            .call_tool(
                "mock",
                CallToolRequestParam {
                    name: "echo_tool".into(),
                    arguments: Some(json!({}).as_object().cloned().unwrap_or_default()),
                },
                ToolCallRoute::default(),
                cancellation_clone,
            )
            .await
    });

    tokio::time::sleep(Duration::from_millis(80)).await;
    cancellation.cancel();

    let cancelled_result = timeout(Duration::from_millis(300), second).await;
    assert!(
        cancelled_result.is_ok(),
        "cancelled request should return quickly"
    );

    let cancelled_result = cancelled_result
        .expect("join timeout")
        .expect("join failed")
        .expect_err("second queued request should be cancelled");
    assert!(
        cancelled_result.to_string().contains("cancelled"),
        "unexpected cancellation error: {cancelled_result}"
    );

    let first_result = timeout(Duration::from_secs(3), first).await;
    assert!(first_result.is_ok(), "first queued request should complete");
    let first_call = first_result.expect("timeout").expect("join failed")?;
    assert_eq!(
        first_call.content[0].as_text().map(|t| t.text.as_str()),
        Some("pong")
    );

    registry.shutdown_all().await?;
    Ok(())
}

#[test]
fn lease_tracker_acquire_release_and_expire() {
    let warm_ttl = Duration::from_secs(600);
    let mut tracker = LeaseTracker::new(warm_ttl);
    let start = Instant::now();
    let key = PoolKey {
        project_root: PathBuf::from("/workspace/app"),
        toolchain_hash: 7,
    };

    tracker.acquire(&key, start);
    assert_eq!(tracker.active_leases(&key), 1);

    tracker.release(&key, start + Duration::from_secs(1));
    assert_eq!(tracker.active_leases(&key), 0);

    let expired = tracker.expire(start + warm_ttl + Duration::from_secs(1));
    assert_eq!(expired, vec![key]);
}

#[test]
fn lease_tracker_reclaims_idle_pools_under_pressure() {
    let mut tracker = LeaseTracker::new(Duration::from_secs(600));
    let start = Instant::now();

    let key_a = PoolKey {
        project_root: PathBuf::from("/workspace/a"),
        toolchain_hash: 1,
    };
    let key_b = PoolKey {
        project_root: PathBuf::from("/workspace/b"),
        toolchain_hash: 1,
    };
    let protected = PoolKey {
        project_root: PathBuf::from("/workspace/c"),
        toolchain_hash: 1,
    };

    tracker.acquire(&key_a, start);
    tracker.release(&key_a, start + Duration::from_secs(1));

    tracker.acquire(&key_b, start);
    tracker.release(&key_b, start + Duration::from_secs(2));

    tracker.acquire(&protected, start);
    tracker.release(&protected, start + Duration::from_secs(3));

    let reclaimed = tracker.reclaim_for_pressure(3, 2, &protected);

    assert_eq!(reclaimed.len(), 1);
    assert_eq!(reclaimed[0], key_a);
    assert!(!reclaimed.contains(&protected));
}

#[tokio::test]
async fn stateful_server_requires_project_root_route() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let script_path = write_script(
        temp.path(),
        r#"#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s\n' "$line" | sed -n 's/.*"id"[ ]*:[ ]*\([^,}]*\).*/\1/p')
  case "$line" in
    *\"initialize\"*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"stateful","version":"0.1.0"}}}\n' "$id"
      ;;
    *\"notifications/initialized\"*)
      ;;
    *\"tools/list\"*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"echo_tool","description":"echo","inputSchema":{"type":"object","properties":{}}}]}}\n' "$id"
      ;;
    *\"tools/call\"*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"pong"}]}}\n' "$id"
      ;;
  esac
done
"#,
    )?;

    let registry = McpRegistry::new(vec![McpServerConfig {
        name: "stateful".to_string(),
        transport: McpTransport::Stdio {
            command: "sh".to_string(),
            args: vec![script_path.to_string_lossy().into_owned()],
            env: HashMap::new(),
        },
        stateful: true,
        memory_max_mb: None,
    }]);

    let result = registry
        .call_tool(
            "stateful",
            CallToolRequestParam {
                name: "echo_tool".into(),
                arguments: Some(json!({}).as_object().cloned().unwrap_or_default()),
            },
            ToolCallRoute::default(),
            CancellationToken::new(),
        )
        .await;

    assert!(result.is_err());
    assert!(
        result
            .expect_err("stateful call should require project_root")
            .to_string()
            .contains("project_root")
    );

    registry.shutdown_all().await?;
    Ok(())
}

#[tokio::test]
async fn test_stateful_pool_request_after_expiry_succeeds() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let script_path = write_script(
        temp.path(),
        r#"#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s\n' "$line" | sed -n 's/.*"id"[ ]*:[ ]*\([^,}]*\).*/\1/p')
  case "$line" in
    *\"initialize\"*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"stateful","version":"0.1.0"}}}\n' "$id"
      ;;
    *\"notifications/initialized\"*)
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

    let pool = StatefulServerPool::new(stateful_config(&script_path));
    {
        let mut inner = pool.inner.lock().await;
        inner.leases.warm_ttl = Duration::ZERO;
    }

    let route = ToolCallRoute {
        project_root: Some(PathBuf::from("/workspace/app")),
        toolchain_hash: Some(7),
    };
    let request = CallToolRequestParam {
        name: "echo_tool".into(),
        arguments: Some(json!({}).as_object().cloned().unwrap_or_default()),
    };

    let first = pool
        .call_tool(request.clone(), route.clone(), CancellationToken::new())
        .await?;
    assert_eq!(
        first.content[0].as_text().map(|t| t.text.as_str()),
        Some("pong")
    );

    let second = pool
        .call_tool(request, route, CancellationToken::new())
        .await?;
    assert_eq!(
        second.content[0].as_text().map(|t| t.text.as_str()),
        Some("pong")
    );

    pool.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn test_stateful_pool_max_active_limit() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let script_path = write_script(
        temp.path(),
        r#"#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s\n' "$line" | sed -n 's/.*"id"[ ]*:[ ]*\([^,}]*\).*/\1/p')
  case "$line" in
    *\"initialize\"*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"stateful","version":"0.1.0"}}}\n' "$id"
      ;;
    *\"notifications/initialized\"*)
      ;;
    *\"tools/call\"*)
      sleep 1
      printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"pong"}]}}\n' "$id"
      ;;
    *)
      printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id"
      ;;
  esac
done
"#,
    )?;

    let mut pool = StatefulServerPool::new(stateful_config(&script_path));
    pool.max_active_pools = 1;
    let pool = Arc::new(pool);

    let request = CallToolRequestParam {
        name: "echo_tool".into(),
        arguments: Some(json!({}).as_object().cloned().unwrap_or_default()),
    };

    let first_pool = pool.clone();
    let first_request = request.clone();
    let first = tokio::spawn(async move {
        first_pool
            .call_tool(
                first_request,
                ToolCallRoute {
                    project_root: Some(PathBuf::from("/workspace/a")),
                    toolchain_hash: Some(1),
                },
                CancellationToken::new(),
            )
            .await
    });

    tokio::time::sleep(Duration::from_millis(100)).await;

    let second = pool
        .call_tool(
            request,
            ToolCallRoute {
                project_root: Some(PathBuf::from("/workspace/b")),
                toolchain_hash: Some(1),
            },
            CancellationToken::new(),
        )
        .await;
    let second_error = second.expect_err("new key should fail after max_active_pools is reached");
    assert!(
        second_error.to_string().contains("max_active_pools"),
        "unexpected error: {second_error}"
    );

    let first_result = first.await.expect("join failed")?;
    assert_eq!(
        first_result.content[0].as_text().map(|t| t.text.as_str()),
        Some("pong")
    );

    pool.shutdown().await?;
    Ok(())
}

// ── Transport label tests ───────────────────────────────────────────

#[tokio::test]
async fn registry_tracks_transport_labels() {
    let stdio_config = McpServerConfig {
        name: "local-mcp".to_string(),
        transport: McpTransport::Stdio {
            command: "false".to_string(),
            args: vec![],
            env: HashMap::new(),
        },
        stateful: false,
        memory_max_mb: None,
    };
    let http_config = McpServerConfig {
        name: "remote-mcp".to_string(),
        transport: McpTransport::Http {
            url: "https://mcp.example.com/v1".to_string(),
            headers: HashMap::new(),
            allow_insecure: false,
        },
        stateful: false,
        memory_max_mb: None,
    };
    let sse_config = McpServerConfig {
        name: "sse-mcp".to_string(),
        transport: McpTransport::Sse {
            url: "https://mcp.example.com/sse".to_string(),
            headers: HashMap::new(),
            allow_insecure: false,
        },
        stateful: false,
        memory_max_mb: None,
    };

    let registry = McpRegistry::new(vec![stdio_config, http_config, sse_config]);

    assert_eq!(registry.transport_label("local-mcp"), "stdio");
    assert_eq!(registry.transport_label("remote-mcp"), "http");
    assert_eq!(registry.transport_label("sse-mcp"), "sse");
    assert_eq!(registry.transport_label("nonexistent"), "stdio"); // default fallback
}

// ── HTTP URL safety tests ───────────────────────────────────────────

#[cfg(feature = "transport-http-client")]
mod http_safety {
    use super::super::{is_ssrf_dangerous_ip, parse_host_port, validate_http_url};
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    #[test]
    fn validate_https_url_passes() {
        assert!(validate_http_url("https://mcp.example.com/v1", false, "test").is_ok());
    }

    #[test]
    fn validate_http_url_rejected_by_default() {
        let err = validate_http_url("http://mcp.example.com/v1", false, "test").unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("HTTPS"), "expected HTTPS enforcement: {msg}");
    }

    #[test]
    fn validate_http_url_allowed_when_insecure() {
        assert!(validate_http_url("http://mcp.example.com/v1", true, "test").is_ok());
    }

    #[test]
    fn validate_file_scheme_rejected() {
        let err = validate_http_url("file:///etc/passwd", false, "test").unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("unsupported URL scheme"), "{msg}");
    }

    #[test]
    fn validate_data_scheme_rejected() {
        // data: URLs have no "://" so they are caught by the no-scheme check
        let err = validate_http_url("data:text/plain,hello", false, "test").unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("no scheme"), "{msg}");
    }

    #[test]
    fn validate_gopher_scheme_rejected() {
        let err = validate_http_url("gopher://evil.com/1", false, "test").unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("unsupported URL scheme"), "{msg}");
    }

    #[test]
    fn validate_no_scheme_rejected() {
        let err = validate_http_url("mcp.example.com/v1", false, "test").unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("no scheme"), "{msg}");
    }

    #[test]
    fn ssrf_loopback_v4_blocked() {
        assert!(is_ssrf_dangerous_ip(IpAddr::V4(Ipv4Addr::LOCALHOST)));
    }

    #[test]
    fn ssrf_loopback_v6_blocked() {
        assert!(is_ssrf_dangerous_ip(IpAddr::V6(Ipv6Addr::LOCALHOST)));
    }

    #[test]
    fn ssrf_private_10_blocked() {
        assert!(is_ssrf_dangerous_ip(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))));
    }

    #[test]
    fn ssrf_private_172_blocked() {
        assert!(is_ssrf_dangerous_ip(IpAddr::V4(Ipv4Addr::new(
            172, 16, 0, 1
        ))));
    }

    #[test]
    fn ssrf_private_192_blocked() {
        assert!(is_ssrf_dangerous_ip(IpAddr::V4(Ipv4Addr::new(
            192, 168, 1, 1
        ))));
    }

    #[test]
    fn ssrf_metadata_ip_blocked() {
        assert!(is_ssrf_dangerous_ip(IpAddr::V4(Ipv4Addr::new(
            169, 254, 169, 254
        ))));
    }

    #[test]
    fn ssrf_public_ip_allowed() {
        assert!(!is_ssrf_dangerous_ip(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))));
    }

    #[test]
    fn ssrf_ipv4_mapped_v6_loopback_blocked() {
        // ::ffff:127.0.0.1
        let v6 = Ipv6Addr::new(0, 0, 0, 0, 0, 0xffff, 0x7f00, 0x0001);
        assert!(is_ssrf_dangerous_ip(IpAddr::V6(v6)));
    }

    #[test]
    fn parse_host_port_basic() {
        assert_eq!(
            parse_host_port("https://mcp.example.com:9090/v1"),
            Some(("mcp.example.com".to_string(), 9090))
        );
    }

    #[test]
    fn parse_host_port_default_https() {
        assert_eq!(
            parse_host_port("https://mcp.example.com/v1"),
            Some(("mcp.example.com".to_string(), 443))
        );
    }

    #[test]
    fn parse_host_port_default_http() {
        assert_eq!(
            parse_host_port("http://mcp.example.com/v1"),
            Some(("mcp.example.com".to_string(), 80))
        );
    }

    #[test]
    fn parse_host_port_ipv6() {
        assert_eq!(
            parse_host_port("https://[::1]:8080/v1"),
            Some(("[::1]".to_string(), 8080))
        );
    }
}
