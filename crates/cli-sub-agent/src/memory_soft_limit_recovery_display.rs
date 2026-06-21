pub(crate) fn format_memory_soft_limit_recovery_lines(
    diagnostic: &csa_session::MemorySoftLimitRecoveryDiagnostic,
) -> Vec<String> {
    let mut lines = vec![format!(
        "Memory-soft-limit recovery: outcome={} dirty_worktree={} commit_created={} changed_paths={}",
        diagnostic.outcome,
        diagnostic.dirty_worktree,
        diagnostic.commit_created,
        diagnostic.changed_paths.len(),
    )];
    if !diagnostic.changed_paths.is_empty() || diagnostic.changed_paths_truncated > 0 {
        lines.push(format!(
            "Changed paths: {}",
            format_changed_paths(
                &diagnostic.changed_paths,
                diagnostic.changed_paths_truncated
            )
        ));
    }
    if let Some(head) = format_head_commit(diagnostic) {
        lines.push(format!("Head commit: {head}"));
    }
    lines.push(format!(
        "Recovery action: {}",
        diagnostic.suggested_recovery_action
    ));
    lines
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

fn format_head_commit(
    diagnostic: &csa_session::MemorySoftLimitRecoveryDiagnostic,
) -> Option<String> {
    let oid = diagnostic.head_oid.as_deref()?.trim();
    if oid.is_empty() {
        return None;
    }
    let short_oid = oid.chars().take(12).collect::<String>();
    let summary = diagnostic
        .head_summary
        .as_deref()
        .map(str::trim)
        .filter(|summary| !summary.is_empty());
    match summary {
        Some(summary) => Some(format!("{short_oid} {summary}")),
        None => Some(short_oid),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovery_lines_include_bounded_commit_metadata() {
        let lines = format_memory_soft_limit_recovery_lines(
            &csa_session::MemorySoftLimitRecoveryDiagnostic {
                outcome: "clean_committed_work".to_string(),
                commit_created: true,
                dirty_worktree: false,
                changed_paths: Vec::new(),
                changed_paths_truncated: 0,
                head_oid: Some("1234567890abcdef".to_string()),
                head_summary: Some("fix session recovery".to_string()),
                suggested_recovery_action: "inspect_head_commit_then_continue".to_string(),
            },
        );

        let rendered = lines.join("\n");
        assert!(rendered.contains("outcome=clean_committed_work"));
        assert!(rendered.contains("commit_created=true"));
        assert!(rendered.contains("Head commit: 1234567890ab fix session recovery"));
        assert!(rendered.contains("Recovery action: inspect_head_commit_then_continue"));
    }
}
