use super::*;
use csa_session::{
    SessionPhase, SessionResult, ToolState, create_session, get_session_dir, get_session_root,
    save_result, save_session,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tempfile::tempdir;

#[cfg(target_os = "linux")]
fn read_process_start_time_ticks(pid: u32) -> u64 {
    let stat_path = format!("/proc/{pid}/stat");
    let content = std::fs::read_to_string(stat_path).expect("read /proc stat");
    let close_paren = content.rfind(')').expect("stat comm terminator");
    let after_comm = &content[close_paren + 1..];
    let mut parts = after_comm.split_whitespace();
    parts.next().expect("state");
    parts.next().expect("ppid");
    parts.next().expect("pgrp");
    for _ in 0..16 {
        parts.next().expect("intermediate stat field");
    }
    parts
        .next()
        .expect("starttime")
        .parse::<u64>()
        .expect("starttime parse")
}

#[cfg(target_os = "linux")]
fn daemon_pid_record(pid: u32) -> String {
    format!("{pid} {}\n", read_process_start_time_ticks(pid))
}

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

#[path = "mcp_server_model_pin_tests.rs"]
mod model_pin_tests;

fn seed_retired_runtime_session(project_root: &Path) -> (String, PathBuf) {
    std::fs::create_dir_all(project_root).expect("create project root");

    let last_accessed = chrono::Utc::now() - chrono::Duration::days(40);
    let mut session = create_session(
        project_root,
        Some("mcp gc runtime test"),
        None,
        Some("codex"),
    )
    .expect("create session");
    session.phase = SessionPhase::Retired;
    session.last_accessed = last_accessed;
    session.tools.insert(
        "codex".to_string(),
        ToolState {
            provider_session_id: Some("provider-session".to_string()),
            last_action_summary: "completed".to_string(),
            last_exit_code: 0,
            updated_at: last_accessed,
            tool_version: None,
            token_usage: None,
        },
    );
    save_session(&session).expect("save retired session");

    let runtime_dir = get_session_root(project_root)
        .expect("session root")
        .join("sessions")
        .join(&session.meta_session_id)
        .join("runtime");
    std::fs::create_dir_all(&runtime_dir).expect("create runtime dir");
    std::fs::write(runtime_dir.join("cache.bin"), b"runtime").expect("write runtime marker");

    (session.meta_session_id, runtime_dir)
}

fn seed_completed_session(
    project_root: &Path,
    status: &str,
    exit_code: i32,
    summary: &str,
) -> String {
    std::fs::create_dir_all(project_root).expect("create project root");
    let mut session = create_session(project_root, Some("mcp wait test"), None, Some("codex"))
        .expect("create session");
    session.phase = SessionPhase::Retired;
    save_session(&session).expect("save retired session");

    let now = chrono::Utc::now();
    save_result(
        project_root,
        &session.meta_session_id,
        &SessionResult {
            status: status.to_string(),
            exit_code,
            summary: summary.to_string(),
            tool: "codex".to_string(),
            started_at: now,
            completed_at: now,
            ..Default::default()
        },
    )
    .expect("save result");
    session.meta_session_id
}

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

#[test]
fn get_tools_returns_expected_tool_count() {
    let tools = get_tools();
    assert_eq!(tools.len(), 5, "MCP tool count changed");
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

#[test]
fn get_tools_csa_session_wait_requires_session_id() {
    let tools = get_tools();
    let wait_tool = tools.iter().find(|t| t.name == "csa_session_wait").unwrap();
    let required = wait_tool.input_schema.get("required").unwrap();
    let required_arr = required.as_array().unwrap();
    assert!(
        required_arr
            .iter()
            .any(|v| v.as_str() == Some("session_id")),
        "csa_session_wait must require 'session_id' parameter"
    );
}

#[test]
fn mcp_session_wait_default_cap_is_below_advertised_codex_tool_timeout() {
    assert_eq!(DEFAULT_MCP_SESSION_WAIT_TIMEOUT_SECONDS, 6_900);
    const {
        assert!(
            DEFAULT_MCP_SESSION_WAIT_TIMEOUT_SECONDS
                < csa_config::DEFAULT_CODEX_SESSION_WAIT_MCP_TOOL_TIMEOUT_SEC
        );
    }
}

#[tokio::test]
async fn mcp_session_wait_returns_nonzero_session_result_without_mcp_error() {
    let tmp = tempdir().expect("tempdir");
    let _sandbox = crate::test_session_sandbox::ScopedSessionSandbox::new(&tmp).await;
    let project_root = tmp.path().join("project");
    let session_id =
        seed_completed_session(&project_root, "failure", 42, "focused test command failed");
    let _cwd_guard = CurrentDirGuard::set(&project_root);
    let state = McpServerState {
        startup_env: crate::startup_env::EMPTY_STARTUP_SUBTREE_ENV.clone(),
    };

    let response = handle_tool_call(
        Some(serde_json::json!({
            "name": "csa_session_wait",
            "arguments": {
                "session_id": session_id,
                "timeout_seconds": 1
            }
        })),
        &state,
    )
    .await
    .expect("wait failure response");

    let text = response
        .get("content")
        .and_then(|content| content.get(0))
        .and_then(|entry| entry.get("text"))
        .and_then(|text| text.as_str())
        .expect("response text");
    assert!(text.contains("Status: failure"), "{text}");
    assert!(text.contains("Exit code: 42"), "{text}");
    assert!(
        text.contains("Summary: focused test command failed"),
        "{text}"
    );
    assert!(text.contains("MCP csa_session_wait exit_code=1"), "{text}");
}

#[tokio::test]
async fn mcp_session_wait_json_returns_parseable_document_with_wait_exit_code() {
    let tmp = tempdir().expect("tempdir");
    let _sandbox = crate::test_session_sandbox::ScopedSessionSandbox::new(&tmp).await;
    let project_root = tmp.path().join("project");
    let session_id =
        seed_completed_session(&project_root, "failure", 42, "focused test command failed");
    let _cwd_guard = CurrentDirGuard::set(&project_root);
    let state = McpServerState {
        startup_env: crate::startup_env::EMPTY_STARTUP_SUBTREE_ENV.clone(),
    };

    let response = handle_tool_call(
        Some(serde_json::json!({
            "name": "csa_session_wait",
            "arguments": {
                "session_id": session_id,
                "timeout_seconds": 1,
                "json": true
            }
        })),
        &state,
    )
    .await
    .expect("wait JSON response");

    let text = response
        .get("content")
        .and_then(|content| content.get(0))
        .and_then(|entry| entry.get("text"))
        .and_then(|text| text.as_str())
        .expect("response text");
    let parsed: serde_json::Value = serde_json::from_str(text).expect("valid wait JSON");
    assert_eq!(parsed["exit_code"], serde_json::json!(42));
    assert_eq!(parsed["mcp_wait_exit_code"], serde_json::json!(1));
    assert!(!text.contains("MCP csa_session_wait exit_code="), "{text}");
    assert!(!text.contains("CSA:SESSION_WAIT_COMPLETED"), "{text}");
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn mcp_session_wait_alive_at_timeout_returns_rewait_content() {
    let tmp = tempdir().expect("tempdir");
    let _sandbox = crate::test_session_sandbox::ScopedSessionSandbox::new(&tmp).await;
    let project_root = tmp.path().join("project");
    std::fs::create_dir_all(&project_root).expect("create project root");
    let session = create_session(
        &project_root,
        Some("mcp wait alive timeout test"),
        None,
        Some("codex"),
    )
    .expect("create active session");
    let session_id = session.meta_session_id;
    let session_dir = get_session_root(&project_root)
        .expect("session root")
        .join("sessions")
        .join(&session_id);
    let mut child = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .expect("spawn child");
    std::fs::write(
        session_dir.join("daemon.pid"),
        daemon_pid_record(child.id()),
    )
    .expect("write daemon pid");
    assert!(
        csa_process::ToolLiveness::daemon_pid_is_alive(&session_dir),
        "daemon should be live"
    );

    let _cwd_guard = CurrentDirGuard::set(&project_root);
    let state = McpServerState {
        startup_env: crate::startup_env::EMPTY_STARTUP_SUBTREE_ENV.clone(),
    };

    let response = handle_tool_call(
        Some(serde_json::json!({
            "name": "csa_session_wait",
            "arguments": {
                "session_id": session_id,
                "timeout_seconds": 1
            }
        })),
        &state,
    )
    .await
    .expect("wait alive response");

    let _ = child.kill();
    let _ = child.wait();

    let text = response
        .get("content")
        .and_then(|content| content.get(0))
        .and_then(|entry| entry.get("text"))
        .and_then(|text| text.as_str())
        .expect("response text");
    assert!(text.contains("status=alive"), "{text}");
    assert!(text.contains("action=re-wait"), "{text}");
    assert!(text.contains("Call csa_session_wait again"), "{text}");
    assert!(text.contains("MCP csa_session_wait exit_code=0"), "{text}");
    assert!(!text.contains("CSA:SESSION_WAIT_COMPLETED"), "{text}");
}

#[tokio::test]
async fn mcp_gc_reap_runtime_protects_hosting_session_runtime() {
    let tmp = tempdir().expect("tempdir");
    let _sandbox = crate::test_session_sandbox::ScopedSessionSandbox::new(&tmp).await;
    let project_root = tmp.path().join("project");
    let (session_id, runtime_dir) = seed_retired_runtime_session(&project_root);
    let _cwd_guard = CurrentDirGuard::set(&project_root);
    let state = McpServerState {
        startup_env: crate::startup_env::StartupSubtreeEnv::from_values(HashMap::from([(
            csa_core::env::CSA_SESSION_ID_ENV_KEY,
            session_id,
        )])),
    };

    let response = handle_tool_call(
        Some(serde_json::json!({
            "name": "csa_gc",
            "arguments": {
                "reap_runtime": true,
                "max_age_days": 30
            }
        })),
        &state,
    )
    .await
    .expect("MCP csa_gc should succeed");

    let text = response
        .get("content")
        .and_then(|content| content.get(0))
        .and_then(|entry| entry.get("text"))
        .and_then(|text| text.as_str())
        .expect("response text");
    assert!(text.contains("Garbage collection completed"));
    assert!(
        runtime_dir.exists(),
        "MCP csa_gc must preserve the hosting session runtime/"
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
    assert!(!json_str.contains("\"result\""));
    assert!(!json_str.contains("\"error\""));
}

#[test]
fn jsonrpc_request_invalid_json_fails() {
    let json = "not valid json {{{";
    let result = serde_json::from_str::<JsonRpcRequest>(json);
    assert!(result.is_err());
}
