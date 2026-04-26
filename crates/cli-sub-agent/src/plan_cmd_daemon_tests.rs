//! Tests for `plan_cmd_daemon`.

use super::*;
use crate::plan_cmd::PlanRunArgs;

fn make_args() -> PlanRunArgs {
    PlanRunArgs {
        file: Some("workflow.toml".to_string()),
        pattern: None,
        vars: vec![],
        tool_override: None,
        dry_run: false,
        chunked: false,
        resume: None,
        cd: None,
        current_depth: 0,
    }
}

#[test]
fn describe_uses_pattern_name_when_set() {
    let mut args = make_args();
    args.file = None;
    args.pattern = Some("dev2merge".to_string());
    assert_eq!(describe_plan_run(&args), "plan: dev2merge");
}

#[test]
fn describe_falls_back_to_file_path() {
    let args = make_args();
    assert_eq!(describe_plan_run(&args), "plan: workflow.toml");
}

#[test]
fn describe_handles_resume_form() {
    let mut args = make_args();
    args.file = None;
    args.resume = Some("/tmp/journal.json".to_string());
    assert_eq!(describe_plan_run(&args), "plan: --resume /tmp/journal.json");
}

#[test]
fn describe_unknown_when_no_source_provided() {
    let mut args = make_args();
    args.file = None;
    assert_eq!(describe_plan_run(&args), "plan: (unknown workflow)");
}

#[test]
fn forwarded_args_strip_through_plan_run() {
    let argv = vec![
        "csa".to_string(),
        "plan".to_string(),
        "run".to_string(),
        "patterns/dev2merge/workflow.toml".to_string(),
        "--sa-mode".to_string(),
        "true".to_string(),
        "--var".to_string(),
        "FEATURE_INPUT=test".to_string(),
    ];
    let forwarded = build_forwarded_plan_args(&argv);
    assert_eq!(
        forwarded,
        vec![
            "patterns/dev2merge/workflow.toml",
            "--sa-mode",
            "true",
            "--var",
            "FEATURE_INPUT=test",
        ]
    );
}

#[test]
fn forwarded_args_drop_foreground_flag() {
    let argv = vec![
        "csa".to_string(),
        "plan".to_string(),
        "run".to_string(),
        "--foreground".to_string(),
        "workflow.toml".to_string(),
    ];
    // The `--foreground` flag is the parent-only opt-out and must not be
    // forwarded to the daemon child (which IS the worker, not a re-spawn).
    let forwarded = build_forwarded_plan_args(&argv);
    assert_eq!(forwarded, vec!["workflow.toml"]);
}

#[test]
fn forwarded_args_handle_global_flags_before_plan() {
    let argv = vec![
        "csa".to_string(),
        "--format".to_string(),
        "json".to_string(),
        "plan".to_string(),
        "run".to_string(),
        "--pattern".to_string(),
        "dev2merge".to_string(),
    ];
    let forwarded = build_forwarded_plan_args(&argv);
    assert_eq!(forwarded, vec!["--pattern", "dev2merge"]);
}

#[test]
fn forwarded_args_empty_when_plan_missing() {
    let argv = vec!["csa".to_string(), "run".to_string()];
    assert!(build_forwarded_plan_args(&argv).is_empty());
}
