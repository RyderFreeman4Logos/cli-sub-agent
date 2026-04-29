//! PostRun integration for the optional jj sidecar journal.

use std::path::Path;

use csa_core::vcs::{JournalError, RevisionId, SnapshotJournal};
use csa_session::JjJournal;
use tracing::{info, warn};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PostRunJjSnapshotOutcome {
    ConfigOff,
    NoChangedPaths,
    NoJjDir,
    NonColocated,
    Snapshot { revision: RevisionId },
    Failed { message: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SessionJjAggregateOutcome {
    ConfigOff,
    AggregateOff,
    ChildSession,
    NoJjDir,
    NonColocated,
    NoSnapshots,
    Aggregated { snapshot_count: usize },
    Failed { message: String },
}

pub(crate) async fn aggregate_session(
    project_root: &Path,
    session_dir: &Path,
    session_id: &str,
    snapshot_count: usize,
    message_template: &str,
) -> anyhow::Result<()> {
    let project_root = project_root.to_path_buf();
    let session_dir = session_dir.to_path_buf();
    let session_id = session_id.to_string();
    let message_template = message_template.to_string();
    tokio::task::spawn_blocking(move || {
        let journal = JjJournal::with_session_dir(project_root, session_dir)?;
        journal.aggregate_session(&session_id, snapshot_count, &message_template)
    })
    .await?
    .map_err(anyhow::Error::from)
}

pub(crate) fn maybe_record_post_run_snapshot(
    config: Option<&csa_config::VcsConfig>,
    project_root: &Path,
    session_dir: &Path,
    session_id: &str,
    tool_name: &str,
    changed_paths: &[String],
    result: &mut csa_process::ExecutionResult,
) -> PostRunJjSnapshotOutcome {
    let outcome = evaluate_post_run_snapshot(
        config,
        project_root,
        session_dir,
        session_id,
        tool_name,
        changed_paths,
    );
    match &outcome {
        PostRunJjSnapshotOutcome::ConfigOff => {
            info!("jj sidecar snapshot disabled by [vcs].auto_snapshot");
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

pub(crate) async fn maybe_aggregate_session_snapshots(
    config: Option<&csa_config::VcsConfig>,
    project_root: &Path,
    session_dir: &Path,
    session_id: &str,
    session_depth: u32,
    result: &mut csa_process::ExecutionResult,
) -> SessionJjAggregateOutcome {
    let config = config.cloned().unwrap_or_default();
    let outcome =
        evaluate_session_aggregate(config, project_root, session_dir, session_id, session_depth)
            .await;
    match &outcome {
        SessionJjAggregateOutcome::ConfigOff => {
            info!("jj sidecar aggregation disabled because [vcs].auto_snapshot is false");
        }
        SessionJjAggregateOutcome::AggregateOff => {
            info!("jj sidecar aggregation disabled by [vcs].auto_aggregate");
        }
        SessionJjAggregateOutcome::ChildSession => {
            info!("Skipping jj sidecar aggregation for child session");
        }
        SessionJjAggregateOutcome::NoJjDir => append_snapshot_notice(
            result,
            "jj sidecar aggregation skipped: .jj/ not found; git fallback is intentionally disabled",
        ),
        SessionJjAggregateOutcome::NonColocated => append_snapshot_notice(
            result,
            "jj sidecar aggregation skipped: repository is not colocated (.git/ and .jj/ are both required); git fallback is intentionally disabled",
        ),
        SessionJjAggregateOutcome::NoSnapshots => {
            info!("Skipping jj sidecar aggregation because no snapshots were recorded");
        }
        SessionJjAggregateOutcome::Aggregated { snapshot_count } => {
            info!(snapshot_count, "Aggregated jj sidecar snapshots");
        }
        SessionJjAggregateOutcome::Failed { message } => {
            append_snapshot_notice(
                result,
                &format!(
                    "jj sidecar aggregation failed: {message}; git fallback is intentionally disabled"
                ),
            );
        }
    }
    outcome
}

async fn evaluate_session_aggregate(
    config: csa_config::VcsConfig,
    project_root: &Path,
    session_dir: &Path,
    session_id: &str,
    session_depth: u32,
) -> SessionJjAggregateOutcome {
    if !config.auto_snapshot {
        return SessionJjAggregateOutcome::ConfigOff;
    }
    if !config.resolved_auto_aggregate() {
        return SessionJjAggregateOutcome::AggregateOff;
    }
    if session_depth != 0 {
        return SessionJjAggregateOutcome::ChildSession;
    }
    if !project_root.join(".jj").is_dir() {
        return SessionJjAggregateOutcome::NoJjDir;
    }
    if !project_root.join(".git").exists() {
        return SessionJjAggregateOutcome::NonColocated;
    }

    let journal = match JjJournal::with_session_dir(project_root, session_dir) {
        Ok(journal) => journal,
        Err(err) => {
            return SessionJjAggregateOutcome::Failed {
                message: render_journal_error(&err),
            };
        }
    };
    let snapshot_count = match journal.snapshot_revisions() {
        Ok(revisions) => revisions.len(),
        Err(err) => {
            return SessionJjAggregateOutcome::Failed {
                message: render_journal_error(&err),
            };
        }
    };
    if snapshot_count == 0 {
        return SessionJjAggregateOutcome::NoSnapshots;
    }

    let message_template = config.aggregate_message_template.clone();
    let aggregate = aggregate_session(
        project_root,
        session_dir,
        session_id,
        snapshot_count,
        &message_template,
    )
    .await;
    match aggregate {
        Ok(()) => SessionJjAggregateOutcome::Aggregated { snapshot_count },
        Err(err) => SessionJjAggregateOutcome::Failed {
            message: err.to_string(),
        },
    }
}

fn evaluate_post_run_snapshot(
    config: Option<&csa_config::VcsConfig>,
    project_root: &Path,
    session_dir: &Path,
    session_id: &str,
    tool_name: &str,
    changed_paths: &[String],
) -> PostRunJjSnapshotOutcome {
    evaluate_post_run_snapshot_with(
        config.cloned().unwrap_or_default(),
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
    config: csa_config::VcsConfig,
    project_root: &Path,
    session_id: &str,
    tool_name: &str,
    changed_paths: &[String],
    snapshot: F,
) -> PostRunJjSnapshotOutcome
where
    F: FnOnce(&str) -> Result<RevisionId, JournalError>,
{
    if !config.auto_snapshot {
        return PostRunJjSnapshotOutcome::ConfigOff;
    }
    if config.snapshot_trigger == csa_config::SnapshotTrigger::ToolCompleted {
        warn!(
            "[vcs].snapshot_trigger=\"tool-completed\" is reserved for V2; falling back to post-run snapshot"
        );
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
    result.stderr_output.push_str("[vcs].auto_snapshot: ");
    result.stderr_output.push_str(message);
    result.stderr_output.push('\n');
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_env_lock::{ScopedEnvVarRestore, TEST_ENV_LOCK};
    use std::fs;
    use std::process::Command;

    fn vcs_config(auto_snapshot: bool) -> csa_config::VcsConfig {
        csa_config::VcsConfig {
            auto_snapshot,
            ..Default::default()
        }
    }

    fn result() -> csa_process::ExecutionResult {
        csa_process::ExecutionResult {
            output: String::new(),
            stderr_output: String::new(),
            summary: "ok".to_string(),
            exit_code: 0,
            peak_memory_mb: None,
        }
    }

    fn run_command(repo: &Path, program: &str, args: &[&str]) {
        let output = Command::new(program)
            .args(args)
            .current_dir(repo)
            .output()
            .unwrap_or_else(|err| panic!("spawn {program}: {err}"));
        assert!(
            output.status.success(),
            "{program} {args:?} failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn jj_log_descriptions(repo: &Path) -> String {
        let output = Command::new("jj")
            .args(["log", "--no-graph", "-T", "description ++ \"\\n\""])
            .current_dir(repo)
            .output()
            .expect("run jj log");
        assert!(
            output.status.success(),
            "jj log failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).to_string()
    }

    fn setup_colocated_jj_git_repo() -> tempfile::TempDir {
        let repo = tempfile::tempdir().expect("repo tempdir");
        run_command(repo.path(), "git", &["init"]);
        run_command(
            repo.path(),
            "git",
            &["config", "user.email", "csa-test@example.com"],
        );
        run_command(repo.path(), "git", &["config", "user.name", "CSA Test"]);
        run_command(repo.path(), "jj", &["git", "init", "--colocate"]);
        run_command(
            repo.path(),
            "jj",
            &[
                "config",
                "set",
                "--repo",
                "user.email",
                "csa-test@example.com",
            ],
        );
        run_command(
            repo.path(),
            "jj",
            &["config", "set", "--repo", "user.name", "CSA Test"],
        );
        repo
    }

    #[test]
    fn config_off_is_a_quiet_noop() {
        let tmp = tempfile::tempdir().expect("tempdir");

        let outcome = evaluate_post_run_snapshot_with(
            vcs_config(false),
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

        let outcome = evaluate_post_run_snapshot_with(
            vcs_config(true),
            tmp.path(),
            "01SESSION",
            "codex",
            &[],
            |_| panic!("snapshot must not run for empty changed paths"),
        );

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
            vcs_config(true),
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
            vcs_config(true),
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
            vcs_config(true),
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
            vcs_config(true),
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
    fn real_colocated_jj_repo_snapshots_only_when_vcs_config_enables_it() {
        if which::which("jj").is_err() {
            eprintln!("skipping real jj snapshot test because jj is not installed");
            return;
        }
        let _env_lock = TEST_ENV_LOCK.blocking_lock();
        let env_home = tempfile::tempdir().expect("env home tempdir");
        let jj_config_home = env_home.path().join("jj-config-home");
        fs::create_dir_all(&jj_config_home).expect("create jj config home");
        let _home_guard = ScopedEnvVarRestore::set("HOME", &jj_config_home);
        let _config_guard = ScopedEnvVarRestore::set("XDG_CONFIG_HOME", &jj_config_home);
        let repo = setup_colocated_jj_git_repo();
        let session_dir = tempfile::tempdir().expect("session tempdir");
        fs::write(repo.path().join("tracked.txt"), "first\n").expect("write tracked file");
        let changed_paths = vec!["tracked.txt".to_string()];

        let mut disabled_result = result();
        let disabled_outcome = maybe_record_post_run_snapshot(
            Some(&vcs_config(false)),
            repo.path(),
            session_dir.path(),
            "01DISABLED",
            "codex",
            &changed_paths,
            &mut disabled_result,
        );
        assert_eq!(disabled_outcome, PostRunJjSnapshotOutcome::ConfigOff);
        assert!(!jj_log_descriptions(repo.path()).contains("01DISABLED"));

        let mut enabled_result = result();
        let enabled_outcome = maybe_record_post_run_snapshot(
            Some(&vcs_config(true)),
            repo.path(),
            session_dir.path(),
            "01ENABLED",
            "codex",
            &changed_paths,
            &mut enabled_result,
        );
        assert!(matches!(
            enabled_outcome,
            PostRunJjSnapshotOutcome::Snapshot { .. }
        ));
        assert!(enabled_result.stderr_output.is_empty());
        assert!(
            jj_log_descriptions(repo.path()).contains("CSA PostRun snapshot session=01ENABLED"),
            "jj log should include the CSA snapshot description"
        );
    }

    #[test]
    fn tool_completed_trigger_falls_back_to_post_run_snapshot() {
        let tmp = tempfile::tempdir().expect("tempdir");
        fs::create_dir(tmp.path().join(".git")).expect("create .git");
        fs::create_dir(tmp.path().join(".jj")).expect("create .jj");
        let config = csa_config::VcsConfig {
            auto_snapshot: true,
            snapshot_trigger: csa_config::SnapshotTrigger::ToolCompleted,
            ..Default::default()
        };

        let outcome = evaluate_post_run_snapshot_with(
            config,
            tmp.path(),
            "01SESSION",
            "codex",
            &["src/lib.rs".to_string()],
            |_| Ok(RevisionId::from("rev-from-fallback")),
        );

        assert_eq!(
            outcome,
            PostRunJjSnapshotOutcome::Snapshot {
                revision: RevisionId::from("rev-from-fallback")
            }
        );
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

    #[tokio::test]
    async fn aggregate_skips_when_auto_aggregate_is_explicitly_off() {
        let tmp = tempfile::tempdir().expect("tempdir");
        fs::create_dir(tmp.path().join(".git")).expect("create .git");
        fs::create_dir(tmp.path().join(".jj")).expect("create .jj");
        let config = csa_config::VcsConfig {
            auto_snapshot: true,
            auto_aggregate: Some(false),
            ..Default::default()
        };

        let outcome =
            evaluate_session_aggregate(config, tmp.path(), tmp.path(), "01SESSION", 0).await;

        assert_eq!(outcome, SessionJjAggregateOutcome::AggregateOff);
    }

    #[tokio::test]
    async fn aggregate_skips_child_sessions() {
        let tmp = tempfile::tempdir().expect("tempdir");
        fs::create_dir(tmp.path().join(".git")).expect("create .git");
        fs::create_dir(tmp.path().join(".jj")).expect("create .jj");
        let config = csa_config::VcsConfig {
            auto_snapshot: true,
            ..Default::default()
        };

        let outcome =
            evaluate_session_aggregate(config, tmp.path(), tmp.path(), "01SESSION", 1).await;

        assert_eq!(outcome, SessionJjAggregateOutcome::ChildSession);
    }
}
