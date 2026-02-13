use super::*;

// --- parse_tool_name tests ---

#[test]
fn parse_tool_name_gemini_cli() {
    let result = parse_tool_name("gemini-cli").unwrap();
    assert!(matches!(result, ToolName::GeminiCli));
}

#[test]
fn parse_tool_name_opencode() {
    let result = parse_tool_name("opencode").unwrap();
    assert!(matches!(result, ToolName::Opencode));
}

#[test]
fn parse_tool_name_codex() {
    let result = parse_tool_name("codex").unwrap();
    assert!(matches!(result, ToolName::Codex));
}

#[test]
fn parse_tool_name_claude_code() {
    let result = parse_tool_name("claude-code").unwrap();
    assert!(matches!(result, ToolName::ClaudeCode));
}

#[test]
fn parse_tool_name_unknown_tool_errors() {
    let result = parse_tool_name("unknown-tool");
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("Unknown tool"),
        "expected 'Unknown tool' in: {}",
        err_msg
    );
}

#[test]
fn parse_tool_name_empty_string_errors() {
    assert!(parse_tool_name("").is_err());
}

// --- validate_tasks tests ---

fn make_task(name: &str, tool: &str, depends_on: Vec<&str>) -> BatchTask {
    BatchTask {
        name: name.to_string(),
        tool: tool.to_string(),
        prompt: format!("do {}", name),
        mode: TaskMode::default(),
        depends_on: depends_on.into_iter().map(String::from).collect(),
        model: None,
    }
}

#[test]
fn validate_tasks_valid_independent_tasks() {
    let tasks = vec![
        make_task("a", "codex", vec![]),
        make_task("b", "codex", vec![]),
    ];
    assert!(validate_tasks(&tasks).is_ok());
}

#[test]
fn validate_tasks_valid_dependency_chain() {
    let tasks = vec![
        make_task("a", "codex", vec![]),
        make_task("b", "codex", vec!["a"]),
        make_task("c", "codex", vec!["b"]),
    ];
    assert!(validate_tasks(&tasks).is_ok());
}

#[test]
fn validate_tasks_duplicate_names_errors() {
    let tasks = vec![
        make_task("dup", "codex", vec![]),
        make_task("dup", "codex", vec![]),
    ];
    let result = validate_tasks(&tasks);
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(err_msg.contains("Duplicate task name"), "{}", err_msg);
}

#[test]
fn validate_tasks_missing_dependency_errors() {
    let tasks = vec![make_task("a", "codex", vec!["nonexistent"])];
    let result = validate_tasks(&tasks);
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(err_msg.contains("unknown task"), "{}", err_msg);
}

#[test]
fn validate_tasks_self_dependency_cycle_errors() {
    let tasks = vec![make_task("a", "codex", vec!["a"])];
    let result = validate_tasks(&tasks);
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("cycle") || err_msg.contains("Cycle"),
        "expected cycle error in: {}",
        err_msg
    );
}

#[test]
fn validate_tasks_two_node_cycle_errors() {
    let tasks = vec![
        make_task("a", "codex", vec!["b"]),
        make_task("b", "codex", vec!["a"]),
    ];
    let result = validate_tasks(&tasks);
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.to_lowercase().contains("cycle"),
        "expected cycle error in: {}",
        err_msg
    );
}

#[test]
fn validate_tasks_three_node_cycle_errors() {
    let tasks = vec![
        make_task("a", "codex", vec!["c"]),
        make_task("b", "codex", vec!["a"]),
        make_task("c", "codex", vec!["b"]),
    ];
    let result = validate_tasks(&tasks);
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.to_lowercase().contains("cycle"),
        "expected cycle error in: {}",
        err_msg
    );
}

#[test]
fn validate_tasks_empty_list_ok() {
    let tasks: Vec<BatchTask> = vec![];
    assert!(validate_tasks(&tasks).is_ok());
}

// --- build_execution_plan tests ---

#[test]
fn build_plan_single_task_one_level() {
    let tasks = vec![make_task("a", "codex", vec![])];
    let plan = build_execution_plan(&tasks).unwrap();
    assert_eq!(plan.len(), 1);
    assert_eq!(plan[0], vec!["a"]);
}

