use super::*;
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
fn apply_no_verify_commit_policy_allows_explicit_override_marker() {
    let mut result = ExecutionResult {
        output: "git commit -n -m \"feat: intentional\"\n".to_string(),
        summary: "ok".to_string(),
        ..Default::default()
    };
    let executed_shell_commands = vec!["git commit -n -m \"feat: intentional\"".to_string()];

    apply_no_verify_commit_policy(
        &mut result,
        &OutputFormat::Json,
        "- POLICY OVERRIDE: ALLOW_GIT_COMMIT_NO_VERIFY=1",
        &executed_shell_commands,
        !executed_shell_commands.is_empty(),
    );

    assert_eq!(result.exit_code, 0);
    assert_eq!(result.summary, "ok");
    assert!(result.stderr_output.is_empty());
}

#[test]
fn apply_no_verify_commit_policy_ignores_plain_text_mentions_without_execute_events() {
    let mut result = ExecutionResult {
        output: "I can mention `git commit --no-verify` in a plan, but did not execute it.\n"
            .to_string(),
        summary: "ok".to_string(),
        ..Default::default()
    };
    let executed_shell_commands = Vec::new();

    apply_no_verify_commit_policy(
        &mut result,
        &OutputFormat::Json,
        "normal prompt",
        &executed_shell_commands,
        !executed_shell_commands.is_empty(),
    );

    assert_eq!(result.exit_code, 0);
    assert_eq!(result.summary, "ok");
    assert!(result.stderr_output.is_empty());
}

#[test]
fn apply_no_verify_commit_policy_blocks_legacy_command_like_output_when_events_missing() {
    let mut result = ExecutionResult {
        output: "$ git commit --no-verify -m \"unsafe\"\n".to_string(),
        summary: "ok".to_string(),
        ..Default::default()
    };
    let executed_shell_commands = Vec::new();

    apply_no_verify_commit_policy(
        &mut result,
        &OutputFormat::Json,
        "normal prompt",
        &executed_shell_commands,
        !executed_shell_commands.is_empty(),
    );

    assert_eq!(result.exit_code, 1);
    assert_eq!(
        result.summary,
        "post-run policy blocked: forbidden git commit --no-verify detected"
    );
    assert!(result.stderr_output.contains("Matched commands:"));
    assert!(result.stderr_output.contains("git commit --no-verify"));
}

#[test]
fn apply_no_verify_commit_policy_ignores_markdown_code_fence_mentions() {
    let mut result = ExecutionResult {
        output: "Planned command:\n```bash\ngit commit --no-verify -m \"unsafe\"\n```\n"
            .to_string(),
        summary: "ok".to_string(),
        ..Default::default()
    };
    let executed_shell_commands = Vec::new();

    apply_no_verify_commit_policy(
        &mut result,
        &OutputFormat::Json,
        "normal prompt",
        &executed_shell_commands,
        !executed_shell_commands.is_empty(),
    );

    assert_eq!(result.exit_code, 0);
    assert_eq!(result.summary, "ok");
    assert!(result.stderr_output.is_empty());
}

#[test]
fn apply_no_verify_commit_policy_does_not_block_echo_mentions() {
    let mut result = ExecutionResult {
        output: String::new(),
        summary: "ok".to_string(),
        ..Default::default()
    };
    let executed_shell_commands = vec!["echo \"git commit --no-verify\"".to_string()];

    apply_no_verify_commit_policy(
        &mut result,
        &OutputFormat::Json,
        "normal prompt",
        &executed_shell_commands,
        !executed_shell_commands.is_empty(),
    );

    assert_eq!(result.exit_code, 0);
    assert_eq!(result.summary, "ok");
    assert!(result.stderr_output.is_empty());
}

#[test]
fn apply_no_verify_commit_policy_blocks_shell_wrapped_git_commit() {
    let mut result = ExecutionResult {
        output: String::new(),
        summary: "ok".to_string(),
        ..Default::default()
    };
    let executed_shell_commands =
        vec!["bash -lc \"git commit -n -m 'unsafe wrapper'\"".to_string()];

    apply_no_verify_commit_policy(
        &mut result,
        &OutputFormat::Json,
        "normal prompt",
        &executed_shell_commands,
        !executed_shell_commands.is_empty(),
    );

    assert_eq!(result.exit_code, 1);
    assert_eq!(
        result.summary,
        "post-run policy blocked: forbidden git commit --no-verify detected"
    );
    assert!(result.stderr_output.contains("Matched commands:"));
    assert!(result.stderr_output.contains("git commit -n"));
}

