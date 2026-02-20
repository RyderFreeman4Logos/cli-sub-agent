//! Tests for build_command, inject_env, effort mapping, and boundary conditions.

use super::*;

/// Helper: create a minimal MetaSessionState for testing.
fn make_test_session() -> MetaSessionState {
    let now = chrono::Utc::now();
    MetaSessionState {
        meta_session_id: "01HTEST000000000000000000".to_string(),
        description: Some("test session".to_string()),
        project_path: "/tmp/test-project".to_string(),
        created_at: now,
        last_accessed: now,
        genealogy: csa_session::state::Genealogy {
            parent_session_id: None,
            depth: 0,
        },
        tools: HashMap::new(),
        context_status: csa_session::state::ContextStatus::default(),
        total_token_usage: None,
        phase: csa_session::state::SessionPhase::Active,
        task_context: csa_session::state::TaskContext::default(),
        turn_count: 0,
        token_budget: None,
        sandbox_info: None,
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
        args.contains(&"--thinking_budget".to_string()),
        "Should have --thinking_budget"
    );
    assert!(args.contains(&"32768".to_string()), "Should have 32768");
    assert!(args.contains(&"-y".to_string()), "Should have -y (yolo)");
    assert!(args.contains(&"-p".to_string()), "Should have -p flag");
    assert!(
        args.contains(&"analyze code".to_string()),
        "Should have prompt"
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

// ── inject_env tests ────────────────────────────────────────────

#[test]
fn test_inject_env_sets_all_vars() {
    let mut cmd = Command::new("echo");
    let mut env_vars = HashMap::new();
    env_vars.insert("KEY_A".to_string(), "value_a".to_string());
    env_vars.insert("KEY_B".to_string(), "value_b".to_string());
    env_vars.insert("KEY_C".to_string(), "value_c".to_string());

    Executor::inject_env(&mut cmd, &env_vars);

    let envs: Vec<_> = cmd.as_std().get_envs().collect();
    let env_map: HashMap<&std::ffi::OsStr, Option<&std::ffi::OsStr>> = envs.into_iter().collect();

    assert_eq!(
        env_map.get(std::ffi::OsStr::new("KEY_A")),
        Some(&Some(std::ffi::OsStr::new("value_a")))
    );
    assert_eq!(
        env_map.get(std::ffi::OsStr::new("KEY_B")),
        Some(&Some(std::ffi::OsStr::new("value_b")))
    );
    assert_eq!(
        env_map.get(std::ffi::OsStr::new("KEY_C")),
        Some(&Some(std::ffi::OsStr::new("value_c")))
    );
}

#[test]
fn test_inject_env_empty_map_is_noop() {
    let mut cmd = Command::new("echo");
    let env_vars = HashMap::new();

    Executor::inject_env(&mut cmd, &env_vars);

    let envs: Vec<_> = cmd.as_std().get_envs().collect();
    assert!(envs.is_empty(), "Empty env map should add no variables");
}

// ── codex_effort mapping for all ThinkingBudget variants ────────

#[test]
fn test_codex_effort_all_variants() {
    assert_eq!(ThinkingBudget::DefaultBudget.codex_effort(), "medium");
    assert_eq!(ThinkingBudget::Low.codex_effort(), "low");
    assert_eq!(ThinkingBudget::Medium.codex_effort(), "medium");
    assert_eq!(ThinkingBudget::High.codex_effort(), "high");
    assert_eq!(ThinkingBudget::Xhigh.codex_effort(), "xhigh");
    assert_eq!(ThinkingBudget::Custom(0).codex_effort(), "high");
    assert_eq!(ThinkingBudget::Custom(100000).codex_effort(), "high");
}

// ── token_count for all ThinkingBudget variants ─────────────────

#[test]
fn test_token_count_all_variants() {
    assert_eq!(ThinkingBudget::DefaultBudget.token_count(), 10000);
    assert_eq!(ThinkingBudget::Low.token_count(), 1024);
    assert_eq!(ThinkingBudget::Medium.token_count(), 8192);
    assert_eq!(ThinkingBudget::High.token_count(), 32768);
    assert_eq!(ThinkingBudget::Xhigh.token_count(), 65536);
    assert_eq!(ThinkingBudget::Custom(0).token_count(), 0);
    assert_eq!(ThinkingBudget::Custom(u32::MAX).token_count(), u32::MAX);
}

// ── Boundary / error path tests ─────────────────────────────────

#[test]
fn test_from_spec_empty_model_name() {
    // An empty model name is syntactically valid — it should parse without error
    let spec = ModelSpec::parse("codex//model/high").unwrap();
    assert_eq!(spec.provider, "");
}

#[test]
fn test_build_command_with_empty_prompt() {
    let exec = Executor::ClaudeCode {
        model_override: None,
        thinking_budget: None,
    };
    let session = make_test_session();
    let (cmd, stdin_data) = exec.build_command("", None, &session, None);
    assert!(stdin_data.is_none(), "Short prompts should stay on argv");

    let args: Vec<_> = cmd
        .as_std()
        .get_args()
        .map(|a| a.to_string_lossy().to_string())
        .collect();

    // Empty prompt should still be passed as an argument
    assert!(
        args.contains(&"".to_string()),
        "Empty prompt should be present as an arg: {args:?}"
    );
}

#[test]
fn test_build_command_prompt_with_special_characters() {
    let exec = Executor::Codex {
        model_override: None,
        thinking_budget: None,
    };
    let session = make_test_session();
    let special_prompt = "Fix the bug in `fn main()` \u{2014} use \"quotes\" & $ENV_VAR\nnewline";

    let (cmd, stdin_data) = exec.build_command(special_prompt, None, &session, None);
    assert!(stdin_data.is_none(), "Short prompts should stay on argv");
    let args: Vec<_> = cmd
        .as_std()
        .get_args()
        .map(|a| a.to_string_lossy().to_string())
        .collect();

    assert!(
        args.contains(&special_prompt.to_string()),
        "Special characters in prompt should be preserved: {args:?}"
    );
}

#[test]
fn test_build_command_no_model_override_omits_model_flag() {
    let exec = Executor::ClaudeCode {
        model_override: None,
        thinking_budget: None,
    };
    let session = make_test_session();
    let (cmd, stdin_data) = exec.build_command("test", None, &session, None);
    assert!(stdin_data.is_none(), "Short prompts should stay on argv");

    let args: Vec<_> = cmd
        .as_std()
        .get_args()
        .map(|a| a.to_string_lossy().to_string())
        .collect();

    assert!(
        !args.contains(&"--model".to_string()),
        "Should not have --model when model_override is None"
    );
    assert!(
        !args.contains(&"--thinking-budget".to_string()),
        "Should not have --thinking-budget when thinking_budget is None"
    );
}

#[test]
fn test_build_command_executable_program() {
    let exec = Executor::GeminiCli {
        model_override: None,
        thinking_budget: None,
    };
    let session = make_test_session();
    let (cmd, stdin_data) = exec.build_command("test", None, &session, None);
    assert!(stdin_data.is_none(), "Short prompts should stay on argv");

    assert_eq!(
        cmd.as_std().get_program(),
        std::ffi::OsStr::new("gemini"),
        "GeminiCli should use 'gemini' executable"
    );
}

#[test]
fn test_build_command_current_dir() {
    let exec = Executor::ClaudeCode {
        model_override: None,
        thinking_budget: None,
    };
    let mut session = make_test_session();
    session.project_path = "/home/user/my-project".to_string();

    let (cmd, stdin_data) = exec.build_command("test", None, &session, None);
    assert!(stdin_data.is_none(), "Short prompts should stay on argv");

    assert_eq!(
        cmd.as_std().get_current_dir(),
        Some(std::path::Path::new("/home/user/my-project")),
        "Should set current_dir to session project_path"
    );
}

// ── STRIPPED_ENV_VARS: recursion guard removal ──────────────────
// Both build_base_command and build_execute_in_command must strip these.

#[test]
fn test_stripped_env_vars_contains_claudecode() {
    assert!(
        Executor::STRIPPED_ENV_VARS.contains(&"CLAUDECODE"),
        "STRIPPED_ENV_VARS must strip CLAUDECODE (recursion detection)"
    );
    assert!(
        Executor::STRIPPED_ENV_VARS.contains(&"CLAUDE_CODE_ENTRYPOINT"),
        "STRIPPED_ENV_VARS must strip CLAUDE_CODE_ENTRYPOINT (parent context)"
    );
}

#[test]
fn test_build_command_strips_claudecode_env() {
    let exec = Executor::ClaudeCode {
        model_override: None,
        thinking_budget: None,
    };
    let session = make_test_session();
    let (cmd, _stdin_data) = exec.build_command("test", None, &session, None);

    let envs: Vec<_> = cmd.as_std().get_envs().collect();
    let env_map: HashMap<&std::ffi::OsStr, Option<&std::ffi::OsStr>> = envs.into_iter().collect();

    // env_remove() registers the key with None value, signalling
    // "remove from inherited environment".
    assert_eq!(
        env_map.get(std::ffi::OsStr::new("CLAUDECODE")),
        Some(&None),
        "CLAUDECODE should be env_removed (value = None)"
    );
    assert_eq!(
        env_map.get(std::ffi::OsStr::new("CLAUDE_CODE_ENTRYPOINT")),
        Some(&None),
        "CLAUDE_CODE_ENTRYPOINT should be env_removed (value = None)"
    );
}

#[test]
fn test_build_command_strips_claudecode_for_all_executors() {
    let executors: Vec<Executor> = vec![
        Executor::GeminiCli {
            model_override: None,
            thinking_budget: None,
        },
        Executor::ClaudeCode {
            model_override: None,
            thinking_budget: None,
        },
        Executor::Codex {
            model_override: None,
            thinking_budget: None,
        },
        Executor::Opencode {
            model_override: None,
            agent: None,
            thinking_budget: None,
        },
    ];

    let session = make_test_session();
    for exec in executors {
        let tool = exec.tool_name().to_string();
        let (cmd, _) = exec.build_command("test", None, &session, None);
        let envs: Vec<_> = cmd.as_std().get_envs().collect();
        let env_map: HashMap<&std::ffi::OsStr, Option<&std::ffi::OsStr>> =
            envs.into_iter().collect();

        assert_eq!(
            env_map.get(std::ffi::OsStr::new("CLAUDECODE")),
            Some(&None),
            "{tool}: CLAUDECODE should be stripped"
        );
    }
}

// ── build_execute_in_command: env stripping ─────────────────────

#[test]
fn test_build_execute_in_command_strips_claudecode_env() {
    let exec = Executor::ClaudeCode {
        model_override: None,
        thinking_budget: None,
    };
    let work_dir = std::path::Path::new("/tmp/test-project");
    let (cmd, _stdin_data) = exec.build_execute_in_command("test", work_dir, None);

    let envs: Vec<_> = cmd.as_std().get_envs().collect();
    let env_map: HashMap<&std::ffi::OsStr, Option<&std::ffi::OsStr>> = envs.into_iter().collect();

    assert_eq!(
        env_map.get(std::ffi::OsStr::new("CLAUDECODE")),
        Some(&None),
        "build_execute_in_command should strip CLAUDECODE"
    );
    assert_eq!(
        env_map.get(std::ffi::OsStr::new("CLAUDE_CODE_ENTRYPOINT")),
        Some(&None),
        "build_execute_in_command should strip CLAUDE_CODE_ENTRYPOINT"
    );
}

#[test]
fn test_build_execute_in_command_strips_claudecode_for_all_executors() {
    let executors: Vec<Executor> = vec![
        Executor::GeminiCli {
            model_override: None,
            thinking_budget: None,
        },
        Executor::ClaudeCode {
            model_override: None,
            thinking_budget: None,
        },
        Executor::Codex {
            model_override: None,
            thinking_budget: None,
        },
        Executor::Opencode {
            model_override: None,
            agent: None,
            thinking_budget: None,
        },
    ];

    let work_dir = std::path::Path::new("/tmp/test-project");
    for exec in executors {
        let tool = exec.tool_name().to_string();
        let (cmd, _) = exec.build_execute_in_command("test", work_dir, None);
        let envs: Vec<_> = cmd.as_std().get_envs().collect();
        let env_map: HashMap<&std::ffi::OsStr, Option<&std::ffi::OsStr>> =
            envs.into_iter().collect();

        assert_eq!(
            env_map.get(std::ffi::OsStr::new("CLAUDECODE")),
            Some(&None),
            "{tool}: build_execute_in_command should strip CLAUDECODE"
        );
    }
}

// NOTE: CSA_SUPPRESS_NOTIFY is injected by the pipeline layer (not executor)
// based on per-tool config. See pipeline.rs suppress_notify logic.
