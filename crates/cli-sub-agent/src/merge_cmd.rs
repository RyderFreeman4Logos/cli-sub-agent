use std::path::Path;
use std::process::{Command, Output};

use anyhow::{Context, Result};

use crate::cli::MergeArgs;

const DEFAULT_BRANCH_FALLBACK: &str = "main";
const REGENERABLE_WEAVE_LOCK_PATH: &str = "weave.lock";

pub(crate) fn handle_merge(args: MergeArgs) -> Result<()> {
    let pr_number = args.pr_number;
    let project_root = crate::pipeline::determine_project_root(args.cd.as_deref())?;
    let base_branch = args.base.unwrap_or_else(|| {
        detect_pr_base_branch(&project_root, pr_number).unwrap_or_else(warn_base_fallback)
    });

    let pr_number_arg = pr_number.to_string();
    run_checked(
        &project_root,
        "gh",
        &["pr", "merge", "--merge", &pr_number_arg],
        "merge pull request",
    )?;

    sync_base_branch_best_effort(&project_root, &base_branch);

    println!("Merged PR #{pr_number}; synced base branch `{base_branch}` if possible.");
    Ok(())
}

fn detect_pr_base_branch(project_root: &Path, pr_number: u64) -> Option<String> {
    let pr_number_arg = pr_number.to_string();
    let output = Command::new("gh")
        .args([
            "pr",
            "view",
            &pr_number_arg,
            "--json",
            "baseRefName",
            "-q",
            ".baseRefName",
        ])
        .current_dir(project_root)
        .output();

    match output {
        Ok(output) if output.status.success() => {
            let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if branch.is_empty() {
                None
            } else {
                Some(branch)
            }
        }
        Ok(output) => {
            eprintln!(
                "WARNING: failed to detect PR base branch with `gh pr view`; falling back to `{DEFAULT_BRANCH_FALLBACK}`\n{}",
                command_output_text(&output)
            );
            None
        }
        Err(err) => {
            eprintln!(
                "WARNING: failed to run `gh pr view` for base branch detection: {err}; falling back to `{DEFAULT_BRANCH_FALLBACK}`"
            );
            None
        }
    }
}

fn warn_base_fallback() -> String {
    eprintln!("WARNING: PR base branch was empty; falling back to `{DEFAULT_BRANCH_FALLBACK}`");
    DEFAULT_BRANCH_FALLBACK.to_string()
}

fn sync_base_branch_best_effort(project_root: &Path, base_branch: &str) {
    if let Err(err) = checkout_base_branch_after_weave_lock_restore(project_root, base_branch) {
        eprintln!("WARNING: merge succeeded, but post-merge checkout failed:\n{err:#}");
        return;
    }

    if let Err(err) = run_checked(
        project_root,
        "git",
        &["pull", "origin", base_branch],
        "pull base branch",
    ) {
        eprintln!("WARNING: merge succeeded, but post-merge pull failed:\n{err:#}");
    }
}

fn checkout_base_branch_after_weave_lock_restore(
    project_root: &Path,
    base_branch: &str,
) -> Result<()> {
    restore_weave_lock_drift_before_checkout(project_root);
    run_checked(
        project_root,
        "git",
        &["checkout", base_branch],
        "checkout base branch",
    )
}

fn restore_weave_lock_drift_before_checkout(project_root: &Path) {
    let Ok(output) = Command::new("git")
        .args(["status", "--porcelain", "--untracked-files=no"])
        .current_dir(project_root)
        .output()
    else {
        return;
    };

    if !output.status.success() {
        return;
    }

    let status = String::from_utf8_lossy(&output.stdout);
    if !has_unstaged_weave_lock_change(&status) {
        return;
    }

    if let Err(err) = run_checked(
        project_root,
        "git",
        &["checkout", "--", REGENERABLE_WEAVE_LOCK_PATH],
        "restore weave.lock",
    ) {
        tracing::debug!(error = %err, "failed to restore transient weave.lock drift before checkout");
    }
}

fn has_unstaged_weave_lock_change(status: &str) -> bool {
    status.lines().any(|line| {
        let second_status = line.as_bytes().get(1).copied();
        let path = line.get(3..);
        second_status.is_some_and(|status| status != b' ')
            && path == Some(REGENERABLE_WEAVE_LOCK_PATH)
    })
}

fn run_checked(project_root: &Path, program: &str, args: &[&str], action: &str) -> Result<()> {
    let output = Command::new(program)
        .args(args)
        .current_dir(project_root)
        .output()
        .with_context(|| format!("failed to run `{}`", shell_command(program, args)))?;

    if !output.status.success() {
        anyhow::bail!(
            "failed to {action} with `{}`\n{}",
            shell_command(program, args),
            command_output_text(&output)
        );
    }

    Ok(())
}

fn shell_command(program: &str, args: &[&str]) -> String {
    std::iter::once(program)
        .chain(args.iter().copied())
        .collect::<Vec<_>>()
        .join(" ")
}

