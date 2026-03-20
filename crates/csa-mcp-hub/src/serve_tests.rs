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
        let request: serde_json::Value = serde_json::from_str(line.trim()).expect("parse request");
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
    let (skill_notify_tx, _skill_notify_rx) = tokio::sync::mpsc::channel(1);
    let skill_notify_tx = super::SkillRefreshNotifier::new(skill_notify_tx);

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
    let _ = tokio::time::timeout(Duration::from_secs(2), reader.read_line(&mut response)).await??;

    tokio::time::timeout(Duration::from_secs(2), server_task).await???;
    Ok(())
}

/// Helper to send a control-plane request with params via a Unix stream pair
/// and return the parsed JSON-RPC response.
async fn control_plane_round_trip(
    router: Arc<ProxyRouter>,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value> {
    let (client, server) = tokio::net::UnixStream::pair()?;
    let (shutdown_tx, _shutdown_rx) = tokio::sync::watch::channel(false);
    let policy = super::ConnectionPolicy {
        max_requests_per_sec: 100,
        max_request_body_bytes: 10 * 1024 * 1024,
        request_timeout: Duration::from_secs(5),
        current_uid: super::current_uid(),
    };
    let (skill_notify_tx, _skill_notify_rx) = tokio::sync::mpsc::channel(1);
    let skill_notify_tx = super::SkillRefreshNotifier::new(skill_notify_tx);

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
        "method": method,
        "params": params,
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
    let mut response_line = String::new();
    tokio::time::timeout(Duration::from_secs(2), reader.read_line(&mut response_line)).await??;

    tokio::time::timeout(Duration::from_secs(2), server_task).await???;

    let response: serde_json::Value = serde_json::from_str(response_line.trim())?;
    Ok(response)
}

#[tokio::test]
async fn hub_search_tools_returns_empty_on_empty_cache() -> Result<()> {
    let router = Arc::new(ProxyRouter::new(
        Arc::new(McpRegistry::new(Vec::new())),
        Duration::from_secs(5),
    ));

    let response =
        control_plane_round_trip(router, "hub/search-tools", json!({"query": "foo"})).await?;

    let tools = response["result"]["tools"]
        .as_array()
        .expect("tools should be array");
    assert!(tools.is_empty());
    Ok(())
}

#[tokio::test]
async fn hub_search_tools_finds_cached_tool() -> Result<()> {
    use crate::proxy::ToolDescriptor;

    let registry = Arc::new(McpRegistry::new(Vec::new()));
    let router = Arc::new(ProxyRouter::new(registry, Duration::from_secs(5)));

    // Manually populate the cache for testing without a real MCP server
    {
        let mut cache = router.tool_cache.write().await;
        cache.insert(
            "my_search_tool".to_string(),
            ToolDescriptor {
                server_name: "test-server".to_string(),
                description: Some("searches things".to_string()),
                input_schema: json!({"type": "object"}),
            },
        );
        cache.insert(
            "other_tool".to_string(),
            ToolDescriptor {
                server_name: "test-server".to_string(),
                description: Some("does other things".to_string()),
                input_schema: json!({"type": "object"}),
            },
        );
    }

    let response = control_plane_round_trip(
        router,
        "hub/search-tools",
        json!({"query": "search", "limit": 10}),
    )
    .await?;

    let tools = response["result"]["tools"]
        .as_array()
        .expect("tools should be array");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0]["name"], "my_search_tool");
    assert_eq!(tools[0]["server_name"], "test-server");
    Ok(())
}

#[tokio::test]
async fn hub_get_tool_schema_returns_schema() -> Result<()> {
    use crate::proxy::ToolDescriptor;

    let registry = Arc::new(McpRegistry::new(Vec::new()));
    let router = Arc::new(ProxyRouter::new(registry, Duration::from_secs(5)));

    {
        let mut cache = router.tool_cache.write().await;
        cache.insert(
            "schema_tool".to_string(),
            ToolDescriptor {
                server_name: "test-server".to_string(),
                description: Some("a tool with schema".to_string()),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "query": {"type": "string"}
                    }
                }),
            },
        );
    }

    let response = control_plane_round_trip(
        router,
        "hub/get-tool-schema",
        json!({"tool_name": "schema_tool"}),
    )
    .await?;

    let result = &response["result"];
    assert_eq!(result["tool_name"], "schema_tool");
    assert_eq!(result["server_name"], "test-server");
    assert_eq!(result["description"], "a tool with schema");
    assert_eq!(
        result["input_schema"],
        json!({"type": "object", "properties": {"query": {"type": "string"}}})
    );
    Ok(())
}

#[tokio::test]
async fn hub_get_tool_schema_not_found() -> Result<()> {
    let router = Arc::new(ProxyRouter::new(
        Arc::new(McpRegistry::new(Vec::new())),
        Duration::from_secs(5),
    ));

    let response = control_plane_round_trip(
        router,
        "hub/get-tool-schema",
        json!({"tool_name": "nonexistent_tool"}),
    )
    .await?;

    assert!(response.get("error").is_some());
    assert_eq!(response["error"]["code"], -32601);
    Ok(())
}

#[tokio::test]
async fn hub_get_tool_schema_missing_param() -> Result<()> {
    let router = Arc::new(ProxyRouter::new(
        Arc::new(McpRegistry::new(Vec::new())),
        Duration::from_secs(5),
    ));

    let response = control_plane_round_trip(router, "hub/get-tool-schema", json!({})).await?;

    assert!(response.get("error").is_some());
    assert_eq!(response["error"]["code"], -32602);
    Ok(())
}
