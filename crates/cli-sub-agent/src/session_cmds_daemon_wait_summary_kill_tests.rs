use super::*;

fn memory_report() -> csa_session::KillDiagnosticReport {
    csa_session::KillDiagnosticReport {
        source: "memory_soft_limit".to_string(),
        signal: Some(15),
        current_mb: Some(9626),
        threshold_mb: Some(9000),
        memory_max_mb: Some(10000),
        soft_limit_percent: Some(90),
        scope_name: Some("csa-codex-01KW641KP78VR43SCKJVN6HGDN.scope".to_string()),
    }
}

fn signal_result(summary: String) -> csa_session::SessionResult {
    let now = Utc::now();
    csa_session::SessionResult {
        post_exec_gate: None,
        status: "signal".to_string(),
        exit_code: 143,
        summary,
        tool: "codex".to_string(),
        original_tool: None,
        fallback_tool: None,
        fallback_reason: None,
        started_at: now,
        completed_at: now,
        events_count: 0,
        artifacts: Vec::new(),
        peak_memory_mb: None,
        kill_hint: Some("memory_soft_limit".to_string()),
        kill_diagnostics: Some(memory_report()),
        last_item: None,
        fallback_chain: None,
        ..Default::default()
    }
}

fn clean_committed_recovery() -> csa_session::MemorySoftLimitRecoveryDiagnostic {
    csa_session::MemorySoftLimitRecoveryDiagnostic {
        outcome: "clean_committed_work".to_string(),
        commit_created: true,
        dirty_worktree: false,
        changed_paths: Vec::new(),
        changed_paths_truncated: 0,
        git_status_short: Vec::new(),
        git_status_short_truncated: 0,
        head_oid: Some("1234567890abcdef".to_string()),
        head_summary: Some("fix session recovery".to_string()),
        suggested_recovery_action: "inspect_head_commit_then_continue".to_string(),
        retry_profile: None,
    }
}

#[test]
fn compact_summary_includes_signal_kill_hint() {
    let temp = tempfile::tempdir().expect("tempdir should be created");
    let diagnostic = "CSA diagnostic: signal kill hint: memory soft limit (termination_reason=signal, CSA memory monitor soft-limit event matched signal exit, current_mb=9626, threshold_mb=9000, memory_max_mb=10000, soft_limit_percent=90, scope_name=csa-codex-01KW641KP78VR43SCKJVN6HGDN.scope). CSA's memory monitor sent SIGTERM at the configured soft limit; increase resources.memory_max_mb or tools.<tool>.memory_max_mb, raise resources.soft_limit_percent only if safe, or reduce compile/test parallelism.";
    let result = signal_result(diagnostic.to_string());

    let summary = render_wait_result_summary(temp.path(), "01KW641KP78VR43SCKJVN6HGDN", &result);

    assert!(summary.contains("Kill hint: memory_soft_limit"));
    assert!(summary.contains("CSA diagnostic: signal kill hint: memory soft limit"));
    assert!(summary.contains("current_mb=9626"));
    assert!(summary.contains("threshold_mb=9000"));
    assert!(summary.contains("memory_max_mb=10000"));
    assert!(summary.contains("soft_limit_percent=90"));
}

#[test]
fn compact_json_includes_kill_diagnostics() {
    let temp = tempfile::tempdir().expect("tempdir should be created");
    let result = signal_result("process killed by signal 15 (SIGTERM)".to_string());

    let rendered = render_wait_result_json(temp.path(), "01TESTWAITKILLJSON", &result)
        .expect("wait result JSON should render");
    let value: serde_json::Value =
        serde_json::from_str(&rendered).expect("wait result JSON should parse");

    assert_eq!(value["kill_hint"], "memory_soft_limit");
    assert_eq!(value["kill_diagnostics"]["source"], "memory_soft_limit");
    assert_eq!(value["kill_diagnostics"]["current_mb"], 9626);
    assert_eq!(value["kill_diagnostics"]["threshold_mb"], 9000);
}

#[test]
fn compact_summary_and_json_include_memory_soft_limit_recovery() {
    let temp = tempfile::tempdir().expect("tempdir should be created");
    let mut result = signal_result("process killed by signal 15 (SIGTERM)".to_string());
    result.memory_soft_limit_recovery = Some(clean_committed_recovery());

    let summary = render_wait_result_summary(temp.path(), "01TESTWAITMEMREC", &result);

    assert!(summary.contains("Memory-soft-limit recovery: outcome=clean_committed_work"));
    assert!(summary.contains("commit_created=true"));
    assert!(summary.contains("Head commit: 1234567890ab fix session recovery"));
    assert!(summary.contains("Recovery action: inspect_head_commit_then_continue"));

    let rendered = render_wait_result_json(temp.path(), "01TESTWAITMEMRECJSON", &result)
        .expect("wait result JSON should render");
    let value: serde_json::Value =
        serde_json::from_str(&rendered).expect("wait result JSON should parse");

    assert_eq!(
        value["memory_soft_limit_recovery"]["outcome"],
        "clean_committed_work"
    );
    assert_eq!(
        value["memory_soft_limit_recovery"]["head_summary"],
        "fix session recovery"
    );
}

