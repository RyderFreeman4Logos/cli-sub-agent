#[test]
fn session_matches_interrupted_skill_requires_signal_and_skill_tag() {
    let mut session = test_session(
        "01KJTESTSIGTERMABCDE12345",
        Utc.with_ymd_and_hms(2026, 3, 1, 13, 10, 0)
            .single()
            .unwrap(),
        SessionPhase::Active,
    );
    session.description = Some(skill_session_description("dev2merge"));
    session.termination_reason = Some("sigterm".to_string());
    assert!(session_matches_interrupted_skill(&session, "dev2merge"));

    session.termination_reason = None;
    assert!(!session_matches_interrupted_skill(&session, "dev2merge"));

    session.termination_reason = Some("sigterm".to_string());
    session.description = Some(skill_session_description("mktd"));
    assert!(!session_matches_interrupted_skill(&session, "dev2merge"));
}

#[test]
fn resolve_run_timeout_seconds_defaults_for_pr_bot_skill() {
    assert_eq!(
        resolve_run_timeout_seconds(None, Some("pr-bot")),
        Some(DEFAULT_PR_BOT_TIMEOUT_SECS)
    );
}

#[test]
fn resolve_run_timeout_seconds_prefers_cli_override() {
    assert_eq!(
        resolve_run_timeout_seconds(Some(900), Some("pr-bot")),
        Some(900)
    );
}

#[test]
fn wall_timeout_seconds_from_error_parses_marker() {
    let err = anyhow::anyhow!("Execution interrupted by WALL_TIMEOUT timeout_secs=1234");
    assert_eq!(wall_timeout_seconds_from_error(&err), Some(1234));
}

#[test]
fn wall_timeout_seconds_from_error_returns_none_without_marker() {
    let err = anyhow::anyhow!("Execution interrupted by SIGTERM");
    assert_eq!(wall_timeout_seconds_from_error(&err), None);
}

#[test]
fn run_error_timeout_seconds_ignores_configured_timeout_without_marker() {
    let err = anyhow::anyhow!("--extra-writable validation failed: rejected paths [\"/ssd\"]");
    assert_eq!(run_error_timeout_seconds(&err, Some(1800)), None);
}

#[test]
fn run_error_timeout_seconds_preserves_real_timeout_marker() {
    let err = anyhow::anyhow!("Execution interrupted by WALL_TIMEOUT timeout_secs=1800");
    assert_eq!(run_error_timeout_seconds(&err, Some(1800)), Some(1800));
}

#[test]
fn evaluate_post_run_commit_guard_returns_none_when_workspace_clean() {
    let before = GitWorkspaceSnapshot {
        head: Some("abc123".to_string()),
        status: "".to_string(),
        ..Default::default()
    };
    let after = GitWorkspaceSnapshot {
        head: Some("abc123".to_string()),
        status: "".to_string(),
        ..Default::default()
    };

    let guard = evaluate_post_run_commit_guard(Some(&before), Some(&after));
    assert!(guard.is_none());
}

#[test]
fn evaluate_post_run_commit_guard_detects_mutation_without_commit() {
    let before = GitWorkspaceSnapshot {
        head: Some("abc123".to_string()),
        status: "".to_string(),
        ..Default::default()
    };
    let after = GitWorkspaceSnapshot {
        head: Some("abc123".to_string()),
        status: " M crates/cli-sub-agent/src/run_cmd.rs\n".to_string(),
        ..Default::default()
    };

    let guard = evaluate_post_run_commit_guard(Some(&before), Some(&after))
        .expect("dirty workspace should produce guard");
    assert!(guard.workspace_mutated);
    assert_eq!(
        guard.changed_paths,
        vec!["crates/cli-sub-agent/src/run_cmd.rs".to_string()]
    );
}

#[test]
fn evaluate_post_run_commit_guard_detects_mutation_when_head_changed() {
    let before = GitWorkspaceSnapshot {
        head: Some("abc123".to_string()),
        status: "".to_string(),
        ..Default::default()
    };
    let after = GitWorkspaceSnapshot {
        head: Some("def456".to_string()),
        status: " M Cargo.lock\n".to_string(),
        ..Default::default()
    };

    let guard = evaluate_post_run_commit_guard(Some(&before), Some(&after))
        .expect("dirty workspace should produce guard");
    assert!(guard.workspace_mutated);
}

#[test]
fn evaluate_post_run_commit_guard_returns_none_for_preexisting_dirty_workspace() {
    let before = GitWorkspaceSnapshot {
        head: Some("abc123".to_string()),
        status: " M Cargo.lock\n".to_string(),
        ..Default::default()
    };
    let after = GitWorkspaceSnapshot {
        head: Some("def456".to_string()),
        status: " M Cargo.lock\n".to_string(),
        ..Default::default()
    };

    let guard = evaluate_post_run_commit_guard(Some(&before), Some(&after));
    assert!(guard.is_none());
}

#[test]
fn format_post_run_commit_guard_message_includes_next_step_and_paths() {
    let guard = PostRunCommitGuard {
        workspace_mutated: true,
        head_changed: false,
        changed_paths: vec![
            "Cargo.lock".to_string(),
            "crates/cli-sub-agent/src/run_cmd.rs".to_string(),
        ],
    };

    let message = format_post_run_commit_guard_message(&guard, false);
    assert!(message.contains("WARNING"));
    assert!(message.contains("csa run --skill commit"));
    assert!(message.contains("Cargo.lock"));
    assert!(message.contains("uncommitted workspace mutations"));
}

