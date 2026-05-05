use super::*;
use std::collections::{HashMap, HashSet};
use weave::compiler::{FailAction, PlanStep, VariableDecl};

#[test]
fn safe_plan_name_normalizes_non_alphanumeric_characters() {
    assert_eq!(safe_plan_name("Dev2Merge Workflow"), "dev2merge_workflow");
    assert_eq!(safe_plan_name("mktd@2026!"), "mktd_2026");
}

#[test]
fn plan_journal_defaults_pipeline_source_for_legacy_json() {
    let raw = r#"{
        "schema_version": 1,
        "workflow_name": "dev2merge",
        "workflow_path": "/repo/patterns/dev2merge/workflow.toml",
        "status": "running",
        "vars": {},
        "completed_steps": [],
        "last_error": null
    }"#;

    let journal: PlanRunJournal = serde_json::from_str(raw).unwrap();

    assert_eq!(journal.pipeline_source, PLAN_PIPELINE_SOURCE_DIRECT);
}

#[test]
fn load_plan_resume_context_reads_running_journal() {
    let tmp = tempfile::tempdir().unwrap();
    let workflow_path = tmp.path().join("workflow.toml");
    std::fs::write(&workflow_path, "[workflow]\nname='test'\n").unwrap();

    let plan = ExecutionPlan {
        name: "test".into(),
        description: String::new(),
        variables: vec![VariableDecl {
            name: "FEATURE".into(),
            default: Some("default".into()),
        }],
        steps: vec![],
    };

    let journal_path = tmp.path().join("test.journal.json");
    let journal = PlanRunJournal {
        schema_version: PLAN_JOURNAL_SCHEMA_VERSION,
        workflow_name: "test".into(),
        workflow_path: normalize_path(&workflow_path),
        pipeline_source: default_plan_pipeline_source(),
        status: "running".into(),
        vars: HashMap::from([
            ("FEATURE".to_string(), "from-journal".to_string()),
            ("STEP_1_OUTPUT".to_string(), "cached".to_string()),
        ]),
        completed_steps: vec![1, 2],
        last_error: None,
        repo_head: Some("abc123".to_string()),
        repo_dirty: Some(false),
    };
    persist_plan_journal(&journal_path, &journal).unwrap();

    let cli_vars = HashMap::from([("FEATURE".to_string(), "from-cli".to_string())]);
    let repo_fingerprint = RepoFingerprint {
        head: Some("abc123".to_string()),
        dirty: Some(false),
    };
    let ctx = load_plan_resume_context(
        &plan,
        &workflow_path,
        &journal_path,
        &cli_vars,
        &repo_fingerprint,
        false,
    )
    .unwrap();

    assert!(ctx.resumed);
    assert!(ctx.completed_steps.contains(&1));
    assert!(ctx.completed_steps.contains(&2));
    assert_eq!(
        ctx.initial_vars.get("FEATURE").map(String::as_str),
        Some("from-cli")
    );
    assert_eq!(
        ctx.initial_vars.get("STEP_1_OUTPUT").map(String::as_str),
        Some("cached")
    );
}

#[test]
fn load_plan_resume_context_rejects_journal_when_repo_fingerprint_changed() {
    let tmp = tempfile::tempdir().unwrap();
    let workflow_path = tmp.path().join("workflow.toml");
    std::fs::write(&workflow_path, "[workflow]\nname='test'\n").unwrap();

    let plan = ExecutionPlan {
        name: "test".into(),
        description: String::new(),
        variables: vec![],
        steps: vec![],
    };

    let journal_path = tmp.path().join("test.journal.json");
    let journal = PlanRunJournal {
        schema_version: PLAN_JOURNAL_SCHEMA_VERSION,
        workflow_name: "test".into(),
        workflow_path: normalize_path(&workflow_path),
        pipeline_source: default_plan_pipeline_source(),
        status: "running".into(),
        vars: HashMap::from([("STEP_1_OUTPUT".to_string(), "cached".to_string())]),
        completed_steps: vec![1],
        last_error: None,
        repo_head: Some("abc123".to_string()),
        repo_dirty: Some(false),
    };
    persist_plan_journal(&journal_path, &journal).unwrap();

    let cli_vars = HashMap::new();
    let repo_fingerprint = RepoFingerprint {
        head: Some("def456".to_string()),
        dirty: Some(false),
    };
    let ctx = load_plan_resume_context(
        &plan,
        &workflow_path,
        &journal_path,
        &cli_vars,
        &repo_fingerprint,
        false,
    )
    .unwrap();

    assert!(!ctx.resumed);
    assert!(ctx.completed_steps.is_empty());
    assert!(!ctx.initial_vars.contains_key("STEP_1_OUTPUT"));
}

