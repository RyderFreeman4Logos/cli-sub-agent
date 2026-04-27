//! PostRun integration for the optional jj sidecar journal.

use std::path::Path;

use csa_core::vcs::{JournalError, RevisionId, SnapshotJournal};
use csa_session::JjJournal;
use tracing::{info, warn};

pub(crate) const AUTO_SNAPSHOT_ENV: &str = "CSA_VCS_AUTO_SNAPSHOT";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PostRunJjSnapshotOutcome {
    ConfigOff,
    NoChangedPaths,
    NoJjDir,
    NonColocated,
    Snapshot { revision: RevisionId },
    Failed { message: String },
}

pub(crate) fn maybe_record_post_run_snapshot(
    project_root: &Path,
    session_dir: &Path,
    session_id: &str,
    tool_name: &str,
    changed_paths: &[String],
    result: &mut csa_process::ExecutionResult,
) -> PostRunJjSnapshotOutcome {
    let outcome = evaluate_post_run_snapshot(
        project_root,
        session_dir,
        session_id,
        tool_name,
        changed_paths,
    );
    match &outcome {
        PostRunJjSnapshotOutcome::ConfigOff => {
            info!("jj sidecar snapshot disabled by CSA_VCS_AUTO_SNAPSHOT");
        }
        PostRunJjSnapshotOutcome::NoChangedPaths => {
            info!("Skipping jj sidecar snapshot because PostRun changed_paths is empty");
        }
        PostRunJjSnapshotOutcome::NoJjDir => append_snapshot_notice(
            result,
            "jj sidecar snapshot skipped: .jj/ not found; git fallback is intentionally disabled",
        ),
        PostRunJjSnapshotOutcome::NonColocated => append_snapshot_notice(
            result,
            "jj sidecar snapshot skipped: repository is not colocated (.git/ and .jj/ are both required); git fallback is intentionally disabled",
        ),
        PostRunJjSnapshotOutcome::Snapshot { revision } => {
            info!(revision = %revision, "Recorded jj sidecar snapshot");
        }
        PostRunJjSnapshotOutcome::Failed { message } => {
            append_snapshot_notice(
                result,
                &format!(
                    "jj sidecar snapshot failed: {message}; git fallback is intentionally disabled"
                ),
            );
        }
    }
    outcome
}

fn evaluate_post_run_snapshot(
    project_root: &Path,
    session_dir: &Path,
    session_id: &str,
    tool_name: &str,
    changed_paths: &[String],
) -> PostRunJjSnapshotOutcome {
    evaluate_post_run_snapshot_with(
        auto_snapshot_enabled(),
        project_root,
        session_id,
        tool_name,
        changed_paths,
        |message| {
            JjJournal::with_session_dir(project_root, session_dir)
                .and_then(|journal| journal.snapshot(message))
        },
    )
}

fn evaluate_post_run_snapshot_with<F>(
    auto_snapshot_enabled: bool,
    project_root: &Path,
    session_id: &str,
    tool_name: &str,
    changed_paths: &[String],
    snapshot: F,
) -> PostRunJjSnapshotOutcome
where
    F: FnOnce(&str) -> Result<RevisionId, JournalError>,
{
    if !auto_snapshot_enabled {
        return PostRunJjSnapshotOutcome::ConfigOff;
    }
    if changed_paths.is_empty() {
        return PostRunJjSnapshotOutcome::NoChangedPaths;
    }
    if !project_root.join(".jj").is_dir() {
        return PostRunJjSnapshotOutcome::NoJjDir;
    }
    if !project_root.join(".git").exists() {
        return PostRunJjSnapshotOutcome::NonColocated;
    }

    let message = format_snapshot_message(session_id, tool_name, changed_paths);
    match snapshot(&message) {
        Ok(revision) => PostRunJjSnapshotOutcome::Snapshot { revision },
        Err(err) => PostRunJjSnapshotOutcome::Failed {
            message: render_journal_error(&err),
        },
    }
}

