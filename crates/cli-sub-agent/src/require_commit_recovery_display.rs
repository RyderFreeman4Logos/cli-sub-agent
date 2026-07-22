use std::borrow::Cow;
use std::path::Path;

const CONTINUATION_PROMPT_PLACEHOLDER: &str = "CONTINUATION_PROMPT.md";

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

pub(crate) fn format_require_commit_recovery_lines_for_display_session(
    session_dir: &Path,
    display_session_id: &str,
    diagnostic: &csa_session::RequireCommitRecoveryDiagnostic,
) -> Vec<String> {
    let session_id = continuation_guidance_session_id(session_dir, display_session_id);
    format_require_commit_recovery_lines_for_session(session_id.as_ref(), diagnostic)
}

pub(crate) fn format_require_commit_recovery_lines_for_session(
    session_id: &str,
    diagnostic: &csa_session::RequireCommitRecoveryDiagnostic,
) -> Vec<String> {
    let mut lines = format_require_commit_recovery_lines(diagnostic);
    let guidance = build_require_commit_recovery_guidance(session_id, diagnostic);
    lines.push(guidance.recovery_note.clone());
    lines.push(format!(
        "Continuation command: {}",
        guidance.continuation_command
    ));
    lines.push(format!(
        "Continuation prompt guidance: {}",
        guidance.continuation_prompt
    ));
    lines
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RequireCommitRecoveryGuidance {
    pub(crate) recovery_note: String,
    pub(crate) continuation_command: String,
    pub(crate) continuation_prompt: String,
}

impl RequireCommitRecoveryGuidance {
    pub(crate) fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "recovery_note": self.recovery_note,
            "continuation_command": self.continuation_command,
            "continuation_prompt": self.continuation_prompt,
        })
    }
}

pub(crate) fn build_require_commit_recovery_guidance_for_display_session(
    session_dir: &Path,
    display_session_id: &str,
    diagnostic: &csa_session::RequireCommitRecoveryDiagnostic,
) -> RequireCommitRecoveryGuidance {
    let session_id = continuation_guidance_session_id(session_dir, display_session_id);
    build_require_commit_recovery_guidance(session_id.as_ref(), diagnostic)
}

pub(crate) fn build_require_commit_recovery_guidance(
    session_id: &str,
    diagnostic: &csa_session::RequireCommitRecoveryDiagnostic,
) -> RequireCommitRecoveryGuidance {
    let args = [
        "csa".to_string(),
        "run".to_string(),
        "--fork-from".to_string(),
        shell_token(session_id),
        "--require-commit".to_string(),
        "--sa-mode".to_string(),
        diagnostic
            .sa_mode
            .map_or_else(|| "<true|false>".to_string(), |value| value.to_string()),
        "--prompt-file".to_string(),
        CONTINUATION_PROMPT_PLACEHOLDER.to_string(),
    ];
    RequireCommitRecoveryGuidance {
        recovery_note: recovery_note(diagnostic),
        continuation_command: args.join(" "),
        continuation_prompt: continuation_prompt_guidance(diagnostic),
    }
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

fn recovery_note(diagnostic: &csa_session::RequireCommitRecoveryDiagnostic) -> String {
    if diagnostic.dirty_worktree && !diagnostic.commit_created {
        return "Work was applied but not committed; use fork-from to continue from this session before changing the worktree.".to_string();
    }
    if diagnostic.dirty_worktree {
        return "Additional tracked work remains uncommitted after the session commit; use fork-from to inspect and finish it.".to_string();
    }
    if !diagnostic.commit_created {
        return "No commit satisfying --require-commit was recorded; use fork-from to inspect and continue this session.".to_string();
    }
    "The require-commit contract failed; use fork-from to inspect and continue this session."
        .to_string()
}

fn continuation_prompt_guidance(
    diagnostic: &csa_session::RequireCommitRecoveryDiagnostic,
) -> String {
    if diagnostic.dirty_worktree {
        return "Inspect git status --short, git diff, and git diff --staged; preserve the listed worktree changes; finish verification and create the missing commit; do not restart from scratch.".to_string();
    }
    "Inspect the previous result, determine why --require-commit was not satisfied, then continue with the smallest commit-producing fix.".to_string()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovery_lines_include_contract_state_and_paths_only() {
        let lines = format_require_commit_recovery_lines_for_session(
            "01KW641KP78VR43SCKJVN6HGDN",
            &csa_session::RequireCommitRecoveryDiagnostic {
                require_commit: true,
                sa_mode: Some(false),
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
            },
        );

        let rendered = lines.join("\n");
        assert!(rendered.contains("CONTRACT FAILURE"));
        assert!(rendered.contains("dirty_tracked_worktree=true"));
        assert!(rendered.contains("commit_created=false"));
        assert!(rendered.contains("status=signal"));
        assert!(rendered.contains("signal=15"));
        assert!(rendered.contains("Dirty tracked paths: src/lib.rs, README.md (+1 more)"));
        assert!(rendered.contains("Blocker: gate=commit-policy-uncommitted"));
        assert!(rendered.contains(
            "Work was applied but not committed; use fork-from to continue from this session"
        ));
        assert!(rendered.contains(
            "Continuation command: csa run --fork-from 01KW641KP78VR43SCKJVN6HGDN --require-commit --sa-mode false --prompt-file CONTINUATION_PROMPT.md"
        ));
        assert!(rendered.contains("git status --short"));
        assert!(!rendered.contains("<continuation"));
        assert!(!rendered.contains("file contents"));
    }

    #[test]
    fn recovery_guidance_uses_target_session_for_resume_wrapper_display() {
        let temp = tempfile::tempdir().expect("tempdir");
        let wrapper_id = csa_session::new_session_id();
        let worker_id = csa_session::new_session_id();
        let worker_dir = temp.path().join(&worker_id);
        std::fs::create_dir_all(&worker_dir).expect("worker dir");
        let mut diagnostic = csa_session::RequireCommitRecoveryDiagnostic {
            require_commit: true,
            sa_mode: Some(false),
            commit_created: false,
            dirty_worktree: true,
            changed_paths: vec!["src/lib.rs".to_string()],
            changed_paths_truncated: 0,
            termination_status: "failure".to_string(),
            exit_code: 1,
            termination_signal: None,
            kill_hint: None,
            blocker_summary: None,
            suggested_recovery_action: "inspect_changed_paths_then_commit_or_revert".to_string(),
        };

        let guidance = build_require_commit_recovery_guidance_for_display_session(
            &worker_dir,
            &wrapper_id,
            &diagnostic,
        );

        assert!(
            guidance
                .continuation_command
                .contains(&format!("--fork-from {worker_id}")),
            "continuation command must target worker session: {guidance:?}"
        );
        assert!(
            !guidance
                .continuation_command
                .contains(&format!("--fork-from {wrapper_id}")),
            "continuation command must not target wrapper session: {guidance:?}"
        );
        assert!(guidance.continuation_command.contains("--sa-mode false"));

        diagnostic.sa_mode = None;
        assert!(
            build_require_commit_recovery_guidance("01TESTLEGACY", &diagnostic)
                .continuation_command
                .contains("--sa-mode <true|false>")
        );
    }
}