#[test]
fn compact_summary_includes_commit_gate_memory_soft_limit_recovery_recipe() {
    let temp = tempfile::tempdir().expect("tempdir should be created");
    let session_dir = temp
        .path()
        .join("home/obj/.local/state/cli-sub-agent/home/obj/project/github/RyderFreeman4Logos/verbatim/sessions/01KW641KP78VR43SCKJVN6HGDN");
    std::fs::create_dir_all(&session_dir).expect("session dir should be created");
    std::fs::write(
        session_dir.join("state.toml"),
        r#"
meta_session_id = "01KW641KP78VR43SCKJVN6HGDN"
project_path = "/home/obj/project/github/RyderFreeman4Logos/verbatim"
branch = "feat/issue-160-ingest-stage-telemetry"
created_at = "2026-06-01T00:00:00Z"
last_accessed = "2026-06-01T00:00:00Z"
"#,
    )
    .expect("state should be written");
    let mut result = signal_result(
        "CSA diagnostic: signal kill hint: memory soft limit (termination_reason=signal, CSA memory monitor soft-limit event matched signal exit, current_mb=9626, threshold_mb=9000, memory_max_mb=10000, soft_limit_percent=90, scope_name=csa-codex-01KW641KP78VR43SCKJVN6HGDN.scope). CSA's memory monitor sent SIGTERM at the configured soft limit.".to_string(),
    );
    result.status = "failure".to_string();
    result.exit_code = 1;
    result.require_commit_recovery = Some(csa_session::RequireCommitRecoveryDiagnostic {
        require_commit: true,
        commit_created: false,
        dirty_worktree: true,
        changed_paths: vec!["crates/verbatim-daemon/src/main.rs".to_string()],
        changed_paths_truncated: 0,
        termination_status: "signal".to_string(),
        exit_code: 143,
        termination_signal: Some(15),
        kill_hint: Some("memory_soft_limit".to_string()),
        blocker_summary: Some("gate=commit-policy-uncommitted".to_string()),
        suggested_recovery_action: "inspect_changed_paths_then_commit_or_revert".to_string(),
    });
    result.memory_soft_limit_recovery = Some(csa_session::MemorySoftLimitRecoveryDiagnostic {
        outcome: "dirty_or_staged_changes".to_string(),
        commit_created: false,
        dirty_worktree: true,
        changed_paths: vec!["crates/verbatim-daemon/src/main.rs".to_string()],
        changed_paths_truncated: 0,
        git_status_short: vec!["MM crates/verbatim-daemon/src/main.rs".to_string()],
        git_status_short_truncated: 0,
        head_oid: None,
        head_summary: None,
        suggested_recovery_action:
            "inspect_git_status_preserve_staged_unstaged_then_retry_lightweight_commit_recovery"
                .to_string(),
        retry_profile: Some("lightweight_commit_only_recovery".to_string()),
    });

    let summary = render_wait_result_summary(&session_dir, "01KW641KP78VR43SCKJVN6HGDN", &result);

    assert!(summary.contains("01KW641KP78VR43SCKJVN6HGDN"));
    assert!(summary.contains("/home/obj/.local/state/cli-sub-agent/"));
    assert!(summary.contains("RyderFreeman4Logos/verbatim"));
    assert!(summary.contains("feat/issue-160-ingest-stage-telemetry"));
    assert!(summary.contains("memory_soft_limit"));
    assert!(summary.contains("Exit code: 1"));
    assert!(summary.contains("Parent wait process exit code 1"));
    assert!(summary.contains("exit_code=143"));
    assert!(summary.contains("process exit code 1"));
    assert!(summary.contains("current_mb=9626"));
    assert!(summary.contains("threshold_mb=9000"));
    assert!(summary.contains("memory_max_mb=10000"));
    assert!(summary.contains("soft_limit_percent=90"));
    assert!(summary.contains("MM crates/verbatim-daemon/src/main.rs"));
    assert!(summary.contains("Retry profile: lightweight_commit_only_recovery"));
    assert!(summary.contains(
        "Continuation command: csa run --fork-from 01KW641KP78VR43SCKJVN6HGDN --require-commit --build-jobs 1 --prompt-file CONTINUATION_PROMPT.md"
    ));
    assert!(summary.contains("preserve existing staged and unstaged work"));
    assert!(summary.contains("low-RSS commit-only salvage"));
    assert!(summary.contains("avoid blind retry under the same memory cap"));
    assert!(summary.contains("git status --short"));
    assert!(summary.contains("preserve staged and unstaged changes"));
    assert!(summary.contains("lighter commit-only/require-commit recovery"));
    assert!(!summary.contains("git reset --hard"));
    assert!(!summary.contains("git checkout --"));
    assert!(!summary.contains("git stash"));
    assert!(!summary.contains("git clean"));

    let rendered = render_wait_result_json(&session_dir, "01KW641KP78VR43SCKJVN6HGDN", &result)
        .expect("wait result JSON should render");
    let value: serde_json::Value =
        serde_json::from_str(&rendered).expect("wait result JSON should parse");
    assert_eq!(value["status"], "failure");
    assert_eq!(value["exit_code"], 1);
    assert_eq!(
        value["require_commit_recovery"]["termination_status"],
        "signal"
    );
    assert_eq!(
        value["memory_soft_limit_recovery"]["git_status_short"][0],
        "MM crates/verbatim-daemon/src/main.rs"
    );
    assert_eq!(
        value["memory_soft_limit_recovery_guidance"]["continuation_command"],
        "csa run --fork-from 01KW641KP78VR43SCKJVN6HGDN --require-commit --build-jobs 1 --prompt-file CONTINUATION_PROMPT.md"
    );
    assert!(
        value["memory_soft_limit_recovery_guidance"]["retry_guidance"]
            .as_str()
            .is_some_and(|text| text.contains("avoid blind retry under the same memory cap"))
    );
}
