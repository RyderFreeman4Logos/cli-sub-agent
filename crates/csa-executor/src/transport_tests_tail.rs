#[test]
fn test_transport_factory_create_routes_tools_to_expected_transport() {
    let legacy_tools = vec![
        Executor::Opencode {
            model_override: None,
            agent: None,
            thinking_budget: None,
        },
        Executor::GeminiCli {
            model_override: None,
            thinking_budget: None,
        },
    ];
    for executor in legacy_tools {
        let transport = TransportFactory::create(&executor, None).expect("transport should build");
        assert!(
            transport.as_ref().as_any().is::<LegacyTransport>(),
            "Expected LegacyTransport for {}",
            executor.tool_name()
        );
    }

    let acp_tools = vec![
        Executor::ClaudeCode {
            model_override: None,
            thinking_budget: None,
        },
        Executor::Codex {
            model_override: None,
            thinking_budget: None,
            runtime_metadata: crate::codex_runtime::codex_runtime_metadata(),
        },
    ];
    for executor in acp_tools {
        let transport = TransportFactory::create(&executor, Some(SessionConfig::default()))
            .expect("transport should build");
        assert!(
            transport.as_ref().as_any().is::<AcpTransport>(),
            "Expected AcpTransport for {}",
            executor.tool_name()
        );
    }
}

#[test]
fn test_transport_factory_create_preserves_session_config_for_acp_transport() {
    let executor = Executor::ClaudeCode {
        model_override: None,
        thinking_budget: None,
    };
    let session_config = SessionConfig {
        no_load: vec!["skills/foo".to_string()],
        extra_load: vec!["skills/bar".to_string()],
        tier: Some("tier-2".to_string()),
        models: vec!["codex/openai/o3/medium".to_string()],
        mcp_servers: Vec::new(),
        mcp_proxy_socket: None,
    };

    let transport = TransportFactory::create(&executor, Some(session_config.clone()))
        .expect("transport should build");
    let acp = transport
        .as_ref()
        .as_any()
        .downcast_ref::<AcpTransport>()
        .expect("expected AcpTransport");

    assert_eq!(acp.session_config, Some(session_config));
}

#[test]
fn test_transport_factory_create_honors_codex_cli_runtime_transport_override() {
    let cli_executor = Executor::Codex {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: crate::codex_runtime::CodexRuntimeMetadata::from_transport(
            crate::codex_runtime::CodexTransport::Cli,
        ),
    };
    let cli_transport = TransportFactory::create(&cli_executor, Some(SessionConfig::default()))
        .expect("transport should build");
    assert!(
        cli_transport.as_ref().as_any().is::<LegacyTransport>(),
        "codex cli override should use LegacyTransport"
    );
}

#[test]
fn test_transport_factory_create_honors_codex_acp_runtime_transport_override() {
    let acp_executor = Executor::Codex {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: crate::codex_runtime::CodexRuntimeMetadata::from_transport(
            crate::codex_runtime::CodexTransport::Acp,
        ),
    };
    let acp_transport = TransportFactory::create(&acp_executor, Some(SessionConfig::default()))
        .expect("transport should build");
    assert!(
        acp_transport.as_ref().as_any().is::<AcpTransport>(),
        "codex acp override should use AcpTransport"
    );
}

#[test]
fn test_legacy_transport_construction_from_executor() {
    let executor = Executor::Opencode {
        model_override: Some("model".to_string()),
        agent: Some("coder".to_string()),
        thinking_budget: None,
    };
    let transport = LegacyTransport::new(executor.clone());

    assert_eq!(transport.executor.tool_name(), executor.tool_name());
    assert_eq!(
        transport.executor.executable_name(),
        executor.executable_name()
    );
}

#[test]
fn test_acp_command_for_tool_mappings() {
    assert_eq!(
        AcpTransport::acp_command_for_tool("claude-code"),
        ("claude-code-acp".to_string(), vec![])
    );
    assert_eq!(
        AcpTransport::acp_command_for_tool("codex"),
        ("codex-acp".to_string(), vec![])
    );
    // If ACP transport is constructed explicitly, gemini-cli uses native ACP mode.
    assert_eq!(
        AcpTransport::acp_command_for_tool("gemini-cli"),
        ("gemini".to_string(), vec!["--acp".to_string()])
    );
    // Unknown tools get "{name}-acp" convention
    assert_eq!(
        AcpTransport::acp_command_for_tool("opencode"),
        ("opencode-acp".to_string(), vec![])
    );
}

