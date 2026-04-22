// ── build_command: session resume ───────────────────────────────

#[test]
fn test_build_command_with_session_resume_codex() {
    let exec = Executor::Codex {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: crate::codex_runtime::codex_runtime_metadata(),
    };
    let session = make_test_session();
    let tool_state = ToolState {
        provider_session_id: Some("thread_abc123".to_string()),
        last_action_summary: "previous run".to_string(),
        last_exit_code: 0,
        updated_at: chrono::Utc::now(),
        tool_version: None,
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
        args == vec![
            "exec",
            "--json",
            "--dangerously-bypass-approvals-and-sandbox",
            "resume",
            "thread_abc123",
            "continue",
        ],
        "Codex resume argv should match the CLI contract"
    );
}

#[test]
fn test_build_command_with_session_resume_codex_long_prompt_uses_stdin_marker() {
    let exec = Executor::Codex {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: crate::codex_runtime::codex_runtime_metadata(),
    };
    let session = make_test_session();
    let tool_state = ToolState {
        provider_session_id: Some("thread_abc123".to_string()),
        last_action_summary: "previous run".to_string(),
        last_exit_code: 0,
        updated_at: chrono::Utc::now(),
        tool_version: None,
        token_usage: None,
    };
    let prompt = "p".repeat(MAX_ARGV_PROMPT_LEN + 1);

    let (cmd, stdin_data) = exec.build_command(&prompt, Some(&tool_state), &session, None);
    let args: Vec<_> = cmd
        .as_std()
        .get_args()
        .map(|a| a.to_string_lossy().to_string())
        .collect();

    assert_eq!(
        args,
        vec![
            "exec",
            "--json",
            "--dangerously-bypass-approvals-and-sandbox",
            "resume",
            "thread_abc123",
            "-",
        ],
        "Codex resume stdin transport should use '-' positional prompt marker"
    );
    assert_eq!(
        stdin_data,
        Some(prompt.as_bytes().to_vec()),
        "Long resume prompts should be piped via stdin"
    );
}

#[test]
fn test_build_command_with_session_resume_claude_cli_is_ignored() {
    let exec = Executor::ClaudeCode {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: crate::claude_runtime::ClaudeCodeRuntimeMetadata::from_transport(
            crate::claude_runtime::ClaudeCodeTransport::Cli,
        ),
    };
    let session = make_test_session();
    let tool_state = ToolState {
        provider_session_id: Some("claude_session_789".to_string()),
        last_action_summary: "previous".to_string(),
        last_exit_code: 0,
        updated_at: chrono::Utc::now(),
        tool_version: None,
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
        !args.contains(&"--resume".to_string()),
        "Claude CLI transport must not advertise resume support"
    );
    assert!(
        !args.contains(&"claude_session_789".to_string()),
        "Claude CLI transport must not pass provider session IDs"
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
        tool_version: None,
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
        runtime_metadata: crate::codex_runtime::codex_runtime_metadata(),
    };
    let session = make_test_session();
    let tool_state = ToolState {
        provider_session_id: None, // No provider session yet
        last_action_summary: "first run".to_string(),
        last_exit_code: 0,
        updated_at: chrono::Utc::now(),
        tool_version: None,
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
        args.contains(&"--json".to_string()),
        "New Codex sessions must request JSONL output for session extraction"
    );
    assert!(
        !args.contains(&"--session-id".to_string()),
        "New Codex sessions must not use the removed --session-id flag"
    );
    assert!(
        !args.contains(&"resume".to_string()),
        "New Codex sessions must not use the resume subcommand"
    );
}