#[test]
fn apply_no_verify_commit_policy_blocks_shell_wrapped_commit_with_quoted_ampersand() {
    let mut result = ExecutionResult {
        output: String::new(),
        summary: "ok".to_string(),
        ..Default::default()
    };
    let executed_shell_commands = vec!["bash -lc \"git commit -m 'A & B' -n\"".to_string()];

    apply_no_verify_commit_policy(
        &mut result,
        &OutputFormat::Json,
        "normal prompt",
        &executed_shell_commands,
        !executed_shell_commands.is_empty(),
    );

    assert_eq!(result.exit_code, 1);
    assert_eq!(
        result.summary,
        "post-run policy blocked: forbidden git commit --no-verify detected"
    );
}

#[test]
fn apply_no_verify_commit_policy_blocks_git_global_option_form() {
    let mut result = ExecutionResult {
        output: String::new(),
        summary: "ok".to_string(),
        ..Default::default()
    };
    let executed_shell_commands = vec!["git -C /tmp/repo commit -n -m \"unsafe\"".to_string()];

    apply_no_verify_commit_policy(
        &mut result,
        &OutputFormat::Json,
        "normal prompt",
        &executed_shell_commands,
        !executed_shell_commands.is_empty(),
    );

    assert_eq!(result.exit_code, 1);
    assert_eq!(
        result.summary,
        "post-run policy blocked: forbidden git commit --no-verify detected"
    );
}

#[test]
fn apply_no_verify_commit_policy_blocks_prefixed_env_assignment_form() {
    let mut result = ExecutionResult {
        output: String::new(),
        summary: "ok".to_string(),
        ..Default::default()
    };
    let executed_shell_commands =
        vec!["GIT_AUTHOR_NAME=bot git commit -n -m \"unsafe\"".to_string()];

    apply_no_verify_commit_policy(
        &mut result,
        &OutputFormat::Json,
        "normal prompt",
        &executed_shell_commands,
        !executed_shell_commands.is_empty(),
    );

    assert_eq!(result.exit_code, 1);
    assert_eq!(
        result.summary,
        "post-run policy blocked: forbidden git commit --no-verify detected"
    );
}

#[test]
fn apply_no_verify_commit_policy_blocks_env_command_with_options() {
    let mut result = ExecutionResult {
        output: String::new(),
        summary: "ok".to_string(),
        ..Default::default()
    };
    let executed_shell_commands = vec!["env -i git commit --no-verify -m \"unsafe\"".to_string()];

    apply_no_verify_commit_policy(
        &mut result,
        &OutputFormat::Json,
        "normal prompt",
        &executed_shell_commands,
        !executed_shell_commands.is_empty(),
    );

    assert_eq!(result.exit_code, 1);
    assert_eq!(
        result.summary,
        "post-run policy blocked: forbidden git commit --no-verify detected"
    );
}

#[test]
fn apply_no_verify_commit_policy_blocks_sudo_command_with_options() {
    let mut result = ExecutionResult {
        output: String::new(),
        summary: "ok".to_string(),
        ..Default::default()
    };
    let executed_shell_commands = vec!["sudo -u root git commit -n -m \"unsafe\"".to_string()];

    apply_no_verify_commit_policy(
        &mut result,
        &OutputFormat::Json,
        "normal prompt",
        &executed_shell_commands,
        !executed_shell_commands.is_empty(),
    );

    assert_eq!(result.exit_code, 1);
    assert_eq!(
        result.summary,
        "post-run policy blocked: forbidden git commit --no-verify detected"
    );
}

#[test]
fn apply_no_verify_commit_policy_blocks_no_verify_when_message_contains_ampersand() {
    let mut result = ExecutionResult {
        output: String::new(),
        summary: "ok".to_string(),
        ..Default::default()
    };
    let executed_shell_commands = vec!["git commit -m \"A & B\" -n".to_string()];

    apply_no_verify_commit_policy(
        &mut result,
        &OutputFormat::Json,
        "normal prompt",
        &executed_shell_commands,
        !executed_shell_commands.is_empty(),
    );

    assert_eq!(result.exit_code, 1);
    assert_eq!(
        result.summary,
        "post-run policy blocked: forbidden git commit --no-verify detected"
    );
}