// NOTE: CSA_SUPPRESS_NOTIFY is injected by the pipeline layer (not transport)
// based on per-tool config via extra_env. See pipeline.rs suppress_notify logic.
#[test]
fn test_acp_build_env_propagates_extra_env() {
    let transport = AcpTransport::new("claude-code", None);
    let now = chrono::Utc::now();
    let session = csa_session::state::MetaSessionState {
        meta_session_id: "01HTEST000000000000000000".to_string(),
        description: Some("test".to_string()),
        project_path: "/tmp/test".to_string(),
        branch: None,
        created_at: now,
        last_accessed: now,
        csa_version: None,
        genealogy: csa_session::state::Genealogy {
            parent_session_id: None,
            depth: 0,
            ..Default::default()
        },
        tools: HashMap::new(),
        context_status: csa_session::state::ContextStatus::default(),
        total_token_usage: None,
        phase: csa_session::state::SessionPhase::Active,
        task_context: csa_session::state::TaskContext::default(),
        turn_count: 0,
        token_budget: None,
        sandbox_info: None,

        termination_reason: None,
        is_seed_candidate: false,
        git_head_at_creation: None,
        pre_session_porcelain: None,
        last_return_packet: None,
        change_id: None,
        spec_id: None,
        fork_call_timestamps: Vec::new(),
        vcs_identity: None,
        identity_version: 1,
    };

    let mut extra = HashMap::new();
    extra.insert("CSA_SUPPRESS_NOTIFY".to_string(), "1".to_string());
    let env = transport.build_env(&session, Some(&extra));
    assert_eq!(
        env.get("CSA_SUPPRESS_NOTIFY"),
        Some(&"1".to_string()),
        "ACP transport should propagate CSA_SUPPRESS_NOTIFY from extra_env"
    );

    // Without extra_env, suppress_notify should NOT be present.
    let env_no_extra = transport.build_env(&session, None);
    assert_eq!(
        env_no_extra.get("CSA_SUPPRESS_NOTIFY"),
        None,
        "ACP transport should not inject CSA_SUPPRESS_NOTIFY on its own"
    );
}

#[test]
fn test_acp_build_env_includes_csa_session_dir() {
    let transport = AcpTransport::new("claude-code", None);
    let now = chrono::Utc::now();
    let session = csa_session::state::MetaSessionState {
        meta_session_id: "01HTEST000000000000000000".to_string(),
        description: Some("test".to_string()),
        project_path: "/tmp/test".to_string(),
        branch: None,
        created_at: now,
        last_accessed: now,
        csa_version: None,
        genealogy: csa_session::state::Genealogy {
            parent_session_id: None,
            depth: 0,
            ..Default::default()
        },
        tools: HashMap::new(),
        context_status: csa_session::state::ContextStatus::default(),
        total_token_usage: None,
        phase: csa_session::state::SessionPhase::Active,
        task_context: csa_session::state::TaskContext::default(),
        turn_count: 0,
        token_budget: None,
        sandbox_info: None,

        termination_reason: None,
        is_seed_candidate: false,
        git_head_at_creation: None,
        pre_session_porcelain: None,
        last_return_packet: None,
        change_id: None,
        spec_id: None,
        fork_call_timestamps: Vec::new(),
        vcs_identity: None,
        identity_version: 1,
    };

    let env = transport.build_env(&session, None);
    let session_dir = env
        .get("CSA_SESSION_DIR")
        .expect("CSA_SESSION_DIR should be present in env");
    assert!(
        session_dir.contains("/sessions/"),
        "CSA_SESSION_DIR should contain /sessions/ path segment, got: {session_dir}"
    );
    assert!(
        session_dir.contains("01HTEST000000000000000000"),
        "CSA_SESSION_DIR should contain the session ID, got: {session_dir}"
    );
    let result_contract_path = env
        .get("CSA_RESULT_TOML_PATH_CONTRACT")
        .expect("CSA_RESULT_TOML_PATH_CONTRACT should be present in env");
    assert!(
        result_contract_path.ends_with("/output/result.toml"),
        "contract path should point to output/result.toml, got: {result_contract_path}"
    );
    assert!(
        result_contract_path.contains("01HTEST000000000000000000"),
        "contract path should include the session ID, got: {result_contract_path}"
    );
}