#[test]
fn build_plan_two_independent_tasks_same_level() {
    let tasks = vec![
        make_task("a", "codex", vec![]),
        make_task("b", "codex", vec![]),
    ];
    let plan = build_execution_plan(&tasks).unwrap();
    assert_eq!(plan.len(), 1);
    assert!(plan[0].contains(&"a".to_string()));
    assert!(plan[0].contains(&"b".to_string()));
}

#[test]
fn build_plan_linear_chain_three_levels() {
    let tasks = vec![
        make_task("a", "codex", vec![]),
        make_task("b", "codex", vec!["a"]),
        make_task("c", "codex", vec!["b"]),
    ];
    let plan = build_execution_plan(&tasks).unwrap();
    assert_eq!(plan.len(), 3);
    assert_eq!(plan[0], vec!["a"]);
    assert_eq!(plan[1], vec!["b"]);
    assert_eq!(plan[2], vec!["c"]);
}

#[test]
fn build_plan_diamond_dependency() {
    // a -> b, a -> c, b -> d, c -> d
    let tasks = vec![
        make_task("a", "codex", vec![]),
        make_task("b", "codex", vec!["a"]),
        make_task("c", "codex", vec!["a"]),
        make_task("d", "codex", vec!["b", "c"]),
    ];
    let plan = build_execution_plan(&tasks).unwrap();
    assert_eq!(plan.len(), 3);
    assert_eq!(plan[0], vec!["a"]);
    // b and c at same level
    assert!(plan[1].contains(&"b".to_string()));
    assert!(plan[1].contains(&"c".to_string()));
    assert_eq!(plan[2], vec!["d"]);
}

#[test]
fn build_plan_empty_tasks_empty_plan() {
    let tasks: Vec<BatchTask> = vec![];
    let plan = build_execution_plan(&tasks).unwrap();
    assert!(plan.is_empty());
}

// --- detect_cycle tests ---

#[test]
fn detect_cycle_no_cycle_returns_ok() {
    let tasks = vec![
        make_task("a", "codex", vec![]),
        make_task("b", "codex", vec!["a"]),
    ];
    let task_map: HashMap<&str, &BatchTask> = tasks.iter().map(|t| (t.name.as_str(), t)).collect();
    let result = detect_cycle("b", &task_map, &mut HashSet::new(), &mut Vec::new());
    assert!(result.is_ok());
}

#[test]
fn detect_cycle_direct_cycle_detected() {
    let tasks = vec![
        make_task("a", "codex", vec!["b"]),
        make_task("b", "codex", vec!["a"]),
    ];
    let task_map: HashMap<&str, &BatchTask> = tasks.iter().map(|t| (t.name.as_str(), t)).collect();
    let result = detect_cycle("a", &task_map, &mut HashSet::new(), &mut Vec::new());
    assert!(result.is_err());
}

// --- BatchTask deserialization tests ---

#[test]
fn batch_config_deserialize_minimal() {
    let toml_str = r#"
[[tasks]]
name = "lint"
tool = "codex"
prompt = "run lint"
"#;
    let config: BatchConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(config.tasks.len(), 1);
    assert_eq!(config.tasks[0].name, "lint");
    assert_eq!(config.tasks[0].mode, TaskMode::Sequential);
    assert!(config.tasks[0].depends_on.is_empty());
    assert!(config.tasks[0].model.is_none());
}

#[test]
fn batch_config_deserialize_with_dependencies_and_mode() {
    let toml_str = r#"
[[tasks]]
name = "build"
tool = "claude-code"
prompt = "build the project"

[[tasks]]
name = "test"
tool = "codex"
prompt = "run tests"
mode = "parallel"
depends_on = ["build"]
model = "gpt-5"
"#;
    let config: BatchConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(config.tasks.len(), 2);
    assert_eq!(config.tasks[1].mode, TaskMode::Parallel);
    assert_eq!(config.tasks[1].depends_on, vec!["build"]);
    assert_eq!(config.tasks[1].model.as_deref(), Some("gpt-5"));
}

#[test]
fn batch_config_deserialize_invalid_toml_errors() {
    let toml_str = "this is not valid toml [[[";
    let result = toml::from_str::<BatchConfig>(toml_str);
    assert!(result.is_err());
}
