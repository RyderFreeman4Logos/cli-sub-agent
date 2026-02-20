use serde_json::json;

use super::AcpTransport;
use csa_acp::{McpServerConfig, SessionConfig};
use std::collections::HashMap;

#[test]
fn test_build_session_meta_with_empty_sources() {
    let sources: Vec<String> = vec![];
    let meta = AcpTransport::build_session_meta(Some(&sources), None).expect("meta should exist");
    assert_eq!(
        serde_json::Value::Object(meta),
        json!({"claudeCode": {"options": {"settingSources": []}}})
    );
}

#[test]
fn test_build_session_meta_with_project_source() {
    let sources = vec!["project".to_string()];
    let meta = AcpTransport::build_session_meta(Some(&sources), None).expect("meta should exist");
    assert_eq!(
        serde_json::Value::Object(meta),
        json!({"claudeCode": {"options": {"settingSources": ["project"]}}})
    );
}

#[test]
fn test_build_session_meta_none_returns_none() {
    assert!(
        AcpTransport::build_session_meta(None, None).is_none(),
        "meta should be absent when setting_sources is None"
    );
}

#[test]
fn test_build_session_meta_includes_direct_mcp_servers() {
    let session_config = SessionConfig {
        mcp_servers: vec![McpServerConfig {
            name: "memory".to_string(),
            command: "npx".to_string(),
            args: vec!["-y".to_string(), "@anthropic/claude-mem-mcp".to_string()],
            env: HashMap::new(),
        }],
        ..Default::default()
    };

    let meta = AcpTransport::build_session_meta(None, Some(&session_config))
        .expect("meta should include mcpServers");
    assert_eq!(
        serde_json::Value::Object(meta),
        json!({"claudeCode": {"options": {"mcpServers": {
            "memory": {
                "command": "npx",
                "args": ["-y", "@anthropic/claude-mem-mcp"],
                "env": {}
            }
        }}}})
    );
}

#[test]
fn test_build_session_meta_prefers_proxy_when_socket_exists() {
    let temp = tempfile::tempdir().expect("tempdir");
    let socket_path = temp.path().join("mcp-hub.sock");
    std::fs::write(&socket_path, "").expect("create fake socket marker");

    let session_config = SessionConfig {
        mcp_proxy_socket: Some(socket_path.to_string_lossy().into_owned()),
        mcp_servers: vec![McpServerConfig {
            name: "memory".to_string(),
            command: "npx".to_string(),
            args: vec!["-y".to_string(), "@anthropic/claude-mem-mcp".to_string()],
            env: HashMap::new(),
        }],
        ..Default::default()
    };

    let meta = AcpTransport::build_session_meta(None, Some(&session_config))
        .expect("meta should include proxy mcpServers");
    let mcp_servers = &serde_json::Value::Object(meta)["claudeCode"]["options"]["mcpServers"];
    assert!(mcp_servers.get("csa-mcp-hub").is_some());
    assert!(mcp_servers.get("memory").is_none());
}
