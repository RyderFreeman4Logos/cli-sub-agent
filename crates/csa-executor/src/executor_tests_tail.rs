#[test]
fn test_thinking_budget_from_spec_gemini() {
    let spec = ModelSpec::parse("gemini-cli/google/gemini-3-pro/high").unwrap();
    let exec = Executor::from_spec(&spec).unwrap();

    let mut cmd = Command::new(exec.executable_name());
    exec.append_tool_args(&mut cmd, "test prompt", None);

    let debug_str = format!("{cmd:?}");
    assert!(!debug_str.contains("--thinking_budget"));
    assert!(!debug_str.contains("32768"));
}

#[test]
fn test_thinking_budget_from_spec_codex() {
    let spec = ModelSpec::parse("codex/openai/gpt-5/low").unwrap();
    let exec = Executor::from_spec(&spec).unwrap();

    let mut cmd = Command::new(exec.executable_name());
    exec.append_tool_args(&mut cmd, "test prompt", None);

    let debug_str = format!("{cmd:?}");
    assert!(debug_str.contains("model_reasoning_effort=low"));
}

#[test]
fn test_hermes_thinking_does_not_emit_codex_reasoning_effort() {
    let spec = ModelSpec::parse("hermes/openai/gpt-5.5/xhigh").unwrap();
    let exec = Executor::from_spec(&spec).unwrap();

    let mut cmd = Command::new(exec.executable_name());
    exec.append_tool_args(&mut cmd, "test prompt", None);

    let debug_str = format!("{cmd:?}");
    assert!(debug_str.contains("--provider"), "{debug_str}");
    assert!(debug_str.contains("openai"), "{debug_str}");
    assert!(debug_str.contains("--model"), "{debug_str}");
    assert!(debug_str.contains("gpt-5.5"), "{debug_str}");
    assert!(
        !debug_str.contains("model_reasoning_effort="),
        "Hermes must not receive Codex-specific reasoning args: {debug_str}"
    );
}

#[test]
fn test_thinking_budget_custom_value() {
    use crate::model_spec::ThinkingBudget;
    let exec = Executor::ClaudeCode {
        model_override: Some("claude-opus".to_string()),
        thinking_budget: Some(ThinkingBudget::Custom(10000)),
        runtime_metadata: claude_runtime_metadata(),
    };

    let mut cmd = Command::new(exec.executable_name());
    exec.append_tool_args(&mut cmd, "test prompt", None);

    let debug_str = format!("{cmd:?}");
    // claude-code 2.x exposes thinking via `--effort <level>`, not the
    // removed `--thinking-budget <tokens>` flag (#1124). `Custom(n)` has no
    // direct level so it folds into "high" (mirrors codex_effort).
    assert!(
        !debug_str.contains("--thinking-budget"),
        "Should not emit removed --thinking-budget flag (#1124): {debug_str}"
    );
    assert!(
        debug_str.contains("--effort"),
        "Should emit --effort: {debug_str}"
    );
    assert!(
        debug_str.contains("high"),
        "Custom(10000) maps to --effort high: {debug_str}"
    );
}

#[test]
fn test_apply_restrictions_allow_all() {
    let exec = Executor::GeminiCli {
        model_override: None,
        thinking_budget: None,
    };

    let original_prompt = "Refactor the authentication module";
    let result = exec.apply_restrictions(original_prompt, true, true);

    // When both edit and write are allowed, prompt should be unchanged
    assert_eq!(result, original_prompt);
}

#[test]
fn test_apply_restrictions_deny_edit_allow_write() {
    let exec = Executor::GeminiCli {
        model_override: None,
        thinking_budget: None,
    };

    let original_prompt = "Analyze the authentication module";
    let result = exec.apply_restrictions(original_prompt, false, true);

    // When edit is denied but write is allowed, prompt should mention edit restriction
    assert!(result.contains("IMPORTANT RESTRICTION"));
    assert!(result.contains("MUST NOT edit or modify any existing files"));
    assert!(result.contains("may only create new files"));
    assert!(result.contains(original_prompt));
}

#[test]
fn test_apply_restrictions_full_read_only() {
    let exec = Executor::GeminiCli {
        model_override: None,
        thinking_budget: None,
    };

    let original_prompt = "Analyze the authentication module";
    let result = exec.apply_restrictions(original_prompt, false, false);

    // Full read-only mode
    assert!(result.contains("READ-ONLY mode"));
    assert!(result.contains("MUST NOT edit existing files"));
    assert!(result.contains("create new files"));
    assert!(result.contains(original_prompt));
}

#[test]
fn test_apply_restrictions_deny_write_allow_edit() {
    let exec = Executor::GeminiCli {
        model_override: None,
        thinking_budget: None,
    };

    let original_prompt = "Fix the bug";
    let result = exec.apply_restrictions(original_prompt, true, false);

    // Can edit but not create new files
    assert!(result.contains("MUST NOT create new files"));
    assert!(result.contains(original_prompt));
}

