use std::path::PathBuf;
use std::process::{Command, Output};

use anyhow::{Context, Result};
use csa_hooks::{MarkerStatus, emit_merge_completed_event, verify_pr_bot_marker};

use crate::cli::MergeArgs;

const DEFAULT_BRANCH_FALLBACK: &str = "main";

pub(crate) fn handle_merge(args: MergeArgs) -> Result<()> {
    if !args.force {
        ensure_worktree_clean(
            "pre-merge",
            Some("pass --force to bypass this pre-merge check"),
        )?;
    }

    let pr_number = args.pr_number;
    let head_sha = detect_pr_head_sha(pr_number)?;

    if args.skip_pr_bot {
        eprintln!("WARNING: bypassing deterministic pr-bot gate via --skip-pr-bot");
    } else {
        verify_pr_bot_gate(pr_number, &head_sha)?;
    }

    let gh_args = build_gh_merge_args(pr_number, args.rebase);
    let gh_arg_refs = gh_args.iter().map(String::as_str).collect::<Vec<_>>();
    run_checked("gh", &gh_arg_refs, "merge pull request")?;

    let marker_path = pr_bot_marker_path(pr_number, &head_sha)?;
    emit_merge_completed_event(pr_number, &head_sha, &marker_path);

    let default_branch = detect_default_branch();
    run_checked(
        "git",
        &["checkout", &default_branch],
        "checkout default branch",
    )?;
    run_checked(
        "git",
        &["pull", "origin", &default_branch],
        "pull default branch",
    )?;
    ensure_worktree_clean("post-merge", None)?;

    let current_branch = current_branch().unwrap_or(default_branch);
    println!("Merged PR #{pr_number}; current branch: {current_branch}");

    Ok(())
}

fn build_gh_merge_args(pr_number: u64, rebase: bool) -> Vec<String> {
    let merge_flag = if rebase { "--rebase" } else { "--merge" };
    vec![
        "pr".to_string(),
        "merge".to_string(),
        pr_number.to_string(),
        merge_flag.to_string(),
    ]
}

fn verify_pr_bot_gate(pr_number: u64, head_sha: &str) -> Result<()> {
    let repo_slug = detect_repo_slug()?;
    let marker_status =
        verify_pr_bot_marker(&pr_bot_marker_base_dir()?, &repo_slug, pr_number, head_sha);

    match marker_status {
        MarkerStatus::Verified => Ok(()),
        MarkerStatus::StaleMarkerExists => anyhow::bail!(
            "BLOCKED: pr-bot pass is stale for PR #{pr_number} at HEAD {head_sha}.\n\
             Run `csa plan run --sa-mode true --pattern pr-bot` for the current HEAD, or pass --skip-pr-bot for an emergency bypass."
        ),
        MarkerStatus::Missing => anyhow::bail!(
            "BLOCKED: pr-bot has not passed for PR #{pr_number} at HEAD {head_sha}.\n\
             Run `csa plan run --sa-mode true --pattern pr-bot` first, or pass --skip-pr-bot for an emergency bypass."
        ),
    }
}

fn pr_bot_marker_path(pr_number: u64, head_sha: &str) -> Result<PathBuf> {
    let repo_slug = detect_repo_slug()?;
    Ok(pr_bot_marker_base_dir()?
        .join(repo_slug)
        .join(format!("{pr_number}-{head_sha}.done")))
}

fn pr_bot_marker_base_dir() -> Result<PathBuf> {
    Ok(csa_config::paths::state_dir()
        .context("cannot determine CSA state directory")?
        .join("pr-bot-markers"))
}

fn detect_repo_slug() -> Result<String> {
    match run_checked_capture(
        "gh",
        &[
            "repo",
            "view",
            "--json",
            "nameWithOwner",
            "-q",
            ".nameWithOwner",
        ],
        "determine GitHub repository slug",
    ) {
        Ok(stdout) => {
            let slug = stdout.trim();
            if slug.is_empty() {
                anyhow::bail!("`gh repo view` returned an empty repository slug");
            }
            Ok(slug.replace('/', "_"))
        }
        Err(gh_err) => {
            let remote = run_checked_capture(
                "git",
                &["remote", "get-url", "origin"],
                "read origin remote URL",
            )?;
            parse_remote_repo_slug(&remote).ok_or_else(|| {
                anyhow::anyhow!(
                    "{gh_err:#}\nfailed to parse repository slug from `git remote get-url origin`: {}",
                    remote.trim()
                )
            })
        }
    }
}

fn detect_pr_head_sha(pr_number: u64) -> Result<String> {
    let pr_number = pr_number.to_string();
    let stdout = run_checked_capture(
        "gh",
        &[
            "pr",
            "view",
            &pr_number,
            "--json",
            "headRefOid",
            "-q",
            ".headRefOid",
        ],
        "determine PR head SHA",
    )?;
    let head_sha = stdout.trim();
    if head_sha.is_empty() {
        anyhow::bail!("`gh pr view {pr_number}` returned an empty head SHA");
    }
    Ok(head_sha.to_string())
}

pub(crate) fn parse_default_branch(remote_show_output: &str) -> Option<String> {
    remote_show_output.lines().find_map(|line| {
        let branch = line.trim().strip_prefix("HEAD branch:")?.trim();
        if branch.is_empty() || branch == "(unknown)" {
            None
        } else {
            Some(branch.to_string())
        }
    })
}

