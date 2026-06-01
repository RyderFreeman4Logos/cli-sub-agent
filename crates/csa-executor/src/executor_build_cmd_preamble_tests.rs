// ── CSA sub-agent identity preamble ────────────────────────────

#[test]
fn test_claude_code_prompt_includes_identity_preamble() {
    let exec = Executor::ClaudeCode {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: crate::claude_runtime::claude_runtime_metadata(),
    };
    let session = make_test_session();
    let (cmd, _stdin_data) = exec.build_command("implement feature X", None, &session, None, None);

    let args: Vec<_> = cmd
        .as_std()
        .get_args()
        .map(|a| a.to_string_lossy().to_string())
        .collect();

    let prompt_arg = args
        .iter()
        .find(|a| a.contains("implement feature X"))
        .expect("Should have prompt arg");

    assert!(
        prompt_arg.starts_with("<csa-sub-agent-context>"),
        "claude-code prompt should start with CSA identity preamble"
    );
    assert!(
        prompt_arg.contains("You are running INSIDE a CSA"),
        "Preamble should state the agent is inside CSA"
    );
    assert!(
        prompt_arg.contains(&session.meta_session_id),
        "Preamble should contain the session ID"
    );
    assert!(
        prompt_arg.contains("CSA_DEPTH=1"),
        "Preamble should contain child depth (parent 0 + 1 = 1)"
    );
    assert!(
        prompt_arg.contains("AGENTS.md rule 049 does not apply"),
        "Preamble should explicitly exempt rule 049"
    );
    assert!(
        prompt_arg.contains("</csa-sub-agent-context>"),
        "Preamble should have closing tag"
    );
    assert!(
        prompt_arg.ends_with("implement feature X"),
        "Original prompt should follow the preamble"
    );
}

#[test]
fn test_claude_code_preamble_reflects_session_depth() {
    let exec = Executor::ClaudeCode {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: crate::claude_runtime::claude_runtime_metadata(),
    };
    let mut session = make_test_session();
    session.genealogy.depth = 3;

    let (cmd, _stdin_data) = exec.build_command("deep task", None, &session, None, None);

    let args: Vec<_> = cmd
        .as_std()
        .get_args()
        .map(|a| a.to_string_lossy().to_string())
        .collect();

    let prompt_arg = args
        .iter()
        .find(|a| a.contains("deep task"))
        .expect("Should have prompt arg");

    assert!(
        prompt_arg.contains("CSA_DEPTH=4"),
        "Preamble depth should be parent depth (3) + 1 = 4"
    );
}

#[test]
fn test_gemini_prompt_has_no_preamble() {
    let exec = Executor::GeminiCli {
        model_override: None,
        thinking_budget: None,
    };
    let session = make_test_session();
    let (cmd, _stdin_data) = exec.build_command("hello world", None, &session, None, None);

    let args: Vec<_> = cmd
        .as_std()
        .get_args()
        .map(|a| a.to_string_lossy().to_string())
        .collect();

    assert!(
        args.contains(&"hello world".to_string()),
        "GeminiCli prompt should be unmodified"
    );
    assert!(
        !args.iter().any(|a| a.contains("<csa-sub-agent-context>")),
        "GeminiCli should NOT have CSA identity preamble"
    );
}

#[test]
fn test_codex_prompt_has_no_preamble() {
    let exec = Executor::Codex {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: crate::codex_runtime::codex_runtime_metadata(),
    };
    let session = make_test_session();
    let (cmd, _stdin_data) = exec.build_command("fix bug", None, &session, None, None);

    let args: Vec<_> = cmd
        .as_std()
        .get_args()
        .map(|a| a.to_string_lossy().to_string())
        .collect();

    assert!(
        args.contains(&"fix bug".to_string()),
        "Codex prompt should be unmodified"
    );
    assert!(
        !args.iter().any(|a| a.contains("<csa-sub-agent-context>")),
        "Codex should NOT have CSA identity preamble"
    );
}

#[test]
fn test_opencode_prompt_has_no_preamble() {
    let exec = Executor::Opencode {
        model_override: None,
        agent: None,
        thinking_budget: None,
    };
    let session = make_test_session();
    let (cmd, _stdin_data) = exec.build_command("write tests", None, &session, None, None);

    let args: Vec<_> = cmd
        .as_std()
        .get_args()
        .map(|a| a.to_string_lossy().to_string())
        .collect();

    assert!(
        args.contains(&"write tests".to_string()),
        "Opencode prompt should be unmodified"
    );
    assert!(
        !args.iter().any(|a| a.contains("<csa-sub-agent-context>")),
        "Opencode should NOT have CSA identity preamble"
    );
}