#[test]
fn test_apply_restrictions_preserves_all_tools() {
    let tools = vec![
        Executor::GeminiCli {
            model_override: None,
            thinking_budget: None,
        },
        Executor::Opencode {
            model_override: None,
            agent: None,
            thinking_budget: None,
        },
        Executor::Codex {
            model_override: None,
            thinking_budget: None,
            runtime_metadata: codex_runtime_metadata(),
        },
        Executor::ClaudeCode {
            model_override: None,
            thinking_budget: None,
            runtime_metadata: claude_runtime_metadata(),
        },
    ];

    let original_prompt = "Analyze code";
    for tool in tools {
        // Test that full read-only restriction works for all tool types
        let restricted = tool.apply_restrictions(original_prompt, false, false);
        assert!(restricted.contains("READ-ONLY mode"));
        assert!(restricted.contains(original_prompt));

        // Test that allowing everything returns original prompt
        let allowed = tool.apply_restrictions(original_prompt, true, true);
        assert_eq!(allowed, original_prompt);
    }
}

#[test]
fn test_opencode_command_construction() {
    use crate::model_spec::ThinkingBudget;
    let exec = Executor::Opencode {
        model_override: Some("google/gemini-2.5-pro".to_string()),
        agent: Some("test-agent".to_string()),
        thinking_budget: Some(ThinkingBudget::High),
    };

    let mut cmd = Command::new(exec.executable_name());
    exec.append_tool_args(&mut cmd, "test prompt", None);

    // Verify command structure matches opencode run syntax
    let debug_str = format!("{cmd:?}");
    assert!(debug_str.contains("\"run\""));
    assert!(debug_str.contains("\"--format\""));
    assert!(debug_str.contains("\"json\""));
    assert!(debug_str.contains("\"-m\""));
    assert!(debug_str.contains("\"google/gemini-2.5-pro\""));
    assert!(debug_str.contains("\"--agent\""));
    assert!(debug_str.contains("\"test-agent\""));
    assert!(debug_str.contains("\"--variant\""));
    assert!(debug_str.contains("\"high\""));
    assert!(debug_str.contains("\"test prompt\""));
    // Verify --yolo is NOT present
    assert!(!debug_str.contains("--yolo"));
}

#[test]
fn test_opencode_variant_mapping() {
    use crate::model_spec::ThinkingBudget;
    let test_cases = vec![
        (ThinkingBudget::Low, "minimal"),
        (ThinkingBudget::Medium, "medium"),
        (ThinkingBudget::High, "high"),
        (ThinkingBudget::Custom(50000), "max"),
    ];

    for (budget, expected_variant) in test_cases {
        let exec = Executor::Opencode {
            model_override: None,
            agent: None,
            thinking_budget: Some(budget),
        };

        let mut cmd = Command::new(exec.executable_name());
        exec.append_tool_args(&mut cmd, "test", None);

        let debug_str = format!("{cmd:?}");
        assert!(
            debug_str.contains(expected_variant),
            "Expected variant '{expected_variant}' not found in command: {debug_str}"
        );
    }
}

