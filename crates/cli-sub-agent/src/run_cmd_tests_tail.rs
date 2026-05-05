use super::*;
use csa_core::transport_events::StreamingMetadata;
use csa_core::types::OutputFormat;

#[test]
fn test_cli_auto_route_parses() {
    let cli = try_parse_cli(&["csa", "run", "--auto-route", "code", "prompt"]).unwrap();
    match cli.command {
        crate::cli::Commands::Run { auto_route, .. } => {
            assert_eq!(auto_route.as_deref(), Some("code"));
        }
        _ => panic!("expected Run command"),
    }
}

#[test]
fn test_cli_auto_route_conflicts_with_tier() {
    let result = try_parse_cli(&[
        "csa",
        "run",
        "--auto-route",
        "code",
        "--tier",
        "tier-2-standard",
        "prompt",
    ]);
    assert!(result.is_err(), "auto-route and tier should conflict");
}

#[test]
fn extract_executed_shell_commands_from_events_returns_execute_titles() {
    let events = vec![
        SessionEvent::ToolCallStarted {
            id: "call-1".to_string(),
            title: "git status".to_string(),
            kind: "Execute".to_string(),
        },
        SessionEvent::ToolCallStarted {
            id: "call-1b".to_string(),
            title: "Run tests".to_string(),
            kind: "Execute".to_string(),
        },
        SessionEvent::ToolCallStarted {
            id: "call-2".to_string(),
            title: "Read README".to_string(),
            kind: "Read".to_string(),
        },
        SessionEvent::ToolCallCompleted {
            id: "call-1".to_string(),
            status: "Completed".to_string(),
        },
    ];

    let commands = extract_executed_shell_commands_from_events(&events);
    assert_eq!(
        commands,
        vec!["git status".to_string(), "Run tests".to_string()]
    );
}

#[test]
fn extract_executed_shell_commands_prefers_incremental_metadata() {
    let mut metadata = StreamingMetadata::default();
    metadata.extracted_commands = vec!["git status".to_string(), "cargo test".to_string()];
    let events = vec![SessionEvent::ToolCallStarted {
        id: "call-1".to_string(),
        title: "Read README".to_string(),
        kind: "Read".to_string(),
    }];
    let commands = extract_executed_shell_commands(&metadata, &events);
    assert_eq!(
        commands,
        vec!["git status".to_string(), "cargo test".to_string()]
    );
}

#[test]
fn events_contain_execute_tool_calls_detects_execute_entries() {
    let events = vec![
        SessionEvent::ToolCallStarted {
            id: "call-1".to_string(),
            title: "Read README".to_string(),
            kind: "Read".to_string(),
        },
        SessionEvent::ToolCallStarted {
            id: "call-2".to_string(),
            title: "git status".to_string(),
            kind: "Execute".to_string(),
        },
    ];

    assert!(events_contain_execute_tool_calls(&events));
}

#[test]
fn execute_tool_calls_observed_uses_incremental_metadata() {
    let mut metadata = StreamingMetadata::default();
    metadata.has_execute_tool_calls = true;
    let events = vec![SessionEvent::ToolCallStarted {
        id: "call-1".to_string(),
        title: "Read README".to_string(),
        kind: "Read".to_string(),
    }];
    assert!(execute_tool_calls_observed(&metadata, &events));
}

#[test]
fn events_contain_execute_tool_calls_returns_false_without_execute_entries() {
    let events = vec![SessionEvent::ToolCallStarted {
        id: "call-1".to_string(),
        title: "Read README".to_string(),
        kind: "Read".to_string(),
    }];

    assert!(!events_contain_execute_tool_calls(&events));
}

#[test]
fn apply_post_run_commit_policy_overrides_summary_on_preexisting_failure() {
    let mut result = ExecutionResult {
        output: String::new(),
        summary: "tool failed".to_string(),
        exit_code: 2,
        ..Default::default()
    };
    let guard = PostRunCommitGuard {
        workspace_mutated: true,
        head_changed: false,
        changed_paths: vec!["src/lib.rs".to_string()],
    };

    apply_post_run_commit_policy(&mut result, &OutputFormat::Json, true, Some(&guard));

    assert_eq!(result.exit_code, 2);
    assert_eq!(
        result.summary,
        "post-run policy blocked: workspace mutated without commit"
    );
    assert!(
        result
            .stderr_output
            .contains("Original summary before commit policy: tool failed")
    );
}

#[test]
fn apply_unverifiable_commit_policy_overrides_summary_on_preexisting_failure() {
    let mut result = ExecutionResult {
        output: String::new(),
        summary: "transport failed".to_string(),
        exit_code: 7,
        ..Default::default()
    };

    apply_unverifiable_commit_policy(&mut result, &OutputFormat::Json, true);

    assert_eq!(result.exit_code, 7);
    assert_eq!(
        result.summary,
        "post-run policy blocked: unable to verify workspace mutation state"
    );
    assert!(
        result
            .stderr_output
            .contains("Original summary before commit policy: transport failed")
    );
}

#[test]
fn evaluate_post_run_commit_guard_detects_untracked_mutation_when_status_is_unchanged() {
    let before = GitWorkspaceSnapshot {
        head: Some("abc123".to_string()),
        status: "?? note.txt\n".to_string(),
        untracked_fingerprint: Some(10),
        ..Default::default()
    };
    let after = GitWorkspaceSnapshot {
        head: Some("abc123".to_string()),
        status: "?? note.txt\n".to_string(),
        untracked_fingerprint: Some(20),
        ..Default::default()
    };

    let guard = evaluate_post_run_commit_guard(Some(&before), Some(&after))
        .expect("untracked mutation should produce guard");
    assert!(guard.workspace_mutated);
}

#[test]
fn evaluate_post_run_commit_guard_detects_index_mutation_when_status_is_unchanged() {
    let before = GitWorkspaceSnapshot {
        head: Some("abc123".to_string()),
        status: "M  src/lib.rs\n".to_string(),
        tracked_index_fingerprint: Some(11),
        ..Default::default()
    };
    let after = GitWorkspaceSnapshot {
        head: Some("abc123".to_string()),
        status: "M  src/lib.rs\n".to_string(),
        tracked_index_fingerprint: Some(22),
        ..Default::default()
    };

    let guard = evaluate_post_run_commit_guard(Some(&before), Some(&after))
        .expect("index mutation should produce guard");
    assert!(guard.workspace_mutated);
}

#[test]
fn apply_post_run_commit_policy_does_not_fail_closed_when_head_changed() {
    let mut result = ExecutionResult {
        output: String::new(),
        summary: "ok".to_string(),
        ..Default::default()
    };
    let guard = PostRunCommitGuard {
        workspace_mutated: true,
        head_changed: true,
        changed_paths: vec!["src/lib.rs".to_string()],
    };

    apply_post_run_commit_policy(&mut result, &OutputFormat::Json, true, Some(&guard));

    assert_eq!(result.exit_code, 0);
    assert_eq!(result.summary, "ok");
    assert!(
        result
            .stderr_output
            .contains("run created commit(s) but still left uncommitted workspace mutations")
    );
}
