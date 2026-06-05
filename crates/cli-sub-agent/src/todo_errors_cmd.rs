use anyhow::{Context, Result};
use chrono::{DateTime, Local, Utc};
use csa_todo::{TodoManager, TodoPlan};
use std::collections::HashSet;
use std::fmt::Write as _;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
struct PlanErrorRow {
    session_id: String,
    timestamp: String,
    tool: String,
    exit_code: i32,
    stderr_summary: String,
}

pub(crate) fn handle_errors(branch: Option<String>, cd: Option<String>) -> Result<()> {
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
    let branch = match branch {
        Some(branch) => branch,
        None => detect_current_branch(&project_root)
            .ok_or_else(|| anyhow::anyhow!("No current branch detected; pass --branch <branch>"))?,
    };
    let manager = TodoManager::new(&project_root)?;

    let Some(plan) = find_latest_plan_for_branch(&manager, &branch)? else {
        eprintln!("No TODO plan found for branch '{branch}'.");
        return Ok(());
    };

    let sessions = select_sessions_for_plan(&project_root, &plan, &branch)?;
    if sessions.is_empty() {
        eprintln!(
            "No CSA sessions found for TODO plan '{}' on branch '{}'.",
            plan.timestamp, branch
        );
        return Ok(());
    }

    let rows = collect_plan_error_rows(&sessions)?;
    if rows.is_empty() {
        eprintln!(
            "No failed CSA sessions found for TODO plan '{}' on branch '{}'.",
            plan.timestamp, branch
        );
        return Ok(());
    }

    print!("{}", render_plan_error_table(&rows));
    Ok(())
}

/// Auto-detect current git branch. Returns None on detached HEAD or error.
fn detect_current_branch(project_root: &Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["branch", "--show-current"])
        .current_dir(project_root)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if branch.is_empty() {
        None
    } else {
        Some(branch)
    }
}

fn find_latest_plan_for_branch(manager: &TodoManager, branch: &str) -> Result<Option<TodoPlan>> {
    Ok(manager.find_by_branch(branch)?.into_iter().next())
}

fn select_sessions_for_plan(
    project_root: &Path,
    plan: &TodoPlan,
    branch: &str,
) -> Result<Vec<csa_session::MetaSessionState>> {
    let linked_session_ids: HashSet<&str> =
        plan.metadata.sessions.iter().map(String::as_str).collect();
    let mut sessions = csa_session::list_sessions(project_root, None)?;

    sessions.retain(|session| {
        session.branch.as_deref() == Some(branch)
            || linked_session_ids.contains(session.meta_session_id.as_str())
    });
    sessions.sort_by_key(|session| std::cmp::Reverse(session.last_accessed));
    Ok(sessions)
}

fn collect_plan_error_rows(
    sessions: &[csa_session::MetaSessionState],
) -> Result<Vec<PlanErrorRow>> {
    let mut rows = Vec::new();

    for session in sessions {
        let session_project = Path::new(&session.project_path);
        let Some(result) = csa_session::load_result(session_project, &session.meta_session_id)
            .with_context(|| {
                format!(
                    "Failed to load result.toml for session {}",
                    session.meta_session_id
                )
            })?
        else {
            continue;
        };

        if !is_error_result(&result) {
            continue;
        }

        rows.push(PlanErrorRow {
            session_id: short_session_id(&session.meta_session_id).to_string(),
            timestamp: format_error_timestamp(result.completed_at),
            tool: truncate(&result.tool, 12),
            exit_code: result.exit_code,
            stderr_summary: format_stderr_summary(&result.summary),
        });
    }

    Ok(rows)
}

fn is_error_result(result: &csa_session::SessionResult) -> bool {
    let status = result.status.trim().to_ascii_lowercase();
    result.exit_code != 0 || matches!(status.as_str(), "failed" | "failure")
}

fn short_session_id(session_id: &str) -> &str {
    &session_id[..11.min(session_id.len())]
}

fn format_error_timestamp(timestamp: DateTime<Utc>) -> String {
    timestamp
        .with_timezone(&Local)
        .format("%Y-%m-%d %H:%M")
        .to_string()
}

fn format_stderr_summary(summary: &str) -> String {
    let one_line = summary.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.is_empty() {
        "-".to_string()
    } else {
        truncate(&one_line, 80)
    }
}

fn render_plan_error_table(rows: &[PlanErrorRow]) -> String {
    let mut table = String::new();
    writeln!(
        table,
        "{:<11}  {:<16}  {:<12}  {:<9}  STDERR_SUMMARY",
        "SESSION", "TIMESTAMP", "TOOL", "EXIT_CODE"
    )
    .expect("write table header");
    writeln!(table, "{}", "-".repeat(118)).expect("write table separator");

    for row in rows {
        writeln!(
            table,
            "{:<11}  {:<16}  {:<12}  {:<9}  {}",
            row.session_id, row.timestamp, row.tool, row.exit_code, row.stderr_summary
        )
        .expect("write table row");
    }

    table
}

