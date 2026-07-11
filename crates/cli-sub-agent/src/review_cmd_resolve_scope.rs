use super::*;
use std::io::{Read, Seek, SeekFrom};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

const GIT_SCOPE_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) fn derive_scope(args: &ReviewArgs) -> String {
    if let Some(ref range) = args.range {
        return format!("range:{range}");
    }
    if let Some(ref files) = args.files {
        return format!("files:{files}");
    }
    if let Some(ref commit) = args.commit {
        return format!("commit:{commit}");
    }
    if args.diff {
        return "uncommitted".to_string();
    }
    format!("base:{}", args.branch.as_deref().unwrap_or("main"))
}

/// Derive the review scope string from CLI arguments and repository state.
///
/// `--diff` primarily means uncommitted changes. If that diff is empty on a
/// feature branch with commits ahead of the default branch, review the branch
/// diff instead so clean committed feature branches are not skipped.
pub(crate) fn derive_scope_for_project(args: &ReviewArgs, project_root: &Path) -> String {
    let scope = derive_scope(args);
    if scope != "uncommitted" {
        return scope;
    }

    derive_diff_scope_for_project(project_root)
}

fn derive_diff_scope_for_project(project_root: &Path) -> String {
    if git_diff_has_output(project_root, &["diff", "HEAD"]).unwrap_or(true) {
        return "uncommitted".to_string();
    }

    if git_has_untracked_files(project_root).unwrap_or(false) {
        return "uncommitted".to_string();
    }

    let Some((current_branch, default_branch)) = detect_current_and_default_branch(project_root)
    else {
        return "uncommitted".to_string();
    };

    if is_protected_review_branch(&current_branch, &default_branch) {
        return "uncommitted".to_string();
    }

    let ahead_range = format!("{default_branch}..HEAD");
    if !git_rev_list_has_commits(project_root, &ahead_range).unwrap_or(false) {
        return "uncommitted".to_string();
    }

    info!("No uncommitted changes; falling back to branch diff (base:{default_branch})");
    format!("base:{default_branch}")
}

fn detect_current_and_default_branch(project_root: &Path) -> Option<(String, String)> {
    let vcs_kind = csa_session::vcs_backends::detect_vcs_kind_with_config(project_root, None, None)
        .ok()
        .flatten()?;
    let backend = csa_session::vcs_backends::create_vcs_backend_with_config(
        project_root,
        Some(vcs_kind),
        None,
    );
    let current_branch = backend.current_branch(project_root).ok().flatten()?;
    let default_branch = backend.default_branch(project_root).ok().flatten()?;
    Some((current_branch, default_branch))
}

fn is_protected_review_branch(current_branch: &str, default_branch: &str) -> bool {
    matches!(current_branch, "main" | "master" | "dev" | "develop")
        || current_branch == default_branch
}

fn git_diff_has_output(project_root: &Path, args: &[&str]) -> Option<bool> {
    let output = git_output_with_timeout(project_root, args)?;

    output.status.success().then_some(!output.stdout.is_empty())
}

fn git_has_untracked_files(project_root: &Path) -> Option<bool> {
    let output = git_output_with_timeout(
        project_root,
        &["ls-files", "--others", "--exclude-standard"],
    )?;

    output.status.success().then_some(!output.stdout.is_empty())
}

fn git_rev_list_has_commits(project_root: &Path, range: &str) -> Option<bool> {
    let output = git_output_with_timeout(project_root, &["rev-list", "--count", range])?;
    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8(output.stdout).ok()?;
    let count = stdout.trim().parse::<u64>().ok()?;
    Some(count > 0)
}

fn git_output_with_timeout(project_root: &Path, args: &[&str]) -> Option<Output> {
    let mut command = Command::new("git");
    command.args(args).current_dir(project_root);
    run_command_with_timeout(&mut command, GIT_SCOPE_PROBE_TIMEOUT)
}

fn run_command_with_timeout(command: &mut Command, timeout: Duration) -> Option<Output> {
    let mut stdout = tempfile::tempfile().ok()?;
    let mut stderr = tempfile::tempfile().ok()?;
    command
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout.try_clone().ok()?))
        .stderr(Stdio::from(stderr.try_clone().ok()?));
    let mut child = command.spawn().ok()?;
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(None) | Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };

    let mut stdout_bytes = Vec::new();
    stdout.seek(SeekFrom::Start(0)).ok()?;
    stdout.read_to_end(&mut stdout_bytes).ok()?;
    let mut stderr_bytes = Vec::new();
    stderr.seek(SeekFrom::Start(0)).ok()?;
    stderr.read_to_end(&mut stderr_bytes).ok()?;
    Some(Output {
        status,
        stdout: stdout_bytes,
        stderr: stderr_bytes,
    })
}

pub(crate) fn review_scope_allows_auto_discovery(args: &ReviewArgs) -> bool {
    args.range.is_some() || (!args.diff && args.commit.is_none() && args.files.is_none())
}

#[cfg(all(test, unix))]
mod timeout_tests {
    use super::*;

    #[test]
    fn command_capture_returns_none_after_deadline() {
        let mut command = Command::new("sleep");
        command.arg("5");
        let started = Instant::now();

        assert!(run_command_with_timeout(&mut command, Duration::from_millis(20)).is_none());
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
