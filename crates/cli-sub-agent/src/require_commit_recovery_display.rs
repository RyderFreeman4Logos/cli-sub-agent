pub(crate) fn format_require_commit_recovery_lines(
    diagnostic: &csa_session::RequireCommitRecoveryDiagnostic,
) -> Vec<String> {
    let mut lines = vec![
        format!(
            "Require-commit recovery: CONTRACT FAILURE; dirty_tracked_worktree={} commit_created={} changed_paths={}{}",
            diagnostic.dirty_worktree,
            diagnostic.commit_created,
            diagnostic.changed_paths.len(),
            format_termination_suffix(diagnostic)
        ),
        format!(
            "Dirty tracked paths: {}",
            format_changed_paths(
                &diagnostic.changed_paths,
                diagnostic.changed_paths_truncated
            )
        ),
    ];
    if let Some(blocker_summary) = diagnostic
        .blocker_summary
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        lines.push(format!("Blocker: {blocker_summary}"));
    }
    lines.push(format!(
        "Recovery action: {}",
        diagnostic.suggested_recovery_action
    ));
    lines
}

fn format_termination_suffix(diagnostic: &csa_session::RequireCommitRecoveryDiagnostic) -> String {
    let mut parts = vec![
        format!("status={}", diagnostic.termination_status),
        format!("exit_code={}", diagnostic.exit_code),
    ];
    if let Some(signal) = diagnostic.termination_signal {
        parts.push(format!("signal={signal}"));
    }
    if let Some(kill_hint) = diagnostic
        .kill_hint
        .as_deref()
        .filter(|hint| !hint.is_empty())
    {
        parts.push(format!("kill_hint={kill_hint}"));
    }
    format!(" ({})", parts.join(", "))
}

fn format_changed_paths(paths: &[String], truncated: usize) -> String {
    if paths.is_empty() {
        return "<none recorded>".to_string();
    }
    let mut rendered = paths.join(", ");
    if truncated > 0 {
        rendered.push_str(&format!(" (+{truncated} more)"));
    }
    rendered
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovery_lines_include_contract_state_and_paths_only() {
        let lines =
            format_require_commit_recovery_lines(&csa_session::RequireCommitRecoveryDiagnostic {
                require_commit: true,
                commit_created: false,
                dirty_worktree: true,
                changed_paths: vec!["src/lib.rs".to_string(), "README.md".to_string()],
                changed_paths_truncated: 1,
                termination_status: "signal".to_string(),
                exit_code: 143,
                termination_signal: Some(15),
                kill_hint: Some("memory_pressure".to_string()),
                blocker_summary: Some("gate=commit-policy-uncommitted".to_string()),
                suggested_recovery_action: "inspect_changed_paths_then_commit_or_revert"
                    .to_string(),
            });

        let rendered = lines.join("\n");
        assert!(rendered.contains("CONTRACT FAILURE"));
        assert!(rendered.contains("dirty_tracked_worktree=true"));
        assert!(rendered.contains("commit_created=false"));
        assert!(rendered.contains("status=signal"));
        assert!(rendered.contains("signal=15"));
        assert!(rendered.contains("Dirty tracked paths: src/lib.rs, README.md (+1 more)"));
        assert!(rendered.contains("Blocker: gate=commit-policy-uncommitted"));
        assert!(!rendered.contains("file contents"));
    }
}
