use super::*;
use crate::claude_runtime::{
    ClaudeCodeRuntimeMetadata, ClaudeCodeTransport, claude_runtime_metadata,
};
use crate::codex_runtime::{CodexRuntimeMetadata, CodexTransport, codex_runtime_metadata};

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
            runtime_metadata: codex_runtime_metadata(),
        }
        .tool_name(),
        "codex"
    );
    assert_eq!(
        Executor::ClaudeCode {
            model_override: None,
            thinking_budget: None,
            runtime_metadata: claude_runtime_metadata(),
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
            runtime_metadata: codex_runtime_metadata(),
        }
        .executable_name(),
        "codex"
    );
    assert_eq!(
        Executor::ClaudeCode {
            model_override: None,
            thinking_budget: None,
            runtime_metadata: claude_runtime_metadata(),
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
        "Install: npm install -g @google/gemini-cli"
    );
    assert_eq!(
        Executor::Opencode {
            model_override: None,
            agent: None,
            thinking_budget: None,
        }
        .install_hint(),
        "Install: go install github.com/sst/opencode@latest"
    );
    assert_eq!(
        Executor::Codex {
            model_override: None,
            thinking_budget: None,
            runtime_metadata: codex_runtime_metadata(),
        }
        .install_hint(),
        codex_runtime_metadata().install_hint()
    );
    assert_eq!(
        Executor::ClaudeCode {
            model_override: None,
            thinking_budget: None,
            runtime_metadata: claude_runtime_metadata(),
        }
        .install_hint(),
        claude_runtime_metadata().install_hint()
    );
}

#[test]
fn test_runtime_binary_name() {
    // Legacy tools: runtime binary = executable name
    assert_eq!(
        Executor::GeminiCli {
            model_override: None,
            thinking_budget: None,
        }
        .runtime_binary_name(),
        "gemini"
    );
    assert_eq!(
        Executor::Opencode {
            model_override: None,
            agent: None,
            thinking_budget: None,
        }
        .runtime_binary_name(),
        "opencode"
    );
    // ACP tools: runtime binary = ACP adapter
    assert_eq!(
        Executor::Codex {
            model_override: None,
            thinking_budget: None,
            runtime_metadata: codex_runtime_metadata(),
        }
        .runtime_binary_name(),
        codex_runtime_metadata().runtime_binary_name()
    );
    assert_eq!(
        Executor::ClaudeCode {
            model_override: None,
            thinking_budget: None,
            runtime_metadata: claude_runtime_metadata(),
        }
        .runtime_binary_name(),
        claude_runtime_metadata().runtime_binary_name()
    );
}

#[test]
fn test_claude_runtime_binary_name_honors_explicit_cli_runtime_metadata() {
    let executor = Executor::ClaudeCode {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: ClaudeCodeRuntimeMetadata::from_transport(ClaudeCodeTransport::Cli),
    };

    assert_eq!(executor.runtime_binary_name(), "claude");
    assert_eq!(
        executor.install_hint(),
        "Install Claude Code CLI and ensure `claude` is on PATH"
    );
}

#[test]
fn test_claude_runtime_binary_name_honors_explicit_acp_runtime_metadata() {
    let executor = Executor::ClaudeCode {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: ClaudeCodeRuntimeMetadata::from_transport(ClaudeCodeTransport::Acp),
    };

    assert_eq!(executor.runtime_binary_name(), "claude-code-acp");
    assert_eq!(
        executor.install_hint(),
        "Install ACP adapter: npm install -g @zed-industries/claude-code-acp"
    );
}

#[test]
fn test_codex_runtime_binary_name_honors_explicit_cli_runtime_metadata() {
    let executor = Executor::Codex {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: CodexRuntimeMetadata::from_transport(CodexTransport::Cli),
    };

    assert_eq!(executor.runtime_binary_name(), "codex");
    assert_eq!(
        executor.install_hint(),
        "Install: npm install -g @openai/codex"
    );
}