/// Truncate a string to `max_len` characters, appending "..." if truncated.
fn truncate(s: &str, max_len: usize) -> String {
    let char_count = s.chars().count();
    if char_count <= max_len {
        return s.to_string();
    }
    if max_len <= 3 {
        return ".".repeat(max_len);
    }

    let truncated: String = s.chars().take(max_len - 3).collect();
    format!("{truncated}...")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, TimeZone};
    use csa_session::SessionResult;
    use std::ffi::{OsStr, OsString};
    use std::path::PathBuf;

    struct EnvVarGuard {
        key: &'static str,
        original: Option<OsString>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: impl AsRef<OsStr>) -> Self {
            let original = std::env::var_os(key);
            // SAFETY: callers are serialized tests that restore the variable on drop.
            unsafe { std::env::set_var(key, value) };
            Self { key, original }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            // SAFETY: callers are serialized tests that restore the variable on drop.
            unsafe {
                match self.original.as_deref() {
                    Some(value) => std::env::set_var(self.key, value),
                    None => std::env::remove_var(self.key),
                }
            }
        }
    }

    fn session_result(status: &str, exit_code: i32, summary: &str, tool: &str) -> SessionResult {
        let started_at = Utc
            .with_ymd_and_hms(2026, 5, 1, 0, 0, 0)
            .single()
            .expect("valid test timestamp");
        SessionResult {
            post_exec_gate: None,
            status: status.to_string(),
            exit_code,
            summary: summary.to_string(),
            tool: tool.to_string(),
            original_tool: None,
            fallback_tool: None,
            fallback_reason: None,
            started_at,
            completed_at: started_at + Duration::seconds(2),
            events_count: 0,
            artifacts: Vec::new(),
            peak_memory_mb: None,
            fallback_chain: None,
            gate_timeout: false,
            warnings: Vec::new(),
            raw_process_exit_code: None,
            uncommitted_changes: None,
            manager_fields: Default::default(),
        }
    }

    fn create_result_session(
        project: &Path,
        branch: &str,
        status: &str,
        exit_code: i32,
        summary: &str,
        tool: &str,
    ) -> csa_session::MetaSessionState {
        let mut session =
            csa_session::create_session_fresh(project, Some(summary), None, Some(tool))
                .expect("create session");
        session.branch = Some(branch.to_string());
        csa_session::save_session(&session).expect("save session branch");
        csa_session::save_result(
            project,
            &session.meta_session_id,
            &session_result(status, exit_code, summary, tool),
        )
        .expect("save result");
        session
    }

    fn setup_state(tmp: &Path) -> (EnvVarGuard, EnvVarGuard, PathBuf) {
        let home_guard = EnvVarGuard::set("HOME", tmp);
        let state_guard = EnvVarGuard::set("XDG_STATE_HOME", tmp.join(".local/state"));
        let project = tmp.join("project");
        std::fs::create_dir_all(&project).expect("create project");
        (home_guard, state_guard, project)
    }

    #[test]
    #[serial_test::serial]
    fn plan_error_rows_include_only_failed_sessions_for_plan() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (_home_guard, _state_guard, project) = setup_state(tmp.path());
        let manager = TodoManager::new(&project).expect("todo manager");
        let plan = manager
            .create("Error aggregation", Some("feat/errors"))
            .expect("create plan");

        let failed = create_result_session(
            &project,
            "feat/errors",
            "failure",
            1,
            "cargo test failed",
            "codex",
        );
        create_result_session(
            &project,
            "feat/errors",
            "success",
            0,
            "tests passed",
            "codex",
        );
        create_result_session(
            &project,
            "feat/other",
            "failure",
            2,
            "wrong branch failed",
            "gemini-cli",
        );

        let sessions =
            select_sessions_for_plan(&project, &plan, "feat/errors").expect("select sessions");
        let rows = collect_plan_error_rows(&sessions).expect("collect errors");

        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].session_id,
            short_session_id(&failed.meta_session_id)
        );
        assert_eq!(rows[0].tool, "codex");
        assert_eq!(rows[0].exit_code, 1);
        assert_eq!(rows[0].stderr_summary, "cargo test failed");
    }

    #[test]
    #[serial_test::serial]
    fn plan_error_rows_include_sessions_explicitly_linked_to_plan() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (_home_guard, _state_guard, project) = setup_state(tmp.path());
        let manager = TodoManager::new(&project).expect("todo manager");
        let plan = manager
            .create("Linked session", Some("feat/errors"))
            .expect("create plan");
        let linked = create_result_session(
            &project,
            "feat/other",
            "failed",
            0,
            "linked session failed",
            "opencode",
        );
        let plan = manager
            .link_session(&plan.timestamp, &linked.meta_session_id)
            .expect("link session");

        let sessions =
            select_sessions_for_plan(&project, &plan, "feat/errors").expect("select sessions");
        let rows = collect_plan_error_rows(&sessions).expect("collect errors");

        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].session_id,
            short_session_id(&linked.meta_session_id)
        );
        assert_eq!(rows[0].exit_code, 0);
        assert_eq!(rows[0].stderr_summary, "linked session failed");
    }

    #[test]
    fn render_plan_error_table_formats_expected_columns() {
        let table = render_plan_error_table(&[PlanErrorRow {
            session_id: "01ABCDEF012".to_string(),
            timestamp: "2026-05-01 00:00".to_string(),
            tool: "codex".to_string(),
            exit_code: 1,
            stderr_summary: "line one line two".to_string(),
        }]);

        assert!(table.contains("SESSION"));
        assert!(table.contains("TIMESTAMP"));
        assert!(table.contains("EXIT_CODE"));
        assert!(table.contains("STDERR_SUMMARY"));
        assert!(table.contains("01ABCDEF012"));
        assert!(table.contains("line one line two"));
    }
}