fn auto_snapshot_enabled() -> bool {
    auto_snapshot_enabled_from_value(std::env::var_os(AUTO_SNAPSHOT_ENV).as_deref())
}

fn auto_snapshot_enabled_from_value(value: Option<&std::ffi::OsStr>) -> bool {
    value
        .and_then(std::ffi::OsStr::to_str)
        .is_some_and(|value| value.trim() == "1")
}

fn format_snapshot_message(session_id: &str, tool_name: &str, changed_paths: &[String]) -> String {
    let mut message = format!(
        "CSA PostRun snapshot session={session_id} tool={tool_name} changed_paths={}",
        changed_paths.len()
    );
    for path in changed_paths.iter().take(8) {
        message.push_str(" path=");
        message.push_str(path);
    }
    message
}

fn render_journal_error(error: &JournalError) -> String {
    error.to_string()
}

fn append_snapshot_notice(result: &mut csa_process::ExecutionResult, message: &str) {
    warn!("{message}");
    if !result.stderr_output.is_empty() && !result.stderr_output.ends_with('\n') {
        result.stderr_output.push('\n');
    }
    result.stderr_output.push_str("CSA_VCS_AUTO_SNAPSHOT: ");
    result.stderr_output.push_str(message);
    result.stderr_output.push('\n');
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn result() -> csa_process::ExecutionResult {
        csa_process::ExecutionResult {
            output: String::new(),
            stderr_output: String::new(),
            summary: "ok".to_string(),
            exit_code: 0,
            peak_memory_mb: None,
        }
    }

    #[test]
    fn config_off_is_a_quiet_noop() {
        let tmp = tempfile::tempdir().expect("tempdir");

        let outcome = evaluate_post_run_snapshot_with(
            false,
            tmp.path(),
            "01SESSION",
            "codex",
            &["src/lib.rs".to_string()],
            |_| panic!("snapshot must not run when config is off"),
        );

        assert_eq!(outcome, PostRunJjSnapshotOutcome::ConfigOff);
        let mut result = result();
        match &outcome {
            PostRunJjSnapshotOutcome::ConfigOff => {}
            other => append_snapshot_notice(&mut result, &format!("{other:?}")),
        }
        assert!(result.stderr_output.is_empty());
    }

    #[test]
    fn changed_paths_empty_skips_snapshot() {
        let tmp = tempfile::tempdir().expect("tempdir");

        let outcome =
            evaluate_post_run_snapshot_with(true, tmp.path(), "01SESSION", "codex", &[], |_| {
                panic!("snapshot must not run for empty changed paths")
            });

        assert_eq!(outcome, PostRunJjSnapshotOutcome::NoChangedPaths);
        let mut result = result();
        match &outcome {
            PostRunJjSnapshotOutcome::NoChangedPaths => {}
            other => append_snapshot_notice(&mut result, &format!("{other:?}")),
        }
        assert!(result.stderr_output.is_empty());
    }

    #[test]
    fn no_jj_dir_degrades_without_git_fallback() {
        let tmp = tempfile::tempdir().expect("tempdir");
        fs::create_dir(tmp.path().join(".git")).expect("create .git");

        let outcome = evaluate_post_run_snapshot_with(
            true,
            tmp.path(),
            "01SESSION",
            "codex",
            &["src/lib.rs".to_string()],
            |_| panic!("snapshot must not run without .jj"),
        );

        assert_eq!(outcome, PostRunJjSnapshotOutcome::NoJjDir);
        let mut result = result();
        append_snapshot_notice(
            &mut result,
            "jj sidecar snapshot skipped: .jj/ not found; git fallback is intentionally disabled",
        );
        assert!(result.stderr_output.contains(".jj/ not found"));
        assert!(
            result
                .stderr_output
                .contains("git fallback is intentionally disabled")
        );
        assert!(!tmp.path().join(".git").join("index").exists());
    }

    #[test]
    fn non_colocated_degrades_without_git_fallback() {
        let tmp = tempfile::tempdir().expect("tempdir");
        fs::create_dir(tmp.path().join(".jj")).expect("create .jj");

        let outcome = evaluate_post_run_snapshot_with(
            true,
            tmp.path(),
            "01SESSION",
            "codex",
            &["src/lib.rs".to_string()],
            |_| panic!("snapshot must not run in a non-colocated repo"),
        );

        assert_eq!(outcome, PostRunJjSnapshotOutcome::NonColocated);
        let mut result = result();
        append_snapshot_notice(
            &mut result,
            "jj sidecar snapshot skipped: repository is not colocated (.git/ and .jj/ are both required); git fallback is intentionally disabled",
        );
        assert!(result.stderr_output.contains("not colocated"));
        assert!(
            result
                .stderr_output
                .contains("git fallback is intentionally disabled")
        );
    }

    #[test]
    fn jj_missing_degrades_without_git_fallback() {
        let tmp = tempfile::tempdir().expect("tempdir");
        fs::create_dir(tmp.path().join(".git")).expect("create .git");
        fs::create_dir(tmp.path().join(".jj")).expect("create .jj");

        let outcome = evaluate_post_run_snapshot_with(
            true,
            tmp.path(),
            "01SESSION",
            "codex",
            &["src/lib.rs".to_string()],
            |_| {
                Err(JournalError::Unavailable(
                    "jj binary not found; git fallback is intentionally disabled".to_string(),
                ))
            },
        );

        assert!(
            matches!(outcome, PostRunJjSnapshotOutcome::Failed { ref message } if message.contains("jj binary not found"))
        );
        let mut result = result();
        if let PostRunJjSnapshotOutcome::Failed { message } = &outcome {
            append_snapshot_notice(
                &mut result,
                &format!(
                    "jj sidecar snapshot failed: {message}; git fallback is intentionally disabled"
                ),
            );
        }
        assert!(result.stderr_output.contains("jj binary not found"));
        assert!(
            result
                .stderr_output
                .contains("git fallback is intentionally disabled")
        );
        assert!(!tmp.path().join(".git").join("index").exists());
    }

    #[test]
    fn snapshot_success_returns_revision_without_notice() {
        let tmp = tempfile::tempdir().expect("tempdir");
        fs::create_dir(tmp.path().join(".git")).expect("create .git");
        fs::create_dir(tmp.path().join(".jj")).expect("create .jj");

        let outcome = evaluate_post_run_snapshot_with(
            true,
            tmp.path(),
            "01SESSION",
            "codex",
            &["src/lib.rs".to_string()],
            |_| Ok(RevisionId::from("rev-from-journal")),
        );

        assert_eq!(
            outcome,
            PostRunJjSnapshotOutcome::Snapshot {
                revision: RevisionId::from("rev-from-journal")
            }
        );
        let result = result();
        assert!(result.stderr_output.is_empty());
    }

    #[test]
    fn auto_snapshot_env_accepts_only_one() {
        assert!(auto_snapshot_enabled_from_value(Some(
            std::ffi::OsStr::new("1")
        )));
        assert!(!auto_snapshot_enabled_from_value(None));
        assert!(!auto_snapshot_enabled_from_value(Some(
            std::ffi::OsStr::new("true",)
        )));
        assert!(!auto_snapshot_enabled_from_value(Some(
            std::ffi::OsStr::new("0",)
        )));
    }

    #[test]
    fn snapshot_message_keeps_untrusted_fields_as_message_text() {
        let message = format_snapshot_message(
            "01SESSION;$(touch hacked)",
            "codex`echo no`\nsecond",
            &["src/lib.rs".to_string()],
        );

        assert!(message.contains("01SESSION;$(touch hacked)"));
        assert!(message.contains("codex`echo no`\nsecond"));
    }
}