#[test]
fn test_acp_build_env_reserved_session_paths_override_extra_env() {
    let transport = AcpTransport::new("claude-code", None);
    let now = chrono::Utc::now();
    let session = csa_session::state::MetaSessionState {
        meta_session_id: "01HTEST000000000000000000".to_string(),
        description: Some("test".to_string()),
        project_path: "/tmp/test".to_string(),
        branch: None,
        created_at: now,
        last_accessed: now,
        csa_version: None,
        genealogy: csa_session::state::Genealogy {
            parent_session_id: None,
            depth: 0,
            ..Default::default()
        },
        tools: HashMap::new(),
        context_status: csa_session::state::ContextStatus::default(),
        total_token_usage: None,
        phase: csa_session::state::SessionPhase::Active,
        task_context: csa_session::state::TaskContext::default(),
        turn_count: 0,
        token_budget: None,
        sandbox_info: None,
        termination_reason: None,
        is_seed_candidate: false,
        git_head_at_creation: None,
        pre_session_porcelain: None,
        last_return_packet: None,
        change_id: None,
        spec_id: None,
        fork_call_timestamps: Vec::new(),
        vcs_identity: None,
        identity_version: 1,
    };

    let mut extra = HashMap::new();
    extra.insert(
        "CSA_SESSION_DIR".to_string(),
        "/tmp/fake-session".to_string(),
    );
    extra.insert(
        csa_session::RESULT_TOML_PATH_CONTRACT_ENV.to_string(),
        "/tmp/fake-session/result.toml".to_string(),
    );

    let env = transport.build_env(&session, Some(&extra));
    let session_dir = env
        .get("CSA_SESSION_DIR")
        .expect("CSA_SESSION_DIR should be present");
    assert!(
        session_dir.contains("/sessions/"),
        "reserved session dir should override extra_env, got: {session_dir}"
    );
    assert!(
        session_dir.contains("01HTEST000000000000000000"),
        "reserved session dir should include the session ID, got: {session_dir}"
    );

    let result_contract_path = env
        .get("CSA_RESULT_TOML_PATH_CONTRACT")
        .expect("CSA_RESULT_TOML_PATH_CONTRACT should be present");
    assert!(
        result_contract_path.ends_with("/output/result.toml"),
        "reserved result contract path should override extra_env, got: {result_contract_path}"
    );
    assert!(
        result_contract_path.contains("01HTEST000000000000000000"),
        "reserved result contract path should include the session ID, got: {result_contract_path}"
    );
}

#[test]
fn test_resume_session_id_extraction() {
    let now = chrono::Utc::now();
    let tool_state = ToolState {
        provider_session_id: Some("test-session-123".to_string()),
        last_action_summary: String::new(),
        last_exit_code: 0,
        updated_at: now,
        tool_version: None,
        token_usage: None,
    };
    let resume_id = tool_state.provider_session_id.as_deref();
    assert_eq!(resume_id, Some("test-session-123"));
}

#[test]
fn test_resume_session_id_none_when_absent() {
    let now = chrono::Utc::now();
    let tool_state = ToolState {
        provider_session_id: None,
        last_action_summary: String::new(),
        last_exit_code: 0,
        updated_at: now,
        tool_version: None,
        token_usage: None,
    };
    let resume_id = tool_state.provider_session_id.as_deref();
    assert!(resume_id.is_none());
}

#[test]
fn test_gemini_retry_model_sequence_matches_policy() {
    // Phase 1: original model + OAuth
    assert_eq!(gemini_retry_model(1), None, "phase 1 keeps original model");
    // Phase 2: original model + API key (no model switch)
    assert_eq!(
        gemini_retry_model(2),
        None,
        "phase 2 keeps original model (auth changes instead)"
    );
    // Phase 3: flash model + API key
    assert_eq!(
        gemini_retry_model(3),
        Some("gemini-3-flash-preview"),
        "phase 3 downgrades to flash model"
    );
    assert_eq!(gemini_retry_model(4), None);
}