#[test]
fn evaluate_post_run_commit_guard_detects_dirty_file_mutation_when_status_text_is_unchanged() {
    let before = GitWorkspaceSnapshot {
        head: Some("abc123".to_string()),
        status: " M crates/cli-sub-agent/src/run_cmd.rs\n".to_string(),
        tracked_worktree_fingerprint: Some(11),
        ..Default::default()
    };
    let after = GitWorkspaceSnapshot {
        head: Some("abc123".to_string()),
        status: " M crates/cli-sub-agent/src/run_cmd.rs\n".to_string(),
        tracked_worktree_fingerprint: Some(22),
        ..Default::default()
    };

    let guard = evaluate_post_run_commit_guard(Some(&before), Some(&after))
        .expect("dirty workspace should produce guard");
    assert!(guard.workspace_mutated);
}

#[test]
fn apply_post_run_commit_policy_sets_failure_when_policy_requires_commit() {
    let mut result = ExecutionResult {
        output: String::new(),
        stderr_output: String::new(),
        summary: "ok".to_string(),
        exit_code: 0,
        peak_memory_mb: None,
    };
    let guard = PostRunCommitGuard {
        workspace_mutated: true,
        head_changed: false,
        changed_paths: vec!["src/lib.rs".to_string()],
    };

    apply_post_run_commit_policy(&mut result, &OutputFormat::Json, true, Some(&guard));

    assert_eq!(result.exit_code, 1);
    assert_eq!(
        result.summary,
        "post-run policy blocked: workspace mutated without commit"
    );
    assert!(result.stderr_output.contains("ERROR"));
}

#[test]
fn apply_post_run_commit_policy_keeps_success_when_policy_disabled() {
    let mut result = ExecutionResult {
        output: String::new(),
        stderr_output: String::new(),
        summary: "ok".to_string(),
        exit_code: 0,
        peak_memory_mb: None,
    };
    let guard = PostRunCommitGuard {
        workspace_mutated: true,
        head_changed: false,
        changed_paths: vec!["src/lib.rs".to_string()],
    };

    apply_post_run_commit_policy(&mut result, &OutputFormat::Json, false, Some(&guard));

    assert_eq!(result.exit_code, 0);
    assert_eq!(result.summary, "ok");
    assert!(result.stderr_output.contains("WARNING"));
}

#[test]
fn changed_paths_from_status_parses_nul_terminated_entries_with_spaces_and_arrows() {
    let status = " M foo bar.txt\0 M docs/a -> b.txt\0";

    let paths = changed_paths_from_status(status, 8);
    assert_eq!(
        paths,
        vec!["foo bar.txt".to_string(), "docs/a -> b.txt".to_string()]
    );
}

#[test]
fn tracked_paths_from_status_uses_raw_paths_without_quote_artifacts() {
    let status = " M foo bar.txt\0?? new file.txt\0";

    let tracked = tracked_paths_from_status(status, |x, y| x != '?' && y != ' ');
    assert_eq!(tracked, vec!["foo bar.txt".to_string()]);
}

#[test]
fn apply_unverifiable_commit_policy_sets_failure_when_verification_is_unavailable() {
    let mut result = ExecutionResult {
        output: String::new(),
        stderr_output: String::new(),
        summary: "ok".to_string(),
        exit_code: 0,
        peak_memory_mb: None,
    };

    apply_unverifiable_commit_policy(&mut result, &OutputFormat::Json, true);

    assert_eq!(result.exit_code, 1);
    assert_eq!(
        result.summary,
        "post-run policy blocked: unable to verify workspace mutation state"
    );
    assert!(
        result
            .stderr_output
            .contains("strict commit policy could not verify")
    );
}

#[test]
fn apply_unverifiable_commit_policy_is_noop_when_verification_is_available() {
    let mut result = ExecutionResult {
        output: String::new(),
        stderr_output: String::new(),
        summary: "ok".to_string(),
        exit_code: 0,
        peak_memory_mb: None,
    };

    apply_unverifiable_commit_policy(&mut result, &OutputFormat::Json, false);

    assert_eq!(result.exit_code, 0);
    assert_eq!(result.summary, "ok");
    assert!(result.stderr_output.is_empty());
}

#[test]
fn is_post_run_commit_policy_block_detects_known_policy_summaries() {
    assert!(is_post_run_commit_policy_block(
        "post-run policy blocked: workspace mutated without commit"
    ));
    assert!(is_post_run_commit_policy_block(
        "post-run policy blocked: unable to verify workspace mutation state"
    ));
    assert!(is_post_run_commit_policy_block(
        "post-run policy blocked: forbidden git commit --no-verify detected"
    ));
    assert!(!is_post_run_commit_policy_block("other failure"));
}

#[test]
fn apply_no_verify_commit_policy_sets_failure_when_forbidden_flag_detected() {
    let mut result = ExecutionResult {
        output: "git commit --no-verify -m \"feat: unsafe\"\n".to_string(),
        stderr_output: String::new(),
        summary: "commit completed".to_string(),
        exit_code: 0,
        peak_memory_mb: None,
    };
    let executed_shell_commands = vec!["git commit --no-verify -m \"feat: unsafe\"".to_string()];

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
    assert!(
        result
            .stderr_output
            .contains("Original summary before commit policy: commit completed")
    );
    assert!(result.stderr_output.contains("Matched commands:"));
    assert!(result.stderr_output.contains("git commit --no-verify"));
}
