use super::*;

#[test]
fn test_tool_name() {
    assert_eq!(
        Executor::GeminiCli {
            model_override: None,
            thinking_budget: None,
        }
        .tool_name(),
        "gemini-cli"
    );
    assert_eq!(
        Executor::Opencode {
            model_override: None,
            agent: None,
            thinking_budget: None,
        }
        .tool_name(),
        "opencode"
    );
    assert_eq!(
        Executor::Codex {
            model_override: None,
            thinking_budget: None,
            suppress_notify: false,
        }
        .tool_name(),
        "codex"
    );
    assert_eq!(
        Executor::ClaudeCode {
            model_override: None,
            thinking_budget: None,
        }
        .tool_name(),
        "claude-code"
    );
}

#[test]
fn test_executable_name() {
    assert_eq!(
        Executor::GeminiCli {
            model_override: None,
            thinking_budget: None,
        }
        .executable_name(),
        "gemini"
    );
    assert_eq!(
        Executor::Opencode {
            model_override: None,
            agent: None,
            thinking_budget: None,
        }
        .executable_name(),
        "opencode"
    );
    assert_eq!(
        Executor::Codex {
            model_override: None,
            thinking_budget: None,
            suppress_notify: false,
        }
        .executable_name(),
        "codex"
    );
    assert_eq!(
        Executor::ClaudeCode {
            model_override: None,
            thinking_budget: None,
        }
        .executable_name(),
        "claude"
    );
}

#[test]
fn test_install_hint() {
    assert_eq!(
        Executor::GeminiCli {
            model_override: None,
            thinking_budget: None,
        }
        .install_hint(),
        "Install: npm install -g @anthropic-ai/gemini-cli"
    );
    assert_eq!(
        Executor::Opencode {
            model_override: None,
            agent: None,
            thinking_budget: None,
        }
        .install_hint(),
        "Install: go install github.com/anthropics/opencode@latest"
    );
    assert_eq!(
        Executor::Codex {
            model_override: None,
            thinking_budget: None,
            suppress_notify: false,
        }
        .install_hint(),
        "Install: npm install -g @openai/codex"
    );
    assert_eq!(
        Executor::ClaudeCode {
            model_override: None,
            thinking_budget: None,
        }
        .install_hint(),
        "Install: npm install -g @anthropic-ai/claude-code"
    );
}

#[test]
fn test_yolo_args() {
    assert_eq!(
        Executor::GeminiCli {
            model_override: None,
            thinking_budget: None,
        }
        .yolo_args(),
        &["-y"]
    );
    assert_eq!(
        Executor::Opencode {
            model_override: None,
            agent: None,
            thinking_budget: None,
        }
        .yolo_args(),
        &[] as &[&str] // opencode does not have a yolo mode
    );
    assert_eq!(
        Executor::Codex {
            model_override: None,
            thinking_budget: None,
            suppress_notify: false,
        }
        .yolo_args(),
        &["--dangerously-bypass-approvals-and-sandbox"]
    );
    assert_eq!(
        Executor::ClaudeCode {
            model_override: None,
            thinking_budget: None,
        }
        .yolo_args(),
        &["--dangerously-skip-permissions"]
    );
}

#[test]
fn test_from_tool_name() {
    let exec = Executor::from_tool_name(&ToolName::GeminiCli, Some("model-1".to_string()));
    assert_eq!(exec.tool_name(), "gemini-cli");
    assert!(matches!(
        exec,
        Executor::GeminiCli {
            model_override: Some(_),
            thinking_budget: None,
        }
    ));

    let exec = Executor::from_tool_name(&ToolName::Opencode, None);
    assert_eq!(exec.tool_name(), "opencode");
    assert!(matches!(
        exec,
        Executor::Opencode {
            model_override: None,
            agent: None,
            thinking_budget: None,
        }
    ));

    let exec = Executor::from_tool_name(&ToolName::Codex, Some("model-2".to_string()));
    assert_eq!(exec.tool_name(), "codex");
    assert!(matches!(
        exec,
        Executor::Codex {
            model_override: Some(_),
            thinking_budget: None,
            ..
        }
    ));

    let exec = Executor::from_tool_name(&ToolName::ClaudeCode, None);
    assert_eq!(exec.tool_name(), "claude-code");
    assert!(matches!(
        exec,
        Executor::ClaudeCode {
            model_override: None,
            thinking_budget: None,
        }
    ));
}

