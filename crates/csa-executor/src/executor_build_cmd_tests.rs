//! Tests for build_command, inject_env, effort mapping, and boundary conditions.

use super::*;

/// Helper: create a minimal MetaSessionState for testing.
fn make_test_session() -> MetaSessionState {
    let now = chrono::Utc::now();
    MetaSessionState {
        meta_session_id: "01HTEST000000000000000000".to_string(),
        description: Some("test session".to_string()),
        project_path: "/tmp/test-project".to_string(),
        branch: None,
        created_at: now,
        last_accessed: now,
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
        last_return_packet: None,
        fork_call_timestamps: Vec::new(),
    }
}

// ── build_command: CSA env vars ─────────────────────────────────

#[test]
fn test_build_command_gemini_sets_csa_env_vars() {
    let exec = Executor::GeminiCli {
        model_override: None,
        thinking_budget: None,
    };
    let session = make_test_session();
    let (cmd, stdin_data) = exec.build_command("hello world", None, &session, None);
    assert!(stdin_data.is_none(), "Short prompts should stay on argv");

    let envs: Vec<_> = cmd.as_std().get_envs().collect();
    let env_map: HashMap<&std::ffi::OsStr, Option<&std::ffi::OsStr>> = envs.into_iter().collect();

    assert_eq!(
        env_map.get(std::ffi::OsStr::new("CSA_SESSION_ID")),
        Some(&Some(std::ffi::OsStr::new("01HTEST000000000000000000"))),
        "CSA_SESSION_ID should match session id"
    );
    assert_eq!(
        env_map.get(std::ffi::OsStr::new("CSA_DEPTH")),
        Some(&Some(std::ffi::OsStr::new("1"))),
        "CSA_DEPTH should be parent depth + 1"
    );
    assert_eq!(
        env_map.get(std::ffi::OsStr::new("CSA_PROJECT_ROOT")),
        Some(&Some(std::ffi::OsStr::new("/tmp/test-project"))),
        "CSA_PROJECT_ROOT should match project path"
    );
    assert_eq!(
        env_map.get(std::ffi::OsStr::new("CSA_TOOL")),
        Some(&Some(std::ffi::OsStr::new("gemini-cli"))),
        "CSA_TOOL should match tool name"
    );
}

#[test]
fn test_build_command_sets_csa_session_dir() {
    let exec = Executor::GeminiCli {
        model_override: None,
        thinking_budget: None,
    };
    let session = make_test_session();
    let (cmd, _stdin_data) = exec.build_command("hello", None, &session, None);

    let envs: Vec<_> = cmd.as_std().get_envs().collect();
    let env_map: HashMap<&std::ffi::OsStr, Option<&std::ffi::OsStr>> = envs.into_iter().collect();

    let session_dir = env_map
        .get(std::ffi::OsStr::new("CSA_SESSION_DIR"))
        .expect("CSA_SESSION_DIR should be present")
        .expect("CSA_SESSION_DIR should have a value");
    let session_dir_str = session_dir.to_string_lossy();
    assert!(
        session_dir_str.contains("/sessions/"),
        "CSA_SESSION_DIR should contain /sessions/ path segment, got: {session_dir_str}"
    );
    assert!(
        session_dir_str.contains("01HTEST000000000000000000"),
        "CSA_SESSION_DIR should contain the session ID, got: {session_dir_str}"
    );
}

#[test]
fn test_build_command_codex_sets_csa_env_vars() {
    let exec = Executor::Codex {
        model_override: None,
        thinking_budget: None,
    };
    let session = make_test_session();
    let (cmd, stdin_data) = exec.build_command("test", None, &session, None);
    assert!(stdin_data.is_none(), "Short prompts should stay on argv");

    let envs: Vec<_> = cmd.as_std().get_envs().collect();
    let env_map: HashMap<&std::ffi::OsStr, Option<&std::ffi::OsStr>> = envs.into_iter().collect();

    assert_eq!(
        env_map.get(std::ffi::OsStr::new("CSA_TOOL")),
        Some(&Some(std::ffi::OsStr::new("codex"))),
    );
}

