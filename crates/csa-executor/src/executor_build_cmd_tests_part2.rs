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
// TODO(acp-notify): ACP path currently propagates CSA_SUPPRESS_NOTIFY env only;
// codex notify suppression (`-c notify=[]`) is covered here for legacy execute_in.

#[test]
fn test_build_execute_in_command_codex_notify_suppressed() {
    let exec = Executor::Codex {
        model_override: None,
        thinking_budget: None,
    };
    let work_dir = std::path::Path::new("/tmp/test-project");
    let mut extra = HashMap::new();
    extra.insert("CSA_SUPPRESS_NOTIFY".to_string(), "1".to_string());

    let (cmd, _stdin_data) = exec.build_execute_in_command("test", work_dir, Some(&extra));
    let args: Vec<_> = cmd
        .as_std()
        .get_args()
        .map(|a| a.to_string_lossy().to_string())
        .collect();

    assert!(
        args.contains(&"-c".to_string()),
        "Codex should include -c when notify is suppressed"
    );
    assert!(
        args.contains(&"notify=[]".to_string()),
        "Codex should inject -c notify=[] when CSA_SUPPRESS_NOTIFY=1"
    );
}

#[test]
fn test_build_execute_in_command_codex_notify_not_suppressed() {
    let exec = Executor::Codex {
        model_override: None,
        thinking_budget: None,
    };
    let work_dir = std::path::Path::new("/tmp/test-project");
    let mut extra = HashMap::new();
    extra.insert("CSA_SUPPRESS_NOTIFY".to_string(), "0".to_string());

    let (cmd, _stdin_data) = exec.build_execute_in_command("test", work_dir, Some(&extra));
    let args: Vec<_> = cmd
        .as_std()
        .get_args()
        .map(|a| a.to_string_lossy().to_string())
        .collect();

    assert!(
        !args.contains(&"notify=[]".to_string()),
        "Codex should not inject notify suppression when CSA_SUPPRESS_NOTIFY!=1"
    );
}

#[test]
fn test_codex_dual_c_flags_coexist() {
    let exec = Executor::Codex {
        model_override: None,
        thinking_budget: Some(ThinkingBudget::Low),
    };
    let work_dir = std::path::Path::new("/tmp/test-project");
    let mut extra = HashMap::new();
    extra.insert("CSA_SUPPRESS_NOTIFY".to_string(), "1".to_string());

    let (cmd, _stdin_data) = exec.build_execute_in_command("test", work_dir, Some(&extra));
    let args: Vec<_> = cmd
        .as_std()
        .get_args()
        .map(|a| a.to_string_lossy().to_string())
        .collect();

    let c_flag_count = args.iter().filter(|arg| arg.as_str() == "-c").count();
    assert_eq!(
        c_flag_count, 2,
        "Codex should include two -c flags when effort and notify suppression coexist"
    );
    assert!(
        args.contains(&"model_reasoning_effort=low".to_string()),
        "Codex should include thinking budget effort arg"
    );
    assert!(
        args.contains(&"notify=[]".to_string()),
        "Codex should include notify suppression arg"
    );
}

#[test]
fn test_build_execute_in_command_gemini_adds_include_directories_from_env() {
    let exec = Executor::GeminiCli {
        model_override: None,
        thinking_budget: None,
    };
    let work_dir = std::path::Path::new("/tmp/test-project");
    let mut extra = HashMap::new();
    extra.insert(
        "CSA_GEMINI_INCLUDE_DIRECTORIES".to_string(),
        "/tmp/a,/tmp/b".to_string(),
    );

    let (cmd, _stdin_data) = exec.build_execute_in_command("test", work_dir, Some(&extra));
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
        "Gemini execute_in should receive work_dir and both include directories"
    );
    assert!(args.contains(&"/tmp/test-project".to_string()));
    assert!(args.contains(&"/tmp/a".to_string()));
    assert!(args.contains(&"/tmp/b".to_string()));
}