#[test]
fn test_executor_for_attempt_overrides_gemini_retry_models() {
    let transport = LegacyTransport::new(Executor::GeminiCli {
        model_override: Some("default".to_string()),
        thinking_budget: None,
    });

    let first = transport.executor_for_attempt(1);
    let second = transport.executor_for_attempt(2);
    let third = transport.executor_for_attempt(3);

    // Phase 1: keep original model
    match first {
        Executor::GeminiCli { model_override, .. } => {
            assert_eq!(model_override.as_deref(), Some("default"));
        }
        _ => panic!("expected GeminiCli executor"),
    }
    // Phase 2: still original model (auth changes, not model)
    match second {
        Executor::GeminiCli { model_override, .. } => {
            assert_eq!(model_override.as_deref(), Some("default"));
        }
        _ => panic!("expected GeminiCli executor"),
    }
    // Phase 3: flash model
    match third {
        Executor::GeminiCli { model_override, .. } => {
            assert_eq!(model_override.as_deref(), Some("gemini-3-flash-preview"));
        }
        _ => panic!("expected GeminiCli executor"),
    }
}

fn build_test_meta_session(project_path: &str) -> MetaSessionState {
    let now = chrono::Utc::now();
    MetaSessionState {
        meta_session_id: "01HTEST000000000000000001".to_string(),
        description: Some("retry-loop-test".to_string()),
        project_path: project_path.to_string(),
        branch: None,
        created_at: now,
        last_accessed: now,
        csa_version: None,
        genealogy: csa_session::state::Genealogy {
            parent_session_id: None,
            depth: 0,
            ..Default::default()
        },
        tools: HashMap::new(),
        context_status: csa_session::state::ContextStatus::default(),
        total_token_usage: None,
        phase: csa_session::state::SessionPhase::Active,
        task_context: csa_session::state::TaskContext::default(),
        turn_count: 0,
        token_budget: None,
        sandbox_info: None,
        termination_reason: None,
        is_seed_candidate: false,
        git_head_at_creation: None,
        pre_session_porcelain: None,
        last_return_packet: None,
        change_id: None,
        spec_id: None,
        fork_call_timestamps: Vec::new(),
        vcs_identity: None,
        identity_version: 1,
    }
}

fn setup_fake_gemini_environment(
    success_on: u32,
) -> (
    tempfile::TempDir,
    HashMap<String, String>,
    std::path::PathBuf,
) {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().expect("tempdir");
    let script_path = temp.path().join("gemini");
    let state_file = temp.path().join("attempts.txt");
    let model_log = temp.path().join("models.log");
    let auth_log = temp.path().join("auth.log");
    std::fs::write(
        &script_path,
        r#"#!/usr/bin/env bash
set -euo pipefail
STATE_FILE="${CSA_FAKE_GEMINI_STATE_FILE:?}"
MODEL_LOG_FILE="${CSA_FAKE_GEMINI_MODEL_LOG_FILE:?}"
AUTH_LOG_FILE="${CSA_FAKE_GEMINI_AUTH_LOG_FILE:?}"
SUCCESS_ON="${CSA_FAKE_GEMINI_SUCCESS_ON:-999}"
FAILURE_REASON="${CSA_FAKE_GEMINI_FAILURE_REASON:-QUOTA_EXHAUSTED}"

count=0
if [ -f "${STATE_FILE}" ]; then
  count="$(cat "${STATE_FILE}")"
fi
count="$((count + 1))"
printf '%s\n' "${count}" >"${STATE_FILE}"

model="inherit"
while [ "$#" -gt 0 ]; do
  if [ "$1" = "-m" ]; then
    shift
    if [ "$#" -gt 0 ]; then
      model="$1"
    fi
    break
  fi
  shift
done
printf '%s\n' "${model}" >>"${MODEL_LOG_FILE}"

# Log auth mode: if GEMINI_API_KEY is set, auth is api_key; otherwise oauth
if [ -n "${GEMINI_API_KEY:-}" ]; then
  printf 'api_key\n' >>"${AUTH_LOG_FILE}"
else
  printf 'oauth\n' >>"${AUTH_LOG_FILE}"
fi

if [ "${count}" -lt "${SUCCESS_ON}" ]; then
  printf "reason: '%s' (attempt=%s)\n" "${FAILURE_REASON}" "${count}" >&2
  exit 1
fi

printf 'ok attempt=%s model=%s\n' "${count}" "${model}"
"#,
    )
    .expect("write fake gemini");
    let mut perms = std::fs::metadata(&script_path)
        .expect("metadata")
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&script_path, perms).expect("chmod +x");

    let old_path = std::env::var("PATH").unwrap_or_default();
    let mut env = HashMap::new();
    env.insert(
        "PATH".to_string(),
        format!("{}:{old_path}", temp.path().display()),
    );
    env.insert(
        "CSA_FAKE_GEMINI_STATE_FILE".to_string(),
        state_file.display().to_string(),
    );
    env.insert(
        "CSA_FAKE_GEMINI_MODEL_LOG_FILE".to_string(),
        model_log.display().to_string(),
    );
    env.insert(
        "CSA_FAKE_GEMINI_AUTH_LOG_FILE".to_string(),
        auth_log.display().to_string(),
    );
    env.insert(
        "CSA_FAKE_GEMINI_SUCCESS_ON".to_string(),
        success_on.to_string(),
    );

    (temp, env, model_log)
}