#[test]
fn test_codex_runtime_binary_name_honors_explicit_acp_runtime_metadata() {
    let executor = Executor::Codex {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: CodexRuntimeMetadata::from_transport(CodexTransport::Acp),
    };

    assert_eq!(executor.runtime_binary_name(), "codex-acp");
    assert_eq!(
        executor.install_hint(),
        "Install ACP adapter: npm install -g @zed-industries/codex-acp"
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
            runtime_metadata: codex_runtime_metadata(),
        }
        .yolo_args(),
        &["--dangerously-bypass-approvals-and-sandbox"]
    );
    assert_eq!(
        Executor::ClaudeCode {
            model_override: None,
            thinking_budget: None,
            runtime_metadata: claude_runtime_metadata(),
        }
        .yolo_args(),
        &["--dangerously-skip-permissions"]
    );
}

#[test]
fn test_from_tool_name() {
    let exec = Executor::from_tool_name(&ToolName::GeminiCli, Some("model-1".to_string()), None);
    assert_eq!(exec.tool_name(), "gemini-cli");
    assert!(matches!(
        exec,
        Executor::GeminiCli {
            model_override: Some(_),
            thinking_budget: None,
        }
    ));

    let exec = Executor::from_tool_name(&ToolName::Opencode, None, None);
    assert_eq!(exec.tool_name(), "opencode");
    assert!(matches!(
        exec,
        Executor::Opencode {
            model_override: None,
            agent: None,
            thinking_budget: None,
        }
    ));

    let exec = Executor::from_tool_name(&ToolName::Codex, Some("model-2".to_string()), None);
    assert_eq!(exec.tool_name(), "codex");
    assert!(matches!(
        exec,
        Executor::Codex {
            model_override: Some(_),
            thinking_budget: None,
            ..
        }
    ));

    let exec = Executor::from_tool_name(&ToolName::ClaudeCode, None, None);
    assert_eq!(exec.tool_name(), "claude-code");
    assert!(matches!(
        exec,
        Executor::ClaudeCode {
            model_override: None,
            thinking_budget: None,
            ..
        }
    ));
}

#[test]
fn test_from_tool_name_with_model_and_thinking() {
    let exec = Executor::from_tool_name(
        &ToolName::Codex,
        Some("gpt-5.1-codex-mini".to_string()),
        Some(ThinkingBudget::Low),
    );
    assert!(matches!(
        exec,
        Executor::Codex {
            model_override: Some(_),
            thinking_budget: Some(ThinkingBudget::Low),
            ..
        }
    ));

    let mut cmd = Command::new(exec.executable_name());
    exec.append_tool_args(&mut cmd, "test prompt", None);
    let debug_str = format!("{cmd:?}");
    assert!(
        debug_str.contains("gpt-5.1-codex-mini"),
        "Missing model in args: {debug_str}"
    );
    assert!(
        debug_str.contains("model_reasoning_effort="),
        "Missing model_reasoning_effort in args: {debug_str}"
    );
    assert!(
        debug_str.contains("model_reasoning_effort=low"),
        "Expected low reasoning effort: {debug_str}"
    );
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

    let spec = ModelSpec::parse("hermes/openai/gpt-5.5/xhigh").unwrap();
    let exec = Executor::from_spec(&spec).unwrap();
    assert_eq!(exec.tool_name(), "hermes");
    match exec {
        Executor::Hermes {
            provider_override,
            model_override,
            thinking_budget,
        } => {
            assert_eq!(provider_override.as_deref(), Some("openai"));
            assert_eq!(model_override.as_deref(), Some("gpt-5.5"));
            assert!(matches!(thinking_budget, Some(ThinkingBudget::Xhigh)));
        }
        other => panic!("expected Hermes executor, got {other:?}"),
    }
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

    // gemini-cli runtime no longer accepts thinking-budget flags.
    let debug_str = format!("{cmd:?}");
    assert!(!debug_str.contains("--thinking_budget"));
    assert!(!debug_str.contains("32768"));
}

#[test]
fn test_thinking_budget_in_codex_args() {
    use crate::model_spec::ThinkingBudget;
    let exec = Executor::Codex {
        model_override: Some("gpt-5".to_string()),
        thinking_budget: Some(ThinkingBudget::Low),
        runtime_metadata: codex_runtime_metadata(),
    };

    let mut cmd = Command::new(exec.executable_name());
    exec.append_tool_args(&mut cmd, "test prompt", None);

    // Check that the command contains -c model_reasoning_effort=low
    let debug_str = format!("{cmd:?}");
    assert!(debug_str.contains("model_reasoning_effort=low"));
}

include!("executor_tests_tail.rs");