#[test]
fn apply_no_verify_commit_policy_blocks_short_flag_combinations_containing_n() {
    let mut result = ExecutionResult {
        output: String::new(),
        summary: "ok".to_string(),
        ..Default::default()
    };
    let executed_shell_commands = vec!["git commit -nq -m \"unsafe\"".to_string()];

    apply_no_verify_commit_policy(
        &mut result,
        &OutputFormat::Json,
        "normal prompt",
        &executed_shell_commands,
        !executed_shell_commands.is_empty(),
    );

    assert_eq!(result.exit_code, 1);
    assert_eq!(
        result.summary,
        "post-run policy blocked: forbidden git commit --no-verify detected"
    );
}

#[test]
fn apply_no_verify_commit_policy_blocks_short_gpg_sign_followed_by_no_verify_flag() {
    let mut result = ExecutionResult {
        output: String::new(),
        summary: "ok".to_string(),
        ..Default::default()
    };
    let executed_shell_commands = vec!["git commit -S -n -m \"unsafe\"".to_string()];

    apply_no_verify_commit_policy(
        &mut result,
        &OutputFormat::Json,
        "normal prompt",
        &executed_shell_commands,
        !executed_shell_commands.is_empty(),
    );

    assert_eq!(result.exit_code, 1);
    assert_eq!(
        result.summary,
        "post-run policy blocked: forbidden git commit --no-verify detected"
    );
}

#[test]
fn apply_no_verify_commit_policy_blocks_long_gpg_sign_followed_by_no_verify_flag() {
    let mut result = ExecutionResult {
        output: String::new(),
        summary: "ok".to_string(),
        ..Default::default()
    };
    let executed_shell_commands = vec!["git commit --gpg-sign -n -m \"unsafe\"".to_string()];

    apply_no_verify_commit_policy(
        &mut result,
        &OutputFormat::Json,
        "normal prompt",
        &executed_shell_commands,
        !executed_shell_commands.is_empty(),
    );

    assert_eq!(result.exit_code, 1);
    assert_eq!(
        result.summary,
        "post-run policy blocked: forbidden git commit --no-verify detected"
    );
}

#[test]
fn apply_no_verify_commit_policy_does_not_treat_message_values_as_no_verify_flags() {
    let mut result = ExecutionResult {
        output: String::new(),
        summary: "ok".to_string(),
        ..Default::default()
    };
    let executed_shell_commands = vec!["git commit -m \"-new release\"".to_string()];

    apply_no_verify_commit_policy(
        &mut result,
        &OutputFormat::Json,
        "normal prompt",
        &executed_shell_commands,
        !executed_shell_commands.is_empty(),
    );

    assert_eq!(result.exit_code, 0);
    assert_eq!(result.summary, "ok");
    assert!(result.stderr_output.is_empty());
}

#[test]
fn apply_no_verify_commit_policy_does_not_use_output_fallback_when_execute_events_are_present() {
    let mut result = ExecutionResult {
        output: "$ git commit --no-verify -m \"mentioned only\"\n".to_string(),
        summary: "ok".to_string(),
        ..Default::default()
    };
    let executed_shell_commands = vec!["git status".to_string()];

    apply_no_verify_commit_policy(
        &mut result,
        &OutputFormat::Json,
        "normal prompt",
        &executed_shell_commands,
        !executed_shell_commands.is_empty(),
    );

    assert_eq!(result.exit_code, 0);
    assert_eq!(result.summary, "ok");
    assert!(result.stderr_output.is_empty());
}

#[test]
fn apply_no_verify_commit_policy_does_not_block_plain_mentions_when_execute_events_are_present() {
    let mut result = ExecutionResult {
        output: "git commit --no-verify -m \"mentioned only\"\n".to_string(),
        summary: "ok".to_string(),
        ..Default::default()
    };
    let executed_shell_commands = vec!["git status".to_string()];

    apply_no_verify_commit_policy(
        &mut result,
        &OutputFormat::Json,
        "normal prompt",
        &executed_shell_commands,
        !executed_shell_commands.is_empty(),
    );

    assert_eq!(result.exit_code, 0);
    assert_eq!(result.summary, "ok");
    assert!(result.stderr_output.is_empty());
}