fn parse_remote_repo_slug(remote_url: &str) -> Option<String> {
    let trimmed = remote_url
        .trim()
        .strip_suffix(".git")
        .unwrap_or(remote_url.trim());
    let path = if let Some(rest) = trimmed.strip_prefix("ssh://") {
        rest.split_once('/')?.1
    } else if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        trimmed.split_once("://")?.1.split_once('/')?.1
    } else {
        trimmed.rsplit_once(':')?.1
    };

    let normalized = path.trim_matches('/');
    if normalized.is_empty() || !normalized.contains('/') {
        None
    } else {
        Some(normalized.replace('/', "_"))
    }
}

fn detect_default_branch() -> String {
    match Command::new("git")
        .args(["remote", "show", "origin"])
        .output()
    {
        Ok(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            parse_default_branch(&stdout).unwrap_or_else(|| {
                eprintln!(
                    "WARNING: could not parse origin default branch; falling back to `{DEFAULT_BRANCH_FALLBACK}`"
                );
                DEFAULT_BRANCH_FALLBACK.to_string()
            })
        }
        Ok(output) => {
            eprintln!(
                "WARNING: `git remote show origin` failed; falling back to `{DEFAULT_BRANCH_FALLBACK}`\n{}",
                command_output_text(&output)
            );
            DEFAULT_BRANCH_FALLBACK.to_string()
        }
        Err(err) => {
            eprintln!(
                "WARNING: failed to run `git remote show origin`: {err}; falling back to `{DEFAULT_BRANCH_FALLBACK}`"
            );
            DEFAULT_BRANCH_FALLBACK.to_string()
        }
    }
}

fn ensure_worktree_clean(phase: &str, hint: Option<&str>) -> Result<()> {
    let output = Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .with_context(|| "failed to run `git status --porcelain`")?;

    if !output.status.success() {
        anyhow::bail!(
            "`git status --porcelain` failed during {phase} check\n{}",
            command_output_text(&output)
        );
    }

    if !output.stdout.is_empty() {
        let status = String::from_utf8_lossy(&output.stdout);
        let hint_text = hint
            .map(|value| format!("\nHint: {value}."))
            .unwrap_or_default();
        anyhow::bail!("working tree is not clean during {phase} check{hint_text}\n{status}");
    }

    Ok(())
}

fn current_branch() -> Result<String> {
    let output = Command::new("git")
        .args(["branch", "--show-current"])
        .output()
        .with_context(|| "failed to run `git branch --show-current`")?;

    if !output.status.success() {
        anyhow::bail!(
            "`git branch --show-current` failed\n{}",
            command_output_text(&output)
        );
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn run_checked(program: &str, args: &[&str], action: &str) -> Result<()> {
    let output = Command::new(program)
        .args(args)
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

fn run_checked_capture(program: &str, args: &[&str], action: &str) -> Result<String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .with_context(|| format!("failed to run `{}`", shell_command(program, args)))?;

    if !output.status.success() {
        anyhow::bail!(
            "failed to {action} with `{}`\n{}",
            shell_command(program, args),
            command_output_text(&output)
        );
    }

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
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
    use super::{build_gh_merge_args, parse_default_branch, parse_remote_repo_slug};

    #[test]
    fn parses_origin_head_branch() {
        let output = r#"
* remote origin
  Fetch URL: git@github.com:owner/repo.git
  Push  URL: git@github.com:owner/repo.git
  HEAD branch: dev
  Remote branches:
    dev tracked
"#;

        assert_eq!(parse_default_branch(output).as_deref(), Some("dev"));
    }

    #[test]
    fn parses_slash_branch_names() {
        let output = "  HEAD branch: release/2026.05\n";

        assert_eq!(
            parse_default_branch(output).as_deref(),
            Some("release/2026.05")
        );
    }

    #[test]
    fn ignores_missing_or_unknown_head_branch() {
        assert_eq!(parse_default_branch("  Remote branches:\n"), None);
        assert_eq!(parse_default_branch("  HEAD branch: (unknown)\n"), None);
    }

    #[test]
    fn parses_repo_slug_from_https_remote() {
        assert_eq!(
            parse_remote_repo_slug("https://github.com/owner/repo.git").as_deref(),
            Some("owner_repo")
        );
    }

    #[test]
    fn parses_repo_slug_from_ssh_remote() {
        assert_eq!(
            parse_remote_repo_slug("git@github.com:owner/repo.git").as_deref(),
            Some("owner_repo")
        );
    }

    #[test]
    fn parses_repo_slug_from_ssh_scheme_remote() {
        assert_eq!(
            parse_remote_repo_slug("ssh://git@github.com/owner/repo.git").as_deref(),
            Some("owner_repo")
        );
    }

    #[test]
    fn rejects_unparseable_remote_slug() {
        assert_eq!(parse_remote_repo_slug("owner-only"), None);
    }

    #[test]
    fn gh_merge_args_use_merge_strategy_without_csa_only_flags() {
        assert_eq!(
            build_gh_merge_args(1334, false),
            vec!["pr", "merge", "1334", "--merge"]
        );
        assert_eq!(
            build_gh_merge_args(1334, true),
            vec!["pr", "merge", "1334", "--rebase"]
        );
    }
}