#[test]
fn test_build_command_depth_increments() {
    let exec = Executor::ClaudeCode {
        model_override: None,
        thinking_budget: None,
    };
    let mut session = make_test_session();
    session.genealogy.depth = 3;

    let (cmd, stdin_data) = exec.build_command("test", None, &session, None);
    assert!(stdin_data.is_none(), "Short prompts should stay on argv");
    let envs: Vec<_> = cmd.as_std().get_envs().collect();
    let env_map: HashMap<&std::ffi::OsStr, Option<&std::ffi::OsStr>> = envs.into_iter().collect();

    assert_eq!(
        env_map.get(std::ffi::OsStr::new("CSA_DEPTH")),
        Some(&Some(std::ffi::OsStr::new("4"))),
        "CSA_DEPTH should be 3 + 1 = 4"
    );
}

#[test]
fn test_build_command_parent_session_env() {
    let exec = Executor::GeminiCli {
        model_override: None,
        thinking_budget: None,
    };
    let mut session = make_test_session();
    session.genealogy.parent_session_id = Some("01HPARENT0000000000000000".to_string());

    let (cmd, stdin_data) = exec.build_command("test", None, &session, None);
    assert!(stdin_data.is_none(), "Short prompts should stay on argv");
    let envs: Vec<_> = cmd.as_std().get_envs().collect();
    let env_map: HashMap<&std::ffi::OsStr, Option<&std::ffi::OsStr>> = envs.into_iter().collect();

    assert_eq!(
        env_map.get(std::ffi::OsStr::new("CSA_PARENT_SESSION")),
        Some(&Some(std::ffi::OsStr::new("01HPARENT0000000000000000"))),
    );
}

#[test]
fn test_build_command_no_parent_session_env_when_root() {
    let exec = Executor::GeminiCli {
        model_override: None,
        thinking_budget: None,
    };
    let session = make_test_session(); // depth=0, no parent

    let (cmd, stdin_data) = exec.build_command("test", None, &session, None);
    assert!(stdin_data.is_none(), "Short prompts should stay on argv");
    let envs: Vec<_> = cmd.as_std().get_envs().collect();
    let env_map: HashMap<&std::ffi::OsStr, Option<&std::ffi::OsStr>> = envs.into_iter().collect();

    assert!(
        !env_map.contains_key(std::ffi::OsStr::new("CSA_PARENT_SESSION")),
        "Root session should not set CSA_PARENT_SESSION"
    );
}

#[test]
fn test_build_command_extra_env_injection() {
    let exec = Executor::ClaudeCode {
        model_override: None,
        thinking_budget: None,
    };
    let session = make_test_session();
    let mut extra = HashMap::new();
    extra.insert("ANTHROPIC_API_KEY".to_string(), "sk-test-key".to_string());
    extra.insert("MY_CUSTOM_VAR".to_string(), "custom_value".to_string());

    let (cmd, stdin_data) = exec.build_command("test", None, &session, Some(&extra));
    assert!(stdin_data.is_none(), "Short prompts should stay on argv");
    let envs: Vec<_> = cmd.as_std().get_envs().collect();
    let env_map: HashMap<&std::ffi::OsStr, Option<&std::ffi::OsStr>> = envs.into_iter().collect();

    assert_eq!(
        env_map.get(std::ffi::OsStr::new("ANTHROPIC_API_KEY")),
        Some(&Some(std::ffi::OsStr::new("sk-test-key"))),
    );
    assert_eq!(
        env_map.get(std::ffi::OsStr::new("MY_CUSTOM_VAR")),
        Some(&Some(std::ffi::OsStr::new("custom_value"))),
    );
}

// ── build_command: per-tool args structure ───────────────────────

#[test]
fn test_build_command_gemini_args_structure() {
    let exec = Executor::GeminiCli {
        model_override: Some("gemini-3-pro".to_string()),
        thinking_budget: Some(ThinkingBudget::High),
    };
    let session = make_test_session();
    let (cmd, stdin_data) = exec.build_command("analyze code", None, &session, None);
    assert!(stdin_data.is_none(), "Short prompts should stay on argv");

    let args: Vec<_> = cmd
        .as_std()
        .get_args()
        .map(|a| a.to_string_lossy().to_string())
        .collect();

    assert!(args.contains(&"-m".to_string()), "Should have -m flag");
    assert!(
        args.contains(&"gemini-3-pro".to_string()),
        "Should have model name"
    );
    assert!(
        !args.contains(&"--thinking_budget".to_string()),
        "gemini-cli no longer supports --thinking_budget"
    );
    assert!(
        !args.iter().any(|arg| arg == "32768"),
        "gemini thinking token count should not be passed as argv"
    );
    assert!(
        args.contains(&"-y".to_string()),
        "Should have -y (yolo mode)"
    );
    assert!(args.contains(&"-p".to_string()), "Should have -p flag");
    assert!(
        args.contains(&"analyze code".to_string()),
        "Should have prompt"
    );
}

