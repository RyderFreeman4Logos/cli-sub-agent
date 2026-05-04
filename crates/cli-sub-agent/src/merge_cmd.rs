use std::process::{Command, Output};

use anyhow::{Context, Result};

use crate::cli::MergeArgs;

const DEFAULT_BRANCH_FALLBACK: &str = "main";

pub(crate) fn handle_merge(args: MergeArgs) -> Result<()> {
    if !args.force {
        ensure_worktree_clean(
            "pre-merge",
            Some("pass --force to bypass this pre-merge check"),
        )?;
    }

    let pr_number = args.pr_number.to_string();
    let merge_flag = if args.rebase { "--rebase" } else { "--merge" };
    run_checked(
        "gh",
        &["pr", "merge", &pr_number, merge_flag],
        "merge pull request",
    )?;

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
    println!(
        "Merged PR #{}; current branch: {current_branch}",
        args.pr_number
    );

    Ok(())
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
    use super::parse_default_branch;

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
}