fn read_model_log(path: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .expect("read model log")
        .lines()
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty())
        .collect()
}

fn auth_log_path(model_log: &std::path::Path) -> std::path::PathBuf {
    model_log.with_file_name("auth.log")
}

fn read_auth_log(model_log: &std::path::Path) -> Vec<String> {
    let path = auth_log_path(model_log);
    std::fs::read_to_string(&path)
        .expect("read auth log")
        .lines()
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty())
        .collect()
}

#[tokio::test]
async fn test_execute_in_retries_until_success_with_expected_model_chain() {
    let (_temp, env, model_log_path) = setup_fake_gemini_environment(3);
    let transport = LegacyTransport::new(Executor::GeminiCli {
        model_override: None,
        thinking_budget: None,
    });

    let result = transport
        .execute_in(
            "test retry loop",
            std::path::Path::new("/tmp"),
            Some(&env),
            StreamMode::BufferOnly,
            30,
            super::ResolvedTimeout(None),
        )
        .await
        .expect("execute_in should succeed on attempt 3");

    assert_eq!(result.execution.exit_code, 0);
    assert!(
        result.execution.output.contains("ok attempt=3"),
        "unexpected output: {}",
        result.execution.output
    );
    let models = read_model_log(&model_log_path);
    // Phase 1: original model (OAuth), Phase 2: original model (API key), Phase 3: flash (API key)
    assert_eq!(
        models,
        vec![
            "inherit".to_string(),
            "inherit".to_string(),
            "gemini-3-flash-preview".to_string()
        ]
    );
}

#[tokio::test]
async fn test_execute_stops_after_max_attempts_and_returns_last_failure() {
    let (temp, env, model_log_path) = setup_fake_gemini_environment(99);
    let transport = LegacyTransport::new(Executor::GeminiCli {
        model_override: None,
        thinking_budget: None,
    });
    let session = build_test_meta_session(temp.path().to_str().expect("utf8 temp path"));
    let options = TransportOptions {
        stream_mode: StreamMode::BufferOnly,
        idle_timeout_seconds: 30,
        acp_crash_max_attempts: 2,
        initial_response_timeout: super::ResolvedTimeout(None),
        liveness_dead_seconds: 30,
        stdin_write_timeout_seconds: 30,
        acp_init_timeout_seconds: 30,
        termination_grace_period_seconds: 1,
        output_spool: None,
        output_spool_max_bytes: csa_process::DEFAULT_SPOOL_MAX_BYTES,
        output_spool_keep_rotated: csa_process::DEFAULT_SPOOL_KEEP_ROTATED,
        setting_sources: None,
        sandbox: None,
    };

    let result = transport
        .execute("test retry loop", None, &session, Some(&env), options)
        .await
        .expect("execute should return final failed attempt result");

    assert_ne!(result.execution.exit_code, 0);
    assert!(
        result.execution.stderr_output.contains("QUOTA_EXHAUSTED"),
        "unexpected stderr: {}",
        result.execution.stderr_output
    );
    let models = read_model_log(&model_log_path);
    // Phase 1: original model (OAuth), Phase 2: original model (API key), Phase 3: flash (API key)
    assert_eq!(
        models,
        vec![
            "inherit".to_string(),
            "inherit".to_string(),
            "gemini-3-flash-preview".to_string()
        ],
        "retry loop should stop after 3 attempts"
    );
}

include!("transport_tests_tail_retry.rs");