#[test]
fn test_from_spec() {
    let spec = ModelSpec::parse("opencode/google/gemini-2.5-pro/high").unwrap();
    let exec = Executor::from_spec(&spec).unwrap();
    assert_eq!(exec.tool_name(), "opencode");
    assert!(matches!(
        exec,
        Executor::Opencode {
            model_override: Some(_),
            agent: None,
            thinking_budget: Some(_),
        }
    ));

    let spec = ModelSpec::parse("codex/anthropic/claude-opus/medium").unwrap();
    let exec = Executor::from_spec(&spec).unwrap();
    assert_eq!(exec.tool_name(), "codex");
    assert!(matches!(
        exec,
        Executor::Codex {
            model_override: Some(_),
            thinking_budget: Some(_),
            ..
        }
    ));
}

#[test]
fn test_from_spec_unknown_tool() {
    let spec = ModelSpec::parse("unknown-tool/provider/model/high").unwrap();
    let result = Executor::from_spec(&spec);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("Unknown tool"));
}

#[test]
fn test_thinking_budget_in_gemini_cli_args() {
    use crate::model_spec::ThinkingBudget;
    let exec = Executor::GeminiCli {
        model_override: Some("gemini-3-pro".to_string()),
        thinking_budget: Some(ThinkingBudget::High),
    };

    let mut cmd = Command::new(exec.executable_name());
    exec.append_tool_args(&mut cmd, "test prompt", None);

    // Check that the command contains --thinking_budget 32768
    let debug_str = format!("{:?}", cmd);
    assert!(debug_str.contains("--thinking_budget"));
    assert!(debug_str.contains("32768"));
}

#[test]
fn test_thinking_budget_in_codex_args() {
    use crate::model_spec::ThinkingBudget;
    let exec = Executor::Codex {
        model_override: Some("gpt-5".to_string()),
        thinking_budget: Some(ThinkingBudget::Low),
        suppress_notify: false,
    };

    let mut cmd = Command::new(exec.executable_name());
    exec.append_tool_args(&mut cmd, "test prompt", None);

    // Check that the command contains --reasoning-effort low
    let debug_str = format!("{:?}", cmd);
    assert!(debug_str.contains("--reasoning-effort"));
    assert!(debug_str.contains("\"low\""));
}

#[test]
fn test_thinking_budget_from_spec_gemini() {
    let spec = ModelSpec::parse("gemini-cli/google/gemini-3-pro/high").unwrap();
    let exec = Executor::from_spec(&spec).unwrap();

    let mut cmd = Command::new(exec.executable_name());
    exec.append_tool_args(&mut cmd, "test prompt", None);

    let debug_str = format!("{:?}", cmd);
    assert!(debug_str.contains("--thinking_budget"));
    assert!(debug_str.contains("32768"));
}

#[test]
fn test_thinking_budget_from_spec_codex() {
    let spec = ModelSpec::parse("codex/openai/gpt-5/low").unwrap();
    let exec = Executor::from_spec(&spec).unwrap();

    let mut cmd = Command::new(exec.executable_name());
    exec.append_tool_args(&mut cmd, "test prompt", None);

    let debug_str = format!("{:?}", cmd);
    assert!(debug_str.contains("--reasoning-effort"));
    assert!(debug_str.contains("\"low\""));
}

#[test]
fn test_thinking_budget_custom_value() {
    use crate::model_spec::ThinkingBudget;
    let exec = Executor::ClaudeCode {
        model_override: Some("claude-opus".to_string()),
        thinking_budget: Some(ThinkingBudget::Custom(10000)),
    };

    let mut cmd = Command::new(exec.executable_name());
    exec.append_tool_args(&mut cmd, "test prompt", None);

    let debug_str = format!("{:?}", cmd);
    assert!(debug_str.contains("--thinking-budget"));
    assert!(debug_str.contains("10000"));
}

#[test]
fn test_apply_restrictions_allow_edit() {
    let exec = Executor::GeminiCli {
        model_override: None,
        thinking_budget: None,
    };

    let original_prompt = "Refactor the authentication module";
    let result = exec.apply_restrictions(original_prompt, true);

    // When edit is allowed, prompt should be unchanged
    assert_eq!(result, original_prompt);
}