#[test]
fn test_execute_in_preserves_model_override() {
    use crate::model_spec::ThinkingBudget;

    // Test each tool variant to ensure execute_in passes model/thinking args
    let tools: Vec<Executor> = vec![
        Executor::GeminiCli {
            model_override: Some("gemini-3-pro".to_string()),
            thinking_budget: Some(ThinkingBudget::High),
        },
        Executor::Codex {
            model_override: Some("gpt-5".to_string()),
            thinking_budget: Some(ThinkingBudget::Low),
            runtime_metadata: codex_runtime_metadata(),
        },
        Executor::ClaudeCode {
            model_override: Some("claude-opus".to_string()),
            thinking_budget: Some(ThinkingBudget::Custom(10000)),
            runtime_metadata: claude_runtime_metadata(),
        },
        Executor::Opencode {
            model_override: Some("google/gemini-2.5-pro".to_string()),
            agent: None,
            thinking_budget: Some(ThinkingBudget::Medium),
        },
        Executor::AntigravityCli {
            model_override: Some("gemini-3-pro".to_string()),
            thinking_budget: Some(ThinkingBudget::High),
        },
        Executor::Hermes {
            provider_override: Some("anthropic".to_string()),
            model_override: Some("claude-opus".to_string()),
            thinking_budget: Some(ThinkingBudget::Medium),
        },
    ];

    for exec in &tools {
        let mut cmd = Command::new(exec.executable_name());
        exec.append_yolo_args(&mut cmd);
        exec.append_model_args(&mut cmd);
        exec.append_prompt_args(&mut cmd, "test prompt");

        let debug_str = format!("{cmd:?}");

        // Every tool should include its model override
        match exec {
            Executor::GeminiCli { .. } => {
                assert!(
                    debug_str.contains("gemini-3-pro"),
                    "GeminiCli missing model: {debug_str}"
                );
                assert!(
                    !debug_str.contains("--thinking_budget"),
                    "GeminiCli should ignore explicit thinking flags: {debug_str}"
                );
            }
            Executor::Codex { .. } => {
                assert!(
                    debug_str.contains("gpt-5"),
                    "Codex missing model: {debug_str}"
                );
                assert!(
                    debug_str.contains("model_reasoning_effort="),
                    "Codex missing thinking: {debug_str}"
                );
            }
            Executor::ClaudeCode { .. } => {
                assert!(
                    debug_str.contains("claude-opus"),
                    "ClaudeCode missing model: {debug_str}"
                );
                // claude-code 2.x: thinking control via --effort <level>,
                // not the removed --thinking-budget <tokens> (#1124).
                assert!(
                    !debug_str.contains("--thinking-budget"),
                    "ClaudeCode must not emit removed --thinking-budget (#1124): {debug_str}"
                );
                assert!(
                    debug_str.contains("--effort"),
                    "ClaudeCode missing thinking effort flag: {debug_str}"
                );
            }
            Executor::Opencode { .. } => {
                assert!(
                    debug_str.contains("google/gemini-2.5-pro"),
                    "Opencode missing model: {debug_str}"
                );
                assert!(
                    debug_str.contains("--variant"),
                    "Opencode missing thinking: {debug_str}"
                );
            }
            Executor::OpenaiCompat { .. } => {
                // HTTP-only tool — no CLI args. Nothing to assert on Command.
            }
            Executor::Hermes { .. } => {
                assert!(
                    debug_str.contains("anthropic"),
                    "Hermes missing provider: {debug_str}"
                );
                assert!(
                    debug_str.contains("claude-opus"),
                    "Hermes missing model: {debug_str}"
                );
                assert!(
                    debug_str.contains("--thinking"),
                    "Hermes missing thinking: {debug_str}"
                );
                assert!(
                    !debug_str.contains("model_reasoning_effort="),
                    "Hermes must not receive Codex-specific thinking args: {debug_str}"
                );
            }
            Executor::AntigravityCli { .. } => {
                // `agy` rejects `-m`; the model override is staged in
                // `~/.gemini/antigravity-cli/settings.json` by
                // `AntigravitySettingsGuard` in the transport layer
                // instead. The CLI args must NOT carry the model (#1620).
                assert!(
                    !debug_str.contains("\"-m\""),
                    "AntigravityCli must NOT emit -m (#1620): {debug_str}"
                );
                assert!(
                    !debug_str.contains("gemini-3-pro"),
                    "AntigravityCli must NOT pass the model name on argv (#1620): {debug_str}"
                );
            }
        }
    }
}

// --- override_thinking_budget tests ---

#[test]
fn override_thinking_budget_replaces_existing() {
    let mut exec =
        Executor::from_tool_name(&ToolName::ClaudeCode, None, Some(ThinkingBudget::Medium));
    exec.override_thinking_budget(ThinkingBudget::Xhigh);
    let debug = format!("{exec:?}");
    assert!(
        debug.contains("Xhigh"),
        "expected Xhigh after override, got: {debug}"
    );
    assert!(
        !debug.contains("Medium"),
        "Medium should be replaced, got: {debug}"
    );
}

#[test]
fn override_thinking_budget_sets_when_none() {
    let mut exec = Executor::from_tool_name(&ToolName::Codex, None, None);
    exec.override_thinking_budget(ThinkingBudget::High);
    let debug = format!("{exec:?}");
    assert!(
        debug.contains("High"),
        "expected High after override from None, got: {debug}"
    );
}

#[test]
fn override_thinking_budget_works_for_all_tools() {
    for tool in &[
        ToolName::GeminiCli,
        ToolName::Opencode,
        ToolName::Codex,
        ToolName::ClaudeCode,
    ] {
        let mut exec = Executor::from_tool_name(tool, None, Some(ThinkingBudget::Low));
        exec.override_thinking_budget(ThinkingBudget::Xhigh);
        let debug = format!("{exec:?}");
        assert!(
            debug.contains("Xhigh"),
            "override failed for {}: {debug}",
            tool.as_str()
        );
    }
}

// --- thinking_budget getter tests (#766) ---

#[test]
fn thinking_budget_getter_returns_some_when_set() {
    let exec = Executor::from_tool_name(&ToolName::Codex, None, Some(ThinkingBudget::High));
    assert!(matches!(exec.thinking_budget(), Some(ThinkingBudget::High)));
}

#[test]
fn thinking_budget_getter_returns_none_when_unset() {
    let exec = Executor::from_tool_name(&ToolName::ClaudeCode, None, None);
    assert!(exec.thinking_budget().is_none());
}

// --- ACP init timeout regression tests (issue #417) ---

#[test]
fn default_acp_init_timeout_is_120() {
    use csa_process::StreamMode;
    let opts = ExecuteOptions::new(StreamMode::TeeToStderr, 250);
    assert_eq!(opts.acp_init_timeout_seconds, 120);
}