#[test]
fn load_plan_resume_context_requires_explicit_resume_for_manual_handoff() {
    let tmp = tempfile::tempdir().unwrap();
    let workflow_path = tmp.path().join("workflow.toml");
    std::fs::write(&workflow_path, "[workflow]\nname='test'\n").unwrap();

    let plan = ExecutionPlan {
        name: "test".into(),
        description: String::new(),
        variables: vec![],
        steps: vec![],
    };

    let journal_path = tmp.path().join("test.journal.json");
    let journal = PlanRunJournal {
        schema_version: PLAN_JOURNAL_SCHEMA_VERSION,
        workflow_name: "test".into(),
        workflow_path: normalize_path(&workflow_path),
        pipeline_source: default_plan_pipeline_source(),
        status: "manual-handoff".into(),
        vars: HashMap::from([("STEP_1_OUTPUT".to_string(), "cached".to_string())]),
        completed_steps: vec![1],
        last_error: Some("manual handoff required".to_string()),
        repo_head: Some("abc123".to_string()),
        repo_dirty: Some(false),
    };
    persist_plan_journal(&journal_path, &journal).unwrap();

    let repo_fingerprint = RepoFingerprint {
        head: Some("abc123".to_string()),
        dirty: Some(false),
    };
    let implicit = load_plan_resume_context(
        &plan,
        &workflow_path,
        &journal_path,
        &HashMap::new(),
        &repo_fingerprint,
        false,
    )
    .unwrap();
    assert!(
        !implicit.resumed,
        "manual handoff must not auto-resume without explicit --resume"
    );

    let explicit = load_plan_resume_context(
        &plan,
        &workflow_path,
        &journal_path,
        &HashMap::new(),
        &repo_fingerprint,
        true,
    )
    .unwrap();
    assert!(
        explicit.resumed,
        "manual handoff should resume when explicitly requested"
    );
}

#[test]
fn load_plan_resume_context_rejects_awaiting_user_journal_even_with_explicit_resume() {
    let tmp = tempfile::tempdir().unwrap();
    let workflow_path = tmp.path().join("workflow.toml");
    std::fs::write(&workflow_path, "[workflow]\nname='test'\n").unwrap();

    let plan = ExecutionPlan {
        name: "test".into(),
        description: String::new(),
        variables: vec![],
        steps: vec![],
    };

    let journal_path = tmp.path().join("test.journal.json");
    let journal = PlanRunJournal {
        schema_version: PLAN_JOURNAL_SCHEMA_VERSION,
        workflow_name: "test".into(),
        workflow_path: normalize_path(&workflow_path),
        pipeline_source: default_plan_pipeline_source(),
        status: "awaiting-user".into(),
        vars: HashMap::from([("STEP_1_OUTPUT".to_string(), "cached".to_string())]),
        completed_steps: vec![1],
        last_error: Some("awaiting user action".to_string()),
        repo_head: Some("abc123".to_string()),
        repo_dirty: Some(false),
    };
    persist_plan_journal(&journal_path, &journal).unwrap();

    let repo_fingerprint = RepoFingerprint {
        head: Some("abc123".to_string()),
        dirty: Some(false),
    };
    let ctx = load_plan_resume_context(
        &plan,
        &workflow_path,
        &journal_path,
        &HashMap::new(),
        &repo_fingerprint,
        true,
    )
    .unwrap();
    assert!(
        !ctx.resumed,
        "awaiting-user journals must force a fresh rerun after remediation"
    );
}