#[test]
fn test_apply_restrictions_deny_edit() {
    let exec = Executor::GeminiCli {
        model_override: None,
        thinking_budget: None,
    };

    let original_prompt = "Analyze the authentication module";
    let result = exec.apply_restrictions(original_prompt, false);

    // When edit is denied, prompt should contain restriction message
    assert!(result.contains("IMPORTANT RESTRICTION"));
    assert!(result.contains("MUST NOT edit or modify any existing files"));
    assert!(result.contains("may only create new files"));
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
            suppress_notify: false,
        },
        Executor::ClaudeCode {
            model_override: None,
            thinking_budget: None,
        },
    ];

    let original_prompt = "Analyze code";
    for tool in tools {
        // Test that restriction works for all tool types
        let restricted = tool.apply_restrictions(original_prompt, false);
        assert!(restricted.contains("IMPORTANT RESTRICTION"));
        assert!(restricted.contains(original_prompt));

        // Test that allowing edit returns original prompt
        let allowed = tool.apply_restrictions(original_prompt, true);
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
    let debug_str = format!("{:?}", cmd);
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

        let debug_str = format!("{:?}", cmd);
        assert!(
            debug_str.contains(expected_variant),
            "Expected variant '{}' not found in command: {}",
            expected_variant,
            debug_str
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
            suppress_notify: false,
        },
        Executor::ClaudeCode {
            model_override: Some("claude-opus".to_string()),
            thinking_budget: Some(ThinkingBudget::Custom(10000)),
        },
        Executor::Opencode {
            model_override: Some("google/gemini-2.5-pro".to_string()),
            agent: None,
            thinking_budget: Some(ThinkingBudget::Medium),
        },
    ];

    for exec in &tools {
        let mut cmd = Command::new(exec.executable_name());
        exec.append_yolo_args(&mut cmd);
        exec.append_model_args(&mut cmd);
        exec.append_prompt_args(&mut cmd, "test prompt");

        let debug_str = format!("{:?}", cmd);

        // Every tool should include its model override
        match exec {
            Executor::GeminiCli { .. } => {
                assert!(
                    debug_str.contains("gemini-3-pro"),
                    "GeminiCli missing model: {debug_str}"
                );
                assert!(
                    debug_str.contains("--thinking_budget"),
                    "GeminiCli missing thinking: {debug_str}"
                );
            }
            Executor::Codex { .. } => {
                assert!(
                    debug_str.contains("gpt-5"),
                    "Codex missing model: {debug_str}"
                );
                assert!(
                    debug_str.contains("--reasoning-effort"),
                    "Codex missing thinking: {debug_str}"
                );
            }
            Executor::ClaudeCode { .. } => {
                assert!(
                    debug_str.contains("claude-opus"),
                    "ClaudeCode missing model: {debug_str}"
                );
                assert!(
                    debug_str.contains("--thinking-budget"),
                    "ClaudeCode missing thinking: {debug_str}"
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
        }
    }
}

#[test]
fn test_codex_suppress_notify_flag() {
    let exec = Executor::Codex {
        model_override: Some("gpt-5".to_string()),
        thinking_budget: None,
        suppress_notify: true,
    };

    let mut cmd = Command::new(exec.executable_name());
    exec.append_tool_args(&mut cmd, "test prompt", None);

    let debug_str = format!("{:?}", cmd);
    assert!(
        debug_str.contains("\"-c\""),
        "Should contain -c flag: {debug_str}"
    );
    assert!(
        debug_str.contains("\"notify=[]\""),
        "Should contain notify=[]: {debug_str}"
    );
}

#[test]
fn test_codex_suppress_notify_default_false() {
    let exec = Executor::Codex {
        model_override: None,
        thinking_budget: None,
        suppress_notify: false,
    };

    let mut cmd = Command::new(exec.executable_name());
    exec.append_tool_args(&mut cmd, "test prompt", None);

    let debug_str = format!("{:?}", cmd);
    assert!(
        !debug_str.contains("notify=[]"),
        "Should NOT contain notify=[] when suppress_notify=false: {debug_str}"
    );
}

#[test]
fn test_set_suppress_notify() {
    let mut exec = Executor::Codex {
        model_override: None,
        thinking_budget: None,
        suppress_notify: false,
    };

    exec.set_suppress_notify(true);

    let mut cmd = Command::new(exec.executable_name());
    exec.append_tool_args(&mut cmd, "test", None);

    let debug_str = format!("{:?}", cmd);
    assert!(
        debug_str.contains("notify=[]"),
        "After set_suppress_notify(true), should contain notify=[]: {debug_str}"
    );
}

#[test]
fn test_set_suppress_notify_noop_for_non_codex() {
    let mut exec = Executor::GeminiCli {
        model_override: None,
        thinking_budget: None,
    };

    // Should be a no-op, not panic
    exec.set_suppress_notify(true);

    let mut cmd = Command::new(exec.executable_name());
    exec.append_tool_args(&mut cmd, "test", None);

    let debug_str = format!("{:?}", cmd);
    assert!(
        !debug_str.contains("notify=[]"),
        "Non-codex should never have notify=[]: {debug_str}"
    );
}
