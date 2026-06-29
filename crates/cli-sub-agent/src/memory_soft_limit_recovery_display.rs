use std::borrow::Cow;
use std::path::Path;

const CONTINUATION_PROMPT_PLACEHOLDER: &str = "CONTINUATION_PROMPT.md";
const COMMIT_ONLY_RETRY_PROFILE: &str = "lightweight_commit_only_recovery";

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

pub(crate) fn format_memory_soft_limit_recovery_lines_for_session(
    session_id: &str,
    diagnostic: &csa_session::MemorySoftLimitRecoveryDiagnostic,
    kill_diagnostics: Option<&csa_session::KillDiagnosticReport>,
) -> Vec<String> {
    let mut lines = format_memory_soft_limit_recovery_lines(diagnostic);
    let guidance =
        build_memory_soft_limit_recovery_guidance(session_id, diagnostic, kill_diagnostics);
    lines.push(format!(
        "Continuation command: {}",
        guidance.continuation_command
    ));
    lines.push(format!(
        "Continuation prompt guidance: {}",
        guidance.continuation_prompt
    ));
    lines.push(format!("Retry guidance: {}", guidance.retry_guidance));
    lines
}

pub(crate) fn format_memory_soft_limit_recovery_lines_for_display_session(
    session_dir: &Path,
    display_session_id: &str,
    diagnostic: &csa_session::MemorySoftLimitRecoveryDiagnostic,
    kill_diagnostics: Option<&csa_session::KillDiagnosticReport>,
) -> Vec<String> {
    let session_id = continuation_guidance_session_id(session_dir, display_session_id);
    format_memory_soft_limit_recovery_lines_for_session(
        session_id.as_ref(),
        diagnostic,
        kill_diagnostics,
    )
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MemorySoftLimitRecoveryGuidance {
    pub(crate) continuation_command: String,
    pub(crate) continuation_prompt: String,
    pub(crate) retry_guidance: String,
}

impl MemorySoftLimitRecoveryGuidance {
    pub(crate) fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "continuation_command": self.continuation_command,
            "continuation_prompt": self.continuation_prompt,
            "retry_guidance": self.retry_guidance,
        })
    }
}

pub(crate) fn build_memory_soft_limit_recovery_guidance(
    session_id: &str,
    diagnostic: &csa_session::MemorySoftLimitRecoveryDiagnostic,
    kill_diagnostics: Option<&csa_session::KillDiagnosticReport>,
) -> MemorySoftLimitRecoveryGuidance {
    let mut args = vec![
        "csa".to_string(),
        "run".to_string(),
        "--fork-from".to_string(),
        shell_token(session_id),
    ];
    if diagnostic.dirty_worktree {
        args.push("--require-commit".to_string());
    }
    args.push("--build-jobs".to_string());
    args.push("1".to_string());
    if let Some(memory_max_mb) = retry_memory_max_mb(kill_diagnostics, diagnostic) {
        args.push("--memory-max-mb".to_string());
        args.push(memory_max_mb.to_string());
    }
    args.push("--prompt-file".to_string());
    args.push(CONTINUATION_PROMPT_PLACEHOLDER.to_string());

    MemorySoftLimitRecoveryGuidance {
        continuation_command: args.join(" "),
        continuation_prompt: continuation_prompt_guidance(diagnostic),
        retry_guidance: retry_guidance(kill_diagnostics, diagnostic),
    }
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
    if diagnostic.retry_profile.as_deref() == Some(COMMIT_ONLY_RETRY_PROFILE) {
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

fn continuation_prompt_guidance(
    diagnostic: &csa_session::MemorySoftLimitRecoveryDiagnostic,
) -> String {
    if diagnostic.dirty_worktree {
        return "Inspect git status --short, git diff, and git diff --staged; preserve existing staged and unstaged work; finish or commit only the salvaged partial work; do not restart from scratch.".to_string();
    }
    if diagnostic.commit_created {
        return "Inspect the recorded HEAD commit and continue only if that commit does not satisfy the original task.".to_string();
    }
    "Continue the original task from the prior session context with reduced parallelism or a higher feasible memory cap.".to_string()
}

fn retry_guidance(
    kill_diagnostics: Option<&csa_session::KillDiagnosticReport>,
    diagnostic: &csa_session::MemorySoftLimitRecoveryDiagnostic,
) -> String {
    let memory_context = memory_retry_context(kill_diagnostics);
    if diagnostic.retry_profile.as_deref() == Some(COMMIT_ONLY_RETRY_PROFILE) {
        return format!(
            "Use the fork-from command for low-RSS commit-only salvage first; avoid blind retry under the same memory cap. {memory_context}"
        );
    }
    format!(
        "Avoid blind retry under the same memory cap; use lower parallelism and only raise memory_max_mb when host RAM/reserve makes it safe. {memory_context}"
    )
}

fn memory_retry_context(kill_diagnostics: Option<&csa_session::KillDiagnosticReport>) -> String {
    let Some(kill_diagnostics) = kill_diagnostics else {
        return "No structured cap sample was available.".to_string();
    };
    let mut parts = Vec::new();
    if let Some(current_mb) = kill_diagnostics.current_mb {
        parts.push(format!("current_mb={current_mb}"));
    }
    if let Some(threshold_mb) = kill_diagnostics.threshold_mb {
        parts.push(format!("threshold_mb={threshold_mb}"));
    }
    if let Some(memory_max_mb) = kill_diagnostics.memory_max_mb {
        parts.push(format!("memory_max_mb={memory_max_mb}"));
    }
    if let Some(soft_limit_percent) = kill_diagnostics.soft_limit_percent {
        parts.push(format!("soft_limit_percent={soft_limit_percent}"));
    }
    if parts.is_empty() {
        "No structured cap sample was available.".to_string()
    } else {
        format!("Observed {}.", parts.join(", "))
    }
}

fn retry_memory_max_mb(
    kill_diagnostics: Option<&csa_session::KillDiagnosticReport>,
    diagnostic: &csa_session::MemorySoftLimitRecoveryDiagnostic,
) -> Option<u64> {
    if diagnostic.retry_profile.as_deref() == Some(COMMIT_ONLY_RETRY_PROFILE) {
        return None;
    }
    let report = kill_diagnostics?;
    let current = report.current_mb?;
    let percent = u64::from(report.soft_limit_percent?);
    if percent == 0 {
        return None;
    }
    let minimum = current.saturating_mul(100).div_ceil(percent);
    let suggested = minimum.saturating_add(512);
    match report.memory_max_mb {
        Some(cap) if suggested <= cap => None,
        _ => Some(suggested),
    }
}

pub(crate) fn build_memory_soft_limit_recovery_guidance_for_display_session(
    session_dir: &Path,
    display_session_id: &str,
    diagnostic: &csa_session::MemorySoftLimitRecoveryDiagnostic,
    kill_diagnostics: Option<&csa_session::KillDiagnosticReport>,
) -> MemorySoftLimitRecoveryGuidance {
    let session_id = continuation_guidance_session_id(session_dir, display_session_id);
    build_memory_soft_limit_recovery_guidance(session_id.as_ref(), diagnostic, kill_diagnostics)
}

fn continuation_guidance_session_id<'a>(
    session_dir: &'a Path,
    display_session_id: &'a str,
) -> Cow<'a, str> {
    crate::session_display_alias::alias_for_display_session(session_dir, display_session_id)
        .map_or(Cow::Borrowed(display_session_id), |alias| {
            Cow::Owned(alias.target_session_id)
        })
}