#[test]
fn test_build_command_gemini_default_model_omits_model_flag() {
    let exec = Executor::GeminiCli {
        model_override: Some("default".to_string()),
        thinking_budget: None,
    };
    let session = make_test_session();
    let (cmd, stdin_data) = exec.build_command("analyze code", None, &session, None);
    assert!(stdin_data.is_none(), "Short prompts should stay on argv");

    let args: Vec<_> = cmd
        .as_std()
        .get_args()
        .map(|a| a.to_string_lossy().to_string())
        .collect();

    assert!(
        !args.contains(&"-m".to_string()),
        "\"default\" model should omit -m and let gemini-cli auto-route"
    );
    assert!(
        !args.contains(&"default".to_string()),
        "\"default\" sentinel should not be passed as a literal model"
    );
}

#[test]
fn test_build_command_gemini_adds_include_directories_from_env() {
    let exec = Executor::GeminiCli {
        model_override: None,
        thinking_budget: None,
    };
    let session = make_test_session();
    let mut extra = HashMap::new();
    extra.insert(
        "CSA_GEMINI_INCLUDE_DIRECTORIES".to_string(),
        " /tmp/one ,/tmp/two\n/tmp/one ".to_string(),
    );

    let (cmd, stdin_data) = exec.build_command("analyze code", None, &session, Some(&extra));
    assert!(stdin_data.is_none(), "Short prompts should stay on argv");

    let args: Vec<_> = cmd
        .as_std()
        .get_args()
        .map(|a| a.to_string_lossy().to_string())
        .collect();

    let include_flag_count = args
        .iter()
        .filter(|arg| arg.as_str() == "--include-directories")
        .count();
    assert_eq!(
        include_flag_count, 3,
        "Expected execution dir + deduplicated include directories from env"
    );
    assert!(args.contains(&"/tmp/test-project".to_string()));
    assert!(args.contains(&"/tmp/one".to_string()));
    assert!(args.contains(&"/tmp/two".to_string()));
}

#[test]
fn test_build_command_gemini_supports_fallback_include_directories_key() {
    let exec = Executor::GeminiCli {
        model_override: None,
        thinking_budget: None,
    };
    let session = make_test_session();
    let mut extra = HashMap::new();
    extra.insert(
        "GEMINI_INCLUDE_DIRECTORIES".to_string(),
        "/tmp/fallback".to_string(),
    );

    let (cmd, stdin_data) = exec.build_command("analyze code", None, &session, Some(&extra));
    assert!(stdin_data.is_none(), "Short prompts should stay on argv");

    let args: Vec<_> = cmd
        .as_std()
        .get_args()
        .map(|a| a.to_string_lossy().to_string())
        .collect();

    assert!(
        args.contains(&"--include-directories".to_string()),
        "Expected --include-directories when fallback env key is set"
    );
    assert!(args.contains(&"/tmp/test-project".to_string()));
    assert!(args.contains(&"/tmp/fallback".to_string()));
}

#[test]
fn test_build_command_gemini_auto_includes_prompt_absolute_path_parent() {
    let exec = Executor::GeminiCli {
        model_override: None,
        thinking_budget: None,
    };
    let session = make_test_session();
    let temp = tempfile::tempdir().expect("tempdir");
    let dir_with_space = temp.path().join("with space");
    std::fs::create_dir_all(&dir_with_space).expect("create spaced dir");
    let file_path = dir_with_space.join("sample.txt");
    std::fs::write(&file_path, "ok").expect("write fixture");
    let prompt = format!("Read and patch {}", file_path.display());

    let (cmd, stdin_data) = exec.build_command(&prompt, None, &session, None);
    assert!(stdin_data.is_none(), "Short prompts should stay on argv");

    let args: Vec<_> = cmd
        .as_std()
        .get_args()
        .map(|a| a.to_string_lossy().to_string())
        .collect();
    let expected_dir =
        std::fs::canonicalize(&dir_with_space).unwrap_or_else(|_| dir_with_space.clone());

    assert!(args.contains(&"--include-directories".to_string()));
    assert!(args.contains(&"/tmp/test-project".to_string()));
    assert!(args.contains(&expected_dir.to_string_lossy().to_string()));
}

