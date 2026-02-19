use super::*;
use weave::compiler::VariableDecl;

#[test]
fn parse_variables_uses_defaults() {
    let plan = ExecutionPlan {
        name: "test".into(),
        description: String::new(),
        variables: vec![
            VariableDecl {
                name: "FOO".into(),
                default: Some("bar".into()),
            },
            VariableDecl {
                name: "BAZ".into(),
                default: None,
            },
        ],
        steps: vec![],
    };

    let vars = parse_variables(&[], &plan).unwrap();
    assert_eq!(vars.get("FOO").map(String::as_str), Some("bar"));
    assert!(!vars.contains_key("BAZ"));
}

#[test]
fn parse_variables_cli_overrides_default() {
    let plan = ExecutionPlan {
        name: "test".into(),
        description: String::new(),
        variables: vec![VariableDecl {
            name: "FOO".into(),
            default: Some("default".into()),
        }],
        steps: vec![],
    };

    let vars = parse_variables(&["FOO=override".into()], &plan).unwrap();
    assert_eq!(vars.get("FOO").map(String::as_str), Some("override"));
}

#[test]
fn parse_variables_rejects_invalid_format() {
    let plan = ExecutionPlan {
        name: "test".into(),
        description: String::new(),
        variables: vec![],
        steps: vec![],
    };

    let err = parse_variables(&["NO_EQUALS_SIGN".into()], &plan);
    assert!(err.is_err());
}

#[test]
fn substitute_vars_replaces_placeholders() {
    let mut vars = HashMap::new();
    vars.insert("NAME".into(), "world".into());
    vars.insert("COUNT".into(), "42".into());

    assert_eq!(
        substitute_vars("Hello ${NAME}, count=${COUNT}!", &vars),
        "Hello world, count=42!"
    );
}

#[test]
fn substitute_vars_leaves_unknown_placeholders() {
    let vars = HashMap::new();
    assert_eq!(substitute_vars("${UNKNOWN}", &vars), "${UNKNOWN}");
}

#[test]
fn extract_bash_code_block_finds_bash_fence() {
    let prompt = "Run this:\n```bash\necho hello\n```\nDone.";
    assert_eq!(extract_bash_code_block(prompt), Some("echo hello"));
}

#[test]
fn extract_bash_code_block_finds_plain_fence() {
    let prompt = "```\nls -la\n```";
    assert_eq!(extract_bash_code_block(prompt), Some("ls -la"));
}

#[test]
fn extract_bash_code_block_returns_none_when_no_fence() {
    assert_eq!(extract_bash_code_block("just some text"), None);
}

#[test]
fn truncate_short_string() {
    assert_eq!(truncate("hello", 10), "hello");
}

#[test]
fn truncate_long_string() {
    let s = "a".repeat(100);
    let result = truncate(&s, 10);
    assert_eq!(result.len(), 13); // 10 chars + "..."
    assert!(result.ends_with("..."));
}

#[test]
fn resolve_step_tool_explicit_bash() {
    let step = PlanStep {
        id: 1,
        title: "test".into(),
        tool: Some("bash".into()),
        prompt: String::new(),
        tier: None,
        depends_on: vec![],
        on_fail: FailAction::Abort,
        condition: None,
        loop_var: None,
    };
    let (tool, spec) = resolve_step_tool(&step, None).unwrap();
    assert_eq!(tool, ToolName::ClaudeCode);
    assert_eq!(spec.as_deref(), Some("bash"));
}

#[test]
fn resolve_step_tool_explicit_codex() {
    let step = PlanStep {
        id: 1,
        title: "test".into(),
        tool: Some("codex".into()),
        prompt: String::new(),
        tier: None,
        depends_on: vec![],
        on_fail: FailAction::Abort,
        condition: None,
        loop_var: None,
    };
    let (tool, _) = resolve_step_tool(&step, None).unwrap();
    assert_eq!(tool, ToolName::Codex);
}

