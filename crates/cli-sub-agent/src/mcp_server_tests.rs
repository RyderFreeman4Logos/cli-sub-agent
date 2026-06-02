use super::*;
use std::path::{Path, PathBuf};
use tempfile::tempdir;

struct EnvVarGuard {
    key: &'static str,
    original: Option<String>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let original = std::env::var(key).ok();
        // SAFETY: test-scoped env mutation guarded by TEST_ENV_LOCK.
        unsafe { std::env::set_var(key, value) };
        Self { key, original }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        // SAFETY: test-scoped env mutation guarded by TEST_ENV_LOCK.
        unsafe {
            match self.original.as_deref() {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }
}

struct CurrentDirGuard {
    original: PathBuf,
}

impl CurrentDirGuard {
    fn set(path: &Path) -> Self {
        let original = std::env::current_dir().expect("current dir");
        std::env::set_current_dir(path).expect("set current dir");
        Self { original }
    }
}

impl Drop for CurrentDirGuard {
    fn drop(&mut self) {
        std::env::set_current_dir(&self.original).expect("restore current dir");
    }
}

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
fn get_tools_csa_run_documents_tier_bypass_gate() {
    let tools = get_tools();
    let run_tool = tools.iter().find(|t| t.name == "csa_run").unwrap();
    let properties = run_tool
        .input_schema
        .get("properties")
        .and_then(|v| v.as_object())
        .expect("csa_run properties");

    let model_spec_description = properties
        .get("model_spec")
        .and_then(|v| v.get("description"))
        .and_then(|v| v.as_str())
        .expect("model_spec description");
    assert!(model_spec_description.contains("[tier_policy].allow_force_bypass"));

    let force_description = properties
        .get("force_ignore_tier_setting")
        .and_then(|v| v.get("description"))
        .and_then(|v| v.as_str())
        .expect("force_ignore_tier_setting description");
    assert!(force_description.contains("[tier_policy].allow_force_bypass"));
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

#[tokio::test]
async fn mcp_run_rejects_model_spec_when_project_tiers_exist_and_policy_is_default() {
    let _guard = crate::test_env_lock::TEST_ENV_LOCK.lock().await;
    let project = tempdir().expect("project tempdir");
    let config_dir = project.path().join(".csa");
    std::fs::create_dir_all(&config_dir).expect("create project config dir");
    std::fs::write(
        config_dir.join("config.toml"),
        r#"
[tiers.quality]
description = "quality"
models = ["codex/openai/gpt-5/high"]
"#,
    )
    .expect("write project config");

    let xdg = tempdir().expect("xdg tempdir");
    let _xdg_guard = EnvVarGuard::set("XDG_CONFIG_HOME", xdg.path());
    let _cwd_guard = CurrentDirGuard::set(project.path());

    let err = handle_run_tool(
        serde_json::json!({
            "prompt": "hello",
            "model_spec": "codex/openai/gpt-5/high"
        }),
        &crate::startup_env::EMPTY_STARTUP_SUBTREE_ENV,
    )
    .await
    .expect_err("MCP exact model bypass should be rejected before execution");
    let message = err.to_string();
    assert!(message.contains("Tier bypass is disabled"));
    assert!(message.contains("--model-spec"));
    assert!(message.contains("[tier_policy].allow_force_bypass"));
}

#[tokio::test]
async fn mcp_run_uses_server_startup_depth_for_recursion_guard() {
    let _guard = crate::test_env_lock::TEST_ENV_LOCK.lock().await;
    let project = tempdir().expect("project tempdir");
    let config_dir = project.path().join(".csa");
    std::fs::create_dir_all(&config_dir).expect("create project config dir");
    std::fs::write(
        config_dir.join("config.toml"),
        r#"
schema_version = 1

[project]
max_recursion_depth = 1
"#,
    )
    .expect("write project config");

    let xdg = tempdir().expect("xdg tempdir");
    let _xdg_guard = EnvVarGuard::set("XDG_CONFIG_HOME", xdg.path());
    let _cwd_guard = CurrentDirGuard::set(project.path());
    let startup_env = crate::startup_env::StartupSubtreeEnv::from_values(
        std::collections::HashMap::from([(csa_core::env::CSA_DEPTH_ENV_KEY, "2".to_string())]),
    );

    let response = handle_run_tool(
        serde_json::json!({
            "prompt": "hello"
        }),
        &startup_env,
    )
    .await
    .expect("recursion guard returns an MCP content response");

    let text = response
        .get("content")
        .and_then(|content| content.get(0))
        .and_then(|entry| entry.get("text"))
        .and_then(|text| text.as_str())
        .expect("response text");
    assert!(text.contains("Max recursion depth (1) exceeded. Current: 2"));
}

#[test]
fn direct_entry_resolved_timeout_preserves_pipeline_semantics() {
    assert_eq!(direct_entry_resolved_timeout(None), ResolvedTimeout(None));
    assert_eq!(
        direct_entry_resolved_timeout(Some(90)),
        ResolvedTimeout(Some(90))
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