#[test]
fn detect_effective_repo_strips_credentials_from_https_origin() {
    let tmp = tempfile::tempdir().unwrap();
    let git_dir = tmp.path();
    let init = std::process::Command::new("git")
        .args(["init"])
        .current_dir(git_dir)
        .output()
        .unwrap();
    assert!(init.status.success());

    let add_origin = std::process::Command::new("git")
        .args([
            "remote",
            "add",
            "origin",
            "https://user:token@github.com/example/private-repo.git",
        ])
        .current_dir(git_dir)
        .output()
        .unwrap();
    assert!(add_origin.status.success());

    assert_eq!(
        detect_effective_repo(git_dir).as_deref(),
        Some("example/private-repo")
    );
}

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
fn parse_variables_rejects_invalid_variable_name() {
    let plan = ExecutionPlan {
        name: "test".into(),
        description: String::new(),
        variables: vec![],
        steps: vec![],
    };

    let err = parse_variables(&["BAD-NAME=value".into()], &plan);
    assert!(err.is_err());
    let message = err.unwrap_err().to_string();
    assert!(message.contains("[A-Za-z_][A-Za-z0-9_]*"));
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
fn extract_output_assignment_markers_parses_uppercase_assignments() {
    let output = r#"
CSA_VAR:BOT_UNAVAILABLE=true
CSA_VAR:FALLBACK_REVIEW_HAS_ISSUES=false
noise line
lowercase=value
"#;
    let allowlist = HashSet::from([
        "BOT_UNAVAILABLE".to_string(),
        "FALLBACK_REVIEW_HAS_ISSUES".to_string(),
    ]);
    let markers = extract_output_assignment_markers(output, &allowlist);
    assert_eq!(
        markers,
        vec![
            ("BOT_UNAVAILABLE".to_string(), "true".to_string()),
            (
                "FALLBACK_REVIEW_HAS_ISSUES".to_string(),
                "false".to_string()
            )
        ]
    );
}

#[test]
fn extract_output_assignment_markers_ignores_non_allowlisted_keys() {
    let output = r#"
CSA_VAR:BOT_UNAVAILABLE=true
CSA_VAR:PATH=/tmp/unsafe
"#;
    let allowlist = HashSet::from(["BOT_UNAVAILABLE".to_string()]);
    let markers = extract_output_assignment_markers(output, &allowlist);
    assert_eq!(
        markers,
        vec![("BOT_UNAVAILABLE".to_string(), "true".to_string())]
    );
}

#[test]
fn extract_output_assignment_markers_ignores_unprefixed_assignments() {
    let output = r#"
BOT_UNAVAILABLE=true
CSA_VAR:BOT_UNAVAILABLE=false
"#;
    let allowlist = HashSet::from(["BOT_UNAVAILABLE".to_string()]);
    let markers = extract_output_assignment_markers(output, &allowlist);
    assert_eq!(
        markers,
        vec![("BOT_UNAVAILABLE".to_string(), "false".to_string())]
    );
}

#[test]
fn is_assignment_marker_key_accepts_expected_format() {
    assert!(is_assignment_marker_key("BOT_UNAVAILABLE"));
    assert!(is_assignment_marker_key("_INTERNAL_FLAG1"));
    assert!(is_assignment_marker_key("bot_unavailable"));
    assert!(!is_assignment_marker_key("1BAD"));
    assert!(!is_assignment_marker_key("BAD-KEY"));
}

#[test]
fn should_inject_assignment_markers_only_for_bash_steps() {
    let bash_step = PlanStep {
        id: 1,
        title: "bash".into(),
        tool: Some("Bash".into()),
        prompt: String::new(),
        tier: None,
        depends_on: vec![],
        on_fail: FailAction::Abort,
        condition: None,
        loop_var: None,
        session: None,
    };
    let codex_step = PlanStep {
        id: 2,
        title: "codex".into(),
        tool: Some("codex".into()),
        prompt: String::new(),
        tier: None,
        depends_on: vec![],
        on_fail: FailAction::Abort,
        condition: None,
        loop_var: None,
        session: None,
    };
    let tier_only_step = PlanStep {
        id: 3,
        title: "tier-only".into(),
        tool: None,
        prompt: String::new(),
        tier: Some("tier-1-fast".into()),
        depends_on: vec![],
        on_fail: FailAction::Abort,
        condition: None,
        loop_var: None,
        session: None,
    };

    assert!(should_inject_assignment_markers(&bash_step));
    assert!(!should_inject_assignment_markers(&codex_step));
    assert!(!should_inject_assignment_markers(&tier_only_step));
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
fn resolve_step_tool_explicit_bash_returns_direct_bash() {
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
        session: None,
    };
    let target = resolve_step_tool(&step, None).unwrap();
    assert!(
        matches!(target, StepTarget::DirectBash),
        "tool=bash must resolve to DirectBash, not a CSA tool"
    );
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
        session: None,
    };
    let target = resolve_step_tool(&step, None).unwrap();
    assert!(matches!(
        target,
        StepTarget::CsaTool {
            tool_name: ToolName::Codex,
            ..
        }
    ));
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
        session: None,
    };
    let target = resolve_step_tool(&step, None).unwrap();
    assert!(matches!(
        target,
        StepTarget::CsaTool {
            tool_name: ToolName::Codex,
            ..
        }
    ));
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
        session: None,
    };
    let target = resolve_step_tool(&step, None).unwrap();
    assert!(matches!(target, StepTarget::WeaveInclude));
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
        session: None,
    };
    assert!(resolve_step_tool(&step, None).is_err());
}
