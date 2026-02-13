use super::*;

// --- parse_tool_name tests ---

#[test]
fn mcp_parse_tool_name_all_valid_tools() {
    assert!(matches!(
        parse_tool_name("gemini-cli").unwrap(),
        ToolName::GeminiCli
    ));
    assert!(matches!(
        parse_tool_name("opencode").unwrap(),
        ToolName::Opencode
    ));
    assert!(matches!(parse_tool_name("codex").unwrap(), ToolName::Codex));
    assert!(matches!(
        parse_tool_name("claude-code").unwrap(),
        ToolName::ClaudeCode
    ));
}

#[test]
fn mcp_parse_tool_name_unknown_errors() {
    assert!(parse_tool_name("vim").is_err());
}

#[test]
fn mcp_parse_tool_name_empty_errors() {
    assert!(parse_tool_name("").is_err());
}

// --- get_tools tests ---

#[test]
fn get_tools_returns_expected_tool_count() {
    let tools = get_tools();
    assert_eq!(
        tools.len(),
        4,
        "Expected 4 MCP tools (session_list, session_delete, gc, run)"
    );
}

#[test]
fn get_tools_all_have_names_and_schemas() {
    for tool in get_tools() {
        assert!(!tool.name.is_empty(), "Tool name must not be empty");
        assert!(
            !tool.description.is_empty(),
            "Tool description must not be empty: {}",
            tool.name
        );
        assert!(
            tool.input_schema.is_object(),
            "Input schema must be an object: {}",
            tool.name
        );
    }
}

#[test]
fn get_tools_csa_run_requires_prompt() {
    let tools = get_tools();
    let run_tool = tools.iter().find(|t| t.name == "csa_run").unwrap();
    let required = run_tool.input_schema.get("required").unwrap();
    let required_arr = required.as_array().unwrap();
    assert!(
        required_arr.iter().any(|v| v.as_str() == Some("prompt")),
        "csa_run must require 'prompt' parameter"
    );
}

#[test]
fn get_tools_csa_session_delete_requires_session_id() {
    let tools = get_tools();
    let del_tool = tools
        .iter()
        .find(|t| t.name == "csa_session_delete")
        .unwrap();
    let required = del_tool.input_schema.get("required").unwrap();
    let required_arr = required.as_array().unwrap();
    assert!(
        required_arr
            .iter()
            .any(|v| v.as_str() == Some("session_id")),
        "csa_session_delete must require 'session_id' parameter"
    );
}

// --- JSON-RPC protocol structure tests ---

#[test]
fn jsonrpc_request_parses_valid_initialize() {
    let json = r#"{"jsonrpc":"2.0","method":"initialize","id":1}"#;
    let req: JsonRpcRequest = serde_json::from_str(json).unwrap();
    assert_eq!(req.method, "initialize");
    assert_eq!(req.id, Some(serde_json::json!(1)));
    assert!(req.params.is_none());
}

#[test]
fn jsonrpc_request_parses_with_params() {
    let json = r#"{"jsonrpc":"2.0","method":"tools/call","params":{"name":"csa_gc","arguments":{}},"id":2}"#;
    let req: JsonRpcRequest = serde_json::from_str(json).unwrap();
    assert_eq!(req.method, "tools/call");
    assert!(req.params.is_some());
    let params = req.params.unwrap();
    assert_eq!(params.get("name").unwrap().as_str().unwrap(), "csa_gc");
}

#[test]
fn jsonrpc_request_parses_notification_without_id() {
    let json = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
    let req: JsonRpcRequest = serde_json::from_str(json).unwrap();
    assert_eq!(req.method, "notifications/initialized");
    assert!(req.id.is_none());
}

#[test]
fn jsonrpc_response_serializes_with_result() {
    let response = JsonRpcResponse {
        jsonrpc: "2.0".to_string(),
        result: Some(serde_json::json!({"status": "ok"})),
        error: None,
        id: Some(serde_json::json!(1)),
    };
    let json_str = serde_json::to_string(&response).unwrap();
    assert!(json_str.contains("\"result\""));
    assert!(!json_str.contains("\"error\""));
}

#[test]
fn jsonrpc_response_serializes_with_error() {
    let response = JsonRpcResponse {
        jsonrpc: "2.0".to_string(),
        result: None,
        error: Some(JsonRpcError {
            code: -32600,
            message: "Invalid Request".to_string(),
        }),
        id: Some(serde_json::json!(1)),
    };
    let json_str = serde_json::to_string(&response).unwrap();
    assert!(!json_str.contains("\"result\""));
    assert!(json_str.contains("\"error\""));
    assert!(json_str.contains("-32600"));
}

#[test]
fn jsonrpc_response_omits_null_fields() {
    let response = JsonRpcResponse {
        jsonrpc: "2.0".to_string(),
        result: None,
        error: None,
        id: None,
    };
    let json_str = serde_json::to_string(&response).unwrap();
    // Neither result nor error should appear thanks to skip_serializing_if
    assert!(!json_str.contains("\"result\""));
    assert!(!json_str.contains("\"error\""));
}

#[test]
fn jsonrpc_request_invalid_json_fails() {
    let json = "not valid json {{{";
    let result = serde_json::from_str::<JsonRpcRequest>(json);
    assert!(result.is_err());
}
