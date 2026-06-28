use std::path::Path;

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
    if !diagnostic.git_status_short.is_empty() || diagnostic.git_status_short_truncated > 0 {
        lines.push(format!(
            "Git status --short: {}",
            format_changed_paths(
                &diagnostic.git_status_short,
                diagnostic.git_status_short_truncated
            )
        ));
    }
    if let Some(head) = format_head_commit(diagnostic) {
        lines.push(format!("Head commit: {head}"));
    }
    if let Some(profile) = diagnostic
        .retry_profile
        .as_deref()
        .filter(|profile| !profile.is_empty())
    {
        lines.push(format!("Retry profile: {profile}"));
    }
    lines.push(format!(
        "Recovery action: {}",
        diagnostic.suggested_recovery_action
    ));
    lines.extend(recovery_recipe_lines(diagnostic));
    lines
}

pub(crate) fn format_memory_soft_limit_context_lines(session_dir: &Path) -> Vec<String> {
    let mut context = vec![format!(
        "Recovery context: session_dir={}",
        session_dir.display()
    )];
    let Some(state) = read_session_state(session_dir) else {
        return context;
    };
    let project = repo_slug_from_project_path(&state.project_path)
        .unwrap_or_else(|| state.project_path.clone());
    let branch = state.branch.as_deref().unwrap_or("<unknown>");
    context.push(format!(
        "Recovery context: project={project} branch={branch}"
    ));
    context
}

fn recovery_recipe_lines(
    diagnostic: &csa_session::MemorySoftLimitRecoveryDiagnostic,
) -> Vec<String> {
    if !diagnostic.dirty_worktree {
        return vec![format!(
            "Recovery recipe: {}",
            clean_recovery_recipe(diagnostic)
        )];
    }

    let mut lines = vec![
        "Recovery recipe: inspect git status --short before acting; preserve staged and unstaged changes, and do not discard or reset this worktree.".to_string(),
        "Status note: two-letter entries such as MM mean staged and unstaged changes are both present for the same path.".to_string(),
    ];
    if diagnostic.retry_profile.as_deref() == Some("lightweight_commit_only_recovery") {
        lines.push(
            "Retry shape: use a lighter commit-only/require-commit recovery after inspection, or rerun with a feasible memory_max_mb/reserve.".to_string(),
        );
    } else {
        lines.push(
            "Retry shape: rerun with less parallel work or a feasible memory_max_mb/reserve after preserving the current worktree.".to_string(),
        );
    }
    lines
}

fn clean_recovery_recipe(diagnostic: &csa_session::MemorySoftLimitRecoveryDiagnostic) -> String {
    if diagnostic.commit_created {
        return "inspect the recorded HEAD commit before continuing; rerun only if the commit does not cover the intended work.".to_string();
    }
    "no tracked side effects were recorded; rerun with more memory headroom or reduced compile/test parallelism.".to_string()
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

fn read_session_state(session_dir: &Path) -> Option<csa_session::MetaSessionState> {
    let raw = std::fs::read_to_string(session_dir.join("state.toml")).ok()?;
    toml::from_str(&raw).ok()
}

fn repo_slug_from_project_path(project_path: &str) -> Option<String> {
    let normalized = project_path.replace('\\', "/");
    let from_github = normalized
        .split_once("/github/")
        .map(|(_, rest)| rest)
        .unwrap_or(normalized.as_str());
    let parts = from_github
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if parts.len() < 2 {
        return None;
    }
    let owner = parts[parts.len() - 2];
    let repo = parts[parts.len() - 1];
    Some(format!("{owner}/{repo}"))
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
                git_status_short: Vec::new(),
                git_status_short_truncated: 0,
                head_oid: Some("1234567890abcdef".to_string()),
                head_summary: Some("fix session recovery".to_string()),
                suggested_recovery_action: "inspect_head_commit_then_continue".to_string(),
                retry_profile: None,
            },
        );

        let rendered = lines.join("\n");
        assert!(rendered.contains("outcome=clean_committed_work"));
        assert!(rendered.contains("commit_created=true"));
        assert!(rendered.contains("Head commit: 1234567890ab fix session recovery"));
        assert!(rendered.contains("Recovery action: inspect_head_commit_then_continue"));
    }

    #[test]
    fn dirty_recovery_lines_include_status_and_commit_only_recipe() {
        let lines = format_memory_soft_limit_recovery_lines(
            &csa_session::MemorySoftLimitRecoveryDiagnostic {
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
            },
        );

        let rendered = lines.join("\n");
        assert!(rendered.contains("Git status --short: MM crates/verbatim-daemon/src/main.rs"));
        assert!(rendered.contains("Retry profile: lightweight_commit_only_recovery"));
        assert!(rendered.contains("preserve staged and unstaged changes"));
        assert!(rendered.contains("lighter commit-only/require-commit recovery"));
        assert!(!rendered.contains("git reset --hard"));
        assert!(!rendered.contains("git checkout --"));
        assert!(!rendered.contains("git stash"));
        assert!(!rendered.contains("git clean"));
    }

    #[test]
    fn context_lines_include_session_project_and_branch() {
        let temp = tempfile::tempdir().expect("tempdir");
        let session_dir = temp
            .path()
            .join("home/obj/.local/state/cli-sub-agent/home/obj/project/github/RyderFreeman4Logos/verbatim/sessions/01KW641KP78VR43SCKJVN6HGDN");
        std::fs::create_dir_all(&session_dir).expect("session dir");
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
        .expect("state");

        let rendered = format_memory_soft_limit_context_lines(&session_dir).join("\n");

        assert!(rendered.contains("01KW641KP78VR43SCKJVN6HGDN"));
        assert!(rendered.contains("/home/obj/.local/state/cli-sub-agent/"));
        assert!(rendered.contains("RyderFreeman4Logos/verbatim"));
        assert!(rendered.contains("feat/issue-160-ingest-stage-telemetry"));
    }
}