#[test]
fn test_build_command_gemini_never_includes_filesystem_root_directory() {
    let exec = Executor::GeminiCli {
        model_override: None,
        thinking_budget: None,
    };
    let session = make_test_session();
    let mut extra = HashMap::new();
    extra.insert(
        "CSA_GEMINI_INCLUDE_DIRECTORIES".to_string(),
        "/".to_string(),
    );

    let (cmd, stdin_data) =
        exec.build_command("Inspect / and summarize", None, &session, Some(&extra));
    assert!(stdin_data.is_none(), "Short prompts should stay on argv");

    let args: Vec<_> = cmd
        .as_std()
        .get_args()
        .map(|a| a.to_string_lossy().to_string())
        .collect();

    assert!(
        !args.iter().any(|arg| arg == "/"),
        "Filesystem root must not be injected into --include-directories"
    );
    assert!(
        args.contains(&"/tmp/test-project".to_string()),
        "Execution directory should still be included"
    );
}

#[test]
fn test_build_command_claude_args_structure() {
    let exec = Executor::ClaudeCode {
        model_override: Some("claude-opus".to_string()),
        thinking_budget: Some(ThinkingBudget::Medium),
    };
    let session = make_test_session();
    let (cmd, stdin_data) = exec.build_command("do stuff", None, &session, None);
    assert!(stdin_data.is_none(), "Short prompts should stay on argv");

    let args: Vec<_> = cmd
        .as_std()
        .get_args()
        .map(|a| a.to_string_lossy().to_string())
        .collect();

    assert!(
        args.contains(&"--dangerously-skip-permissions".to_string()),
        "Should have permissions skip"
    );
    assert!(
        args.contains(&"--output-format".to_string()),
        "Should have --output-format"
    );
    assert!(args.contains(&"json".to_string()), "Should output json");
    assert!(args.contains(&"--model".to_string()), "Should have --model");
    assert!(
        args.contains(&"claude-opus".to_string()),
        "Should have model name"
    );
    assert!(
        args.contains(&"--thinking-budget".to_string()),
        "Should have --thinking-budget"
    );
    assert!(
        args.contains(&"8192".to_string()),
        "Medium budget = 8192 tokens"
    );
    assert!(args.contains(&"-p".to_string()), "Should have -p flag");
    assert!(args.contains(&"do stuff".to_string()), "Should have prompt");
}

#[test]
fn test_build_command_codex_args_structure() {
    let exec = Executor::Codex {
        model_override: Some("gpt-5".to_string()),
        thinking_budget: Some(ThinkingBudget::Low),
    };
    let session = make_test_session();
    let (cmd, stdin_data) = exec.build_command("fix bug", None, &session, None);
    assert!(stdin_data.is_none(), "Short prompts should stay on argv");

    let args: Vec<_> = cmd
        .as_std()
        .get_args()
        .map(|a| a.to_string_lossy().to_string())
        .collect();

    assert!(
        args.contains(&"exec".to_string()),
        "Should have exec subcommand"
    );
    assert!(
        args.contains(&"--dangerously-bypass-approvals-and-sandbox".to_string()),
        "Should have sandbox bypass"
    );
    assert!(args.contains(&"--model".to_string()), "Should have --model");
    assert!(args.contains(&"gpt-5".to_string()), "Should have model");
    assert!(
        args.contains(&"-c".to_string()),
        "Should have -c flag for reasoning effort"
    );
    assert!(
        args.contains(&"model_reasoning_effort=low".to_string()),
        "Low budget = low effort via -c model_reasoning_effort=low"
    );
    assert!(args.contains(&"fix bug".to_string()), "Should have prompt");
}

#[test]
fn test_build_command_opencode_args_structure() {
    let exec = Executor::Opencode {
        model_override: Some("google/gemini-2.5-pro".to_string()),
        agent: Some("coder".to_string()),
        thinking_budget: Some(ThinkingBudget::Xhigh),
    };
    let session = make_test_session();
    let (cmd, stdin_data) = exec.build_command("write tests", None, &session, None);
    assert!(stdin_data.is_none(), "Short prompts should stay on argv");

    let args: Vec<_> = cmd
        .as_std()
        .get_args()
        .map(|a| a.to_string_lossy().to_string())
        .collect();

    assert!(
        args.contains(&"run".to_string()),
        "Should have run subcommand"
    );
    assert!(
        args.contains(&"--format".to_string()),
        "Should have --format"
    );
    assert!(
        args.contains(&"json".to_string()),
        "Should have json format"
    );
    assert!(args.contains(&"-m".to_string()), "Should have -m flag");
    assert!(
        args.contains(&"google/gemini-2.5-pro".to_string()),
        "Should have model"
    );
    assert!(args.contains(&"--agent".to_string()), "Should have --agent");
    assert!(
        args.contains(&"coder".to_string()),
        "Should have agent name"
    );
    assert!(
        args.contains(&"--variant".to_string()),
        "Should have --variant"
    );
    assert!(
        args.contains(&"max".to_string()),
        "Xhigh maps to max variant"
    );
    assert!(
        args.contains(&"write tests".to_string()),
        "Should have prompt"
    );
}