fn command_output_text(output: &Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    match (stdout.trim().is_empty(), stderr.trim().is_empty()) {
        (true, true) => "(no output)".to_string(),
        (false, true) => format!("stdout:\n{}", stdout.trim_end()),
        (true, false) => format!("stderr:\n{}", stderr.trim_end()),
        (false, false) => format!(
            "stdout:\n{}\nstderr:\n{}",
            stdout.trim_end(),
            stderr.trim_end()
        ),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use clap::Parser;
    use tempfile::TempDir;

    use super::{
        checkout_base_branch_after_weave_lock_restore, has_unstaged_weave_lock_change,
        shell_command,
    };
    use crate::cli::{Cli, Commands};

    #[test]
    fn parses_merge_args_with_default_base_detection() {
        let cli = Cli::parse_from(["csa", "merge", "1626"]);

        match cli.command {
            Commands::Merge(args) => {
                assert_eq!(args.pr_number, 1626);
                assert_eq!(args.base, None);
                assert_eq!(args.cd, None);
            }
            _ => panic!("expected merge command"),
        }
    }

    #[test]
    fn parses_merge_args_with_explicit_base() {
        let cli = Cli::parse_from(["csa", "merge", "1626", "--base", "dev"]);

        match cli.command {
            Commands::Merge(args) => {
                assert_eq!(args.pr_number, 1626);
                assert_eq!(args.base.as_deref(), Some("dev"));
                assert_eq!(args.cd, None);
            }
            _ => panic!("expected merge command"),
        }
    }

    #[test]
    fn parses_merge_args_with_cd() {
        let cli = Cli::parse_from(["csa", "merge", "1626", "--cd", "/tmp/repo"]);

        match cli.command {
            Commands::Merge(args) => {
                assert_eq!(args.pr_number, 1626);
                assert_eq!(args.cd.as_deref(), Some("/tmp/repo"));
            }
            _ => panic!("expected merge command"),
        }
    }

    #[test]
    fn renders_exact_gh_merge_command() {
        assert_eq!(
            shell_command("gh", &["pr", "merge", "--merge", "1626"]),
            "gh pr merge --merge 1626"
        );
    }

    #[test]
    fn detects_unstaged_weave_lock_change_only_for_repo_root_lockfile() {
        assert!(has_unstaged_weave_lock_change(" M weave.lock\n"));
        assert!(has_unstaged_weave_lock_change("MM weave.lock\n"));
        assert!(!has_unstaged_weave_lock_change("M  weave.lock\n"));
        assert!(!has_unstaged_weave_lock_change(" M nested/weave.lock\n"));
        assert!(!has_unstaged_weave_lock_change(" M other.txt\n"));
    }

    #[test]
    fn checkout_restores_dirty_weave_lock_before_switching_to_base() {
        let repo = setup_checkout_blocked_repo();

        git(&repo, &["checkout", "-b", "feature"]);
        write(repo.path(), "weave.lock", "feature lock\n");
        git(&repo, &["add", "weave.lock"]);
        git(&repo, &["commit", "-m", "feature lock"]);
        write(repo.path(), "weave.lock", "dirty lock\n");

        checkout_base_branch_after_weave_lock_restore(repo.path(), "main")
            .expect("checkout should restore weave.lock drift and switch to main");

        assert_eq!(current_branch(&repo), "main");
        assert_eq!(read(repo.path(), "weave.lock"), "main lock\n");
        assert_eq!(git_stdout(&repo, &["status", "--porcelain"]), "");
    }

    #[test]
    fn checkout_preserves_other_dirty_file_and_reports_checkout_failure() {
        let repo = setup_checkout_blocked_repo();

        git(&repo, &["checkout", "-b", "feature"]);
        write(repo.path(), "weave.lock", "feature lock\n");
        write(repo.path(), "tracked.txt", "feature contents\n");
        git(&repo, &["add", "weave.lock", "tracked.txt"]);
        git(&repo, &["commit", "-m", "feature changes"]);
        write(repo.path(), "weave.lock", "dirty lock\n");
        write(repo.path(), "tracked.txt", "dirty contents\n");

        let err = checkout_base_branch_after_weave_lock_restore(repo.path(), "main")
            .expect_err("other dirty tracked files should still block checkout");
        let err = format!("{err:#}");

        assert!(err.contains("failed to checkout base branch"));
        assert!(err.contains("tracked.txt"));
        assert_eq!(current_branch(&repo), "feature");
        assert_eq!(read(repo.path(), "weave.lock"), "feature lock\n");
        assert_eq!(read(repo.path(), "tracked.txt"), "dirty contents\n");
    }

    fn setup_checkout_blocked_repo() -> TempDir {
        let repo = TempDir::new().expect("tempdir");
        git(&repo, &["init", "-b", "main"]);
        git(&repo, &["config", "core.excludesFile", ""]);
        git(&repo, &["config", "user.email", "test@example.com"]);
        git(&repo, &["config", "user.name", "Test User"]);
        write(repo.path(), "weave.lock", "main lock\n");
        write(repo.path(), "tracked.txt", "main contents\n");
        git(&repo, &["add", "weave.lock", "tracked.txt"]);
        git(&repo, &["commit", "-m", "initial"]);
        repo
    }

    fn git(repo: &TempDir, args: &[&str]) {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(repo.path())
            .output()
            .expect("run git");
        assert!(
            output.status.success(),
            "git {} failed\nstdout:\n{}\nstderr:\n{}",
            args.join(" "),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn git_stdout(repo: &TempDir, args: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(repo.path())
            .output()
            .expect("run git");
        assert!(
            output.status.success(),
            "git {} failed\nstdout:\n{}\nstderr:\n{}",
            args.join(" "),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).to_string()
    }

    fn current_branch(repo: &TempDir) -> String {
        git_stdout(repo, &["branch", "--show-current"])
            .trim()
            .to_string()
    }

    fn read(repo: &Path, path: &str) -> String {
        std::fs::read_to_string(repo.join(path)).expect("read file")
    }

    fn write(repo: &Path, path: &str, contents: &str) {
        std::fs::write(repo.join(path), contents).expect("write file");
    }
}