fn shell_token(value: &str) -> String {
    let safe = !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'/'));
    if safe {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', r"'\''"))
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
    fn session_recovery_lines_include_fork_from_command_and_retry_context() {
        let diagnostic = csa_session::MemorySoftLimitRecoveryDiagnostic {
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
        };
        let kill = csa_session::KillDiagnosticReport {
            source: "memory_soft_limit".to_string(),
            signal: Some(15),
            current_mb: Some(9626),
            threshold_mb: Some(9000),
            memory_max_mb: Some(10000),
            soft_limit_percent: Some(90),
            scope_name: Some("csa-codex-01KW641KP78VR43SCKJVN6HGDN.scope".to_string()),
        };

        let rendered = format_memory_soft_limit_recovery_lines_for_session(
            "01KW641KP78VR43SCKJVN6HGDN",
            &diagnostic,
            Some(&kill),
        )
        .join("\n");

        assert!(rendered.contains(
            "Continuation command: csa run --fork-from 01KW641KP78VR43SCKJVN6HGDN --require-commit --build-jobs 1 --prompt-file CONTINUATION_PROMPT.md"
        ));
        assert!(rendered.contains("preserve existing staged and unstaged work"));
        assert!(rendered.contains("low-RSS commit-only salvage"));
        assert!(rendered.contains("avoid blind retry under the same memory cap"));
        assert!(rendered.contains("current_mb=9626"));
        assert!(rendered.contains("threshold_mb=9000"));
        assert!(rendered.contains("memory_max_mb=10000"));
        assert!(rendered.contains("soft_limit_percent=90"));
        assert!(!rendered.contains("git reset --hard"));
        assert!(!rendered.contains("git checkout --"));
        assert!(!rendered.contains("git stash"));
        assert!(!rendered.contains("git clean"));
    }

    #[test]
    fn non_commit_only_recovery_command_suggests_higher_feasible_cap() {
        let diagnostic = csa_session::MemorySoftLimitRecoveryDiagnostic {
            outcome: "no_tracked_repo_side_effects".to_string(),
            commit_created: false,
            dirty_worktree: false,
            changed_paths: Vec::new(),
            changed_paths_truncated: 0,
            git_status_short: Vec::new(),
            git_status_short_truncated: 0,
            head_oid: None,
            head_summary: None,
            suggested_recovery_action: "rerun_with_more_memory_or_reduce_parallelism".to_string(),
            retry_profile: None,
        };
        let kill = csa_session::KillDiagnosticReport {
            source: "memory_soft_limit".to_string(),
            signal: Some(15),
            current_mb: Some(9626),
            threshold_mb: Some(9000),
            memory_max_mb: Some(10000),
            soft_limit_percent: Some(90),
            scope_name: None,
        };

        let guidance = build_memory_soft_limit_recovery_guidance(
            "01KW641KP78VR43SCKJVN6HGDN",
            &diagnostic,
            Some(&kill),
        );

        assert!(guidance.continuation_command.contains("--build-jobs 1"));
        assert!(
            guidance
                .continuation_command
                .contains("--memory-max-mb 11208")
        );
        assert!(guidance.retry_guidance.contains("lower parallelism"));
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