// ── build_command: session resume ───────────────────────────────

#[test]
fn test_build_command_with_session_resume_codex() {
    let exec = Executor::Codex {
        model_override: None,
        thinking_budget: None,
    };
    let session = make_test_session();
    let tool_state = ToolState {
        provider_session_id: Some("thread_abc123".to_string()),
        last_action_summary: "previous run".to_string(),
        last_exit_code: 0,
        updated_at: chrono::Utc::now(),
        token_usage: None,
    };

    let (cmd, stdin_data) = exec.build_command("continue", Some(&tool_state), &session, None);
    assert!(stdin_data.is_none(), "Short prompts should stay on argv");
    let args: Vec<_> = cmd
        .as_std()
        .get_args()
        .map(|a| a.to_string_lossy().to_string())
        .collect();

    assert!(
        args.contains(&"--session-id".to_string()),
        "Codex should use --session-id for resume"
    );
    assert!(
        args.contains(&"thread_abc123".to_string()),
        "Should pass the session id"
    );
}

#[test]
fn test_build_command_with_session_resume_claude() {
    let exec = Executor::ClaudeCode {
        model_override: None,
        thinking_budget: None,
    };
    let session = make_test_session();
    let tool_state = ToolState {
        provider_session_id: Some("claude_session_789".to_string()),
        last_action_summary: "previous".to_string(),
        last_exit_code: 0,
        updated_at: chrono::Utc::now(),
        token_usage: None,
    };

    let (cmd, stdin_data) = exec.build_command("continue", Some(&tool_state), &session, None);
    assert!(stdin_data.is_none(), "Short prompts should stay on argv");
    let args: Vec<_> = cmd
        .as_std()
        .get_args()
        .map(|a| a.to_string_lossy().to_string())
        .collect();

    assert!(
        args.contains(&"--resume".to_string()),
        "ClaudeCode should use --resume"
    );
    assert!(
        args.contains(&"claude_session_789".to_string()),
        "Should pass the session id"
    );
}

#[test]
fn test_build_command_with_session_resume_gemini() {
    let exec = Executor::GeminiCli {
        model_override: None,
        thinking_budget: None,
    };
    let session = make_test_session();
    let tool_state = ToolState {
        provider_session_id: Some("gemini_session_abc".to_string()),
        last_action_summary: "previous".to_string(),
        last_exit_code: 0,
        updated_at: chrono::Utc::now(),
        token_usage: None,
    };

    let (cmd, stdin_data) = exec.build_command("continue", Some(&tool_state), &session, None);
    assert!(stdin_data.is_none(), "Short prompts should stay on argv");
    let args: Vec<_> = cmd
        .as_std()
        .get_args()
        .map(|a| a.to_string_lossy().to_string())
        .collect();

    assert!(
        args.contains(&"-r".to_string()),
        "GeminiCli should use -r for resume"
    );
    assert!(
        args.contains(&"gemini_session_abc".to_string()),
        "Should pass the session id"
    );
}

#[test]
fn test_build_command_no_resume_without_provider_session_id() {
    let exec = Executor::Codex {
        model_override: None,
        thinking_budget: None,
    };
    let session = make_test_session();
    let tool_state = ToolState {
        provider_session_id: None, // No provider session yet
        last_action_summary: "first run".to_string(),
        last_exit_code: 0,
        updated_at: chrono::Utc::now(),
        token_usage: None,
    };

    let (cmd, stdin_data) = exec.build_command("start", Some(&tool_state), &session, None);
    assert!(stdin_data.is_none(), "Short prompts should stay on argv");
    let args: Vec<_> = cmd
        .as_std()
        .get_args()
        .map(|a| a.to_string_lossy().to_string())
        .collect();

    assert!(
        !args.contains(&"--session-id".to_string()),
        "Should not have --session-id when provider_session_id is None"
    );
}

include!("executor_build_cmd_tests_part2.rs");
