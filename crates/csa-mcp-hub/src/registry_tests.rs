use anyhow::Result;
use csa_config::McpServerConfig;
use rmcp::model::CallToolRequestParam;
use serde_json::json;
use std::collections::HashMap;
use std::fs;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

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

fn config(script_path: &std::path::Path, name: &str) -> McpServerConfig {
    McpServerConfig {
        name: name.to_string(),
        command: "sh".to_string(),
        args: vec![script_path.to_string_lossy().into_owned()],
        env: HashMap::new(),
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

    let registry = McpRegistry::new(vec![config(&script_path, "mock")]);

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

    let registry = McpRegistry::new(vec![config(&script_path, "flaky")]);

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

    let registry = Arc::new(McpRegistry::new(vec![config(&script_path, "mock")]));

    let registry_first = registry.clone();
    let first = tokio::spawn(async move {
        registry_first
            .call_tool(
                "mock",
                CallToolRequestParam {
                    name: "echo_tool".into(),
                    arguments: Some(json!({}).as_object().cloned().unwrap_or_default()),
                },
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