#[test]
fn resolve_step_tool_fallback_no_config() {
    let step = PlanStep {
        id: 1,
        title: "test".into(),
        tool: None,
        prompt: String::new(),
        tier: None,
        depends_on: vec![],
        on_fail: FailAction::Abort,
        condition: None,
        loop_var: None,
    };
    let (tool, _) = resolve_step_tool(&step, None).unwrap();
    assert_eq!(tool, ToolName::Codex);
}

#[test]
fn resolve_step_tool_weave_returns_include_marker() {
    let step = PlanStep {
        id: 1,
        title: "include".into(),
        tool: Some("weave".into()),
        prompt: String::new(),
        tier: None,
        depends_on: vec![],
        on_fail: FailAction::Abort,
        condition: None,
        loop_var: None,
    };
    let (_tool, spec) = resolve_step_tool(&step, None).unwrap();
    assert_eq!(spec.as_deref(), Some("weave-include"));
}

#[test]
fn resolve_step_tool_unknown_tool_errors() {
    let step = PlanStep {
        id: 1,
        title: "test".into(),
        tool: Some("nonexistent".into()),
        prompt: String::new(),
        tier: None,
        depends_on: vec![],
        on_fail: FailAction::Abort,
        condition: None,
        loop_var: None,
    };
    assert!(resolve_step_tool(&step, None).is_err());
}

#[tokio::test]
async fn execute_step_skips_condition_with_nonzero_exit() {
    let step = PlanStep {
        id: 1,
        title: "conditional".into(),
        tool: Some("bash".into()),
        prompt: "echo test".into(),
        tier: None,
        depends_on: vec![],
        on_fail: FailAction::Abort,
        condition: Some("${SOME_VAR}".into()),
        loop_var: None,
    };
    let vars = HashMap::new();
    let tmp = tempfile::tempdir().unwrap();
    let result = execute_step(&step, &vars, tmp.path(), None).await;
    assert!(result.skipped);
    assert_ne!(
        result.exit_code, 0,
        "unsupported skip must not masquerade as success"
    );
}

#[tokio::test]
async fn execute_step_skips_loop_with_nonzero_exit() {
    let step = PlanStep {
        id: 1,
        title: "loop".into(),
        tool: Some("bash".into()),
        prompt: "echo test".into(),
        tier: None,
        depends_on: vec![],
        on_fail: FailAction::Abort,
        condition: None,
        loop_var: Some(weave::compiler::LoopSpec {
            variable: "item".into(),
            collection: "${ITEMS}".into(),
            max_iterations: 10,
        }),
    };
    let vars = HashMap::new();
    let tmp = tempfile::tempdir().unwrap();
    let result = execute_step(&step, &vars, tmp.path(), None).await;
    assert!(result.skipped);
    assert_ne!(result.exit_code, 0);
}

#[tokio::test]
async fn execute_step_skips_weave_include() {
    let step = PlanStep {
        id: 1,
        title: "include security-audit".into(),
        tool: Some("weave".into()),
        prompt: "INCLUDE security-audit".into(),
        tier: None,
        depends_on: vec![],
        on_fail: FailAction::Abort,
        condition: None,
        loop_var: None,
    };
    let vars = HashMap::new();
    let tmp = tempfile::tempdir().unwrap();
    let result = execute_step(&step, &vars, tmp.path(), None).await;
    assert!(result.skipped);
    assert_eq!(
        result.exit_code, 0,
        "INCLUDE skip should be success (harmless)"
    );
}

#[tokio::test]
async fn execute_step_bash_runs_code_block() {
    let step = PlanStep {
        id: 1,
        title: "echo test".into(),
        tool: Some("bash".into()),
        prompt: "Run this:\n```bash\necho hello\n```\n".into(),
        tier: None,
        depends_on: vec![],
        on_fail: FailAction::Abort,
        condition: None,
        loop_var: None,
    };
    let vars = HashMap::new();
    let tmp = tempfile::tempdir().unwrap();
    let result = execute_step(&step, &vars, tmp.path(), None).await;
    assert!(!result.skipped);
    assert_eq!(result.exit_code, 0);
}