#[test]
fn apply_no_verify_commit_policy_skips_output_fallback_when_execute_event_seen_without_titles() {
    let mut result = ExecutionResult {
        output: "$ git commit --no-verify -m \"mentioned only\"\n".to_string(),
        summary: "ok".to_string(),
        ..Default::default()
    };
    let executed_shell_commands = Vec::new();

    apply_no_verify_commit_policy(
        &mut result,
        &OutputFormat::Json,
        "normal prompt",
        &executed_shell_commands,
        true,
    );

    assert_eq!(result.exit_code, 0);
    assert_eq!(result.summary, "ok");
    assert!(result.stderr_output.is_empty());
}

#[test]
fn apply_no_verify_commit_policy_ignores_markdown_quote_prefix_with_execute_events() {
    let mut result = ExecutionResult {
        output: "> git commit --no-verify -m \"quoted mention\"\n".to_string(),
        summary: "ok".to_string(),
        ..Default::default()
    };
    let executed_shell_commands = vec!["git status".to_string()];

    apply_no_verify_commit_policy(
        &mut result,
        &OutputFormat::Json,
        "normal prompt",
        &executed_shell_commands,
        !executed_shell_commands.is_empty(),
    );

    assert_eq!(result.exit_code, 0);
    assert_eq!(result.summary, "ok");
    assert!(result.stderr_output.is_empty());
}

#[test]
fn apply_no_verify_commit_policy_legacy_fallback_handles_env_assignment_prefix() {
    let mut result = ExecutionResult {
        output: "GIT_AUTHOR_NAME=bot git commit -n -m \"unsafe\"\n".to_string(),
        summary: "ok".to_string(),
        ..Default::default()
    };
    let executed_shell_commands = Vec::new();

    apply_no_verify_commit_policy(
        &mut result,
        &OutputFormat::Json,
        "normal prompt",
        &executed_shell_commands,
        !executed_shell_commands.is_empty(),
    );

    assert_eq!(result.exit_code, 1);
    assert_eq!(
        result.summary,
        "post-run policy blocked: forbidden git commit --no-verify detected"
    );
}

#[test]
fn apply_no_verify_commit_policy_blocks_shell_script_with_preceding_commands() {
    let mut result = ExecutionResult {
        output: String::new(),
        summary: "ok".to_string(),
        ..Default::default()
    };
    let executed_shell_commands =
        vec!["bash -lc \"echo pre; git commit -n -m unsafe\"".to_string()];

    apply_no_verify_commit_policy(
        &mut result,
        &OutputFormat::Json,
        "normal prompt",
        &executed_shell_commands,
        !executed_shell_commands.is_empty(),
    );

    assert_eq!(result.exit_code, 1);
    assert_eq!(
        result.summary,
        "post-run policy blocked: forbidden git commit --no-verify detected"
    );
}

#[test]
fn apply_no_verify_commit_policy_blocks_shell_script_without_space_after_separator() {
    let mut result = ExecutionResult {
        output: String::new(),
        summary: "ok".to_string(),
        ..Default::default()
    };
    let executed_shell_commands = vec!["bash -lc \"echo pre;git commit -n -m unsafe\"".to_string()];

    apply_no_verify_commit_policy(
        &mut result,
        &OutputFormat::Json,
        "normal prompt",
        &executed_shell_commands,
        !executed_shell_commands.is_empty(),
    );

    assert_eq!(result.exit_code, 1);
    assert_eq!(
        result.summary,
        "post-run policy blocked: forbidden git commit --no-verify detected"
    );
}

#[test]
fn apply_no_verify_commit_policy_does_not_cross_pipe_boundary_into_other_commands() {
    let mut result = ExecutionResult {
        output: String::new(),
        summary: "ok".to_string(),
        ..Default::default()
    };
    let executed_shell_commands = vec!["bash -lc \"git commit -m safe | grep -n foo\"".to_string()];

    apply_no_verify_commit_policy(
        &mut result,
        &OutputFormat::Json,
        "normal prompt",
        &executed_shell_commands,
        !executed_shell_commands.is_empty(),
    );

    assert_eq!(result.exit_code, 0);
    assert_eq!(result.summary, "ok");
    assert!(result.stderr_output.is_empty());
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
