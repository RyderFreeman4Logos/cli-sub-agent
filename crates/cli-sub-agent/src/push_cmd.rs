use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use csa_core::types::ReviewDecision;
use serde_json::Value;

use crate::cli::PushArgs;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedReviewVerdict {
    decision: ReviewDecision,
    severity_total: u32,
    inline_findings_count: usize,
}

#[derive(Debug, Clone)]
struct ReviewSessionCandidate {
    session_id: String,
    session_dir: PathBuf,
    result_completed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
struct ReviewCoverage {
    session_id: String,
}

pub(crate) fn handle_push(args: PushArgs) -> Result<()> {
    let current_branch = current_branch()?;
    let remote = args.remote.as_deref().unwrap_or("origin");
    let refspec = args.refspec.as_deref().unwrap_or(&current_branch);

    if args.force {
        if args.check_only {
            println!("Review gate bypassed by --force.");
            return Ok(());
        }
        return run_git_push(remote, refspec, &args);
    }

    let coverage = check_review_gate(&current_branch)?;
    if args.check_only {
        println!(
            "Review gate passed for branch {} via session {}.",
            current_branch, coverage.session_id
        );
        return Ok(());
    }

    run_git_push(remote, refspec, &args)
}

fn check_review_gate(branch: &str) -> Result<ReviewCoverage> {
    let head = git_output(["rev-parse", "HEAD"])?;
    let project_root = std::env::current_dir().context("Failed to determine current directory")?;
    let Some(candidate) = find_latest_review_session(&project_root, branch)? else {
        bail!("No review session found for branch {branch}. Run 'csa review' first.");
    };

    let verdict = read_review_verdict(&candidate.session_dir)?;
    let findings_count = read_findings_count(&candidate.session_dir)?;
    let reviewed_head = read_reviewed_head(&candidate.session_dir)?;

    if let Some(reviewed_head) = reviewed_head.as_deref() {
        if reviewed_head != head {
            bail!(
                "HEAD {head} is ahead of last reviewed commit {reviewed_head}. Run 'csa review' before pushing."
            );
        }
    } else if let Some(completed_at) = candidate.result_completed_at {
        if branch_has_commits_after(completed_at)? {
            let reviewed = latest_branch_commit_at_or_before(completed_at)?
                .unwrap_or_else(|| "unknown".to_string());
            bail!(
                "HEAD {head} is ahead of last reviewed commit {reviewed}. Run 'csa review' before pushing."
            );
        }
    } else {
        bail!(
            "Review session {} has no reviewed HEAD or completion timestamp; run 'csa review' before pushing.",
            candidate.session_id
        );
    }

    if verdict_is_allowed(&verdict, findings_count) {
        Ok(ReviewCoverage {
            session_id: candidate.session_id,
        })
    } else {
        bail!(
            "Last review session {} did not pass. Run 'csa review' and fix reported findings before pushing.",
            candidate.session_id
        );
    }
}

fn find_latest_review_session(
    project_root: &Path,
    branch: &str,
) -> Result<Option<ReviewSessionCandidate>> {
    let session_root = csa_session::get_session_root(project_root)?;
    let sessions_dir = session_root.join("sessions");
    if !sessions_dir.exists() {
        return Ok(None);
    }

    let mut entries = fs::read_dir(&sessions_dir)
        .with_context(|| {
            format!(
                "Failed to read sessions directory: {}",
                sessions_dir.display()
            )
        })?
        .collect::<std::result::Result<Vec<_>, _>>()
        .with_context(|| {
            format!(
                "Failed to read sessions directory: {}",
                sessions_dir.display()
            )
        })?;

    entries.sort_by_key(|entry| std::cmp::Reverse(entry.file_name()));

    for entry in entries {
        if !entry
            .file_type()
            .with_context(|| {
                format!(
                    "Failed to inspect session entry: {}",
                    entry.path().display()
                )
            })?
            .is_dir()
        {
            continue;
        }

        let session_id = entry.file_name().to_string_lossy().to_string();
        let session_dir = entry.path();
        if !session_dir
            .join("output")
            .join("review-verdict.json")
            .exists()
        {
            continue;
        }

        let state = match csa_session::load_session(project_root, &session_id) {
            Ok(state) => state,
            Err(err) => {
                tracing::debug!(session_id, error = %err, "Skipping unreadable session state");
                continue;
            }
        };

        if !session_matches_branch_or_description(&state, branch) {
            continue;
        }
        if !session_looks_like_review(&state, &session_dir) {
            continue;
        }

        let result_completed_at = read_session_result_completed_at(&session_dir)?;
        return Ok(Some(ReviewSessionCandidate {
            session_id,
            session_dir,
            result_completed_at,
        }));
    }

    Ok(None)
}

fn session_matches_branch_or_description(
    state: &csa_session::MetaSessionState,
    branch: &str,
) -> bool {
    state.branch.as_deref() == Some(branch)
        || state
            .description
            .as_deref()
            .is_some_and(|description| description.contains(branch))
}

fn session_looks_like_review(state: &csa_session::MetaSessionState, session_dir: &Path) -> bool {
    state.task_context.task_type.as_deref() == Some("review")
        || state
            .description
            .as_deref()
            .is_some_and(|description| description.to_ascii_lowercase().contains("review"))
        || session_dir.join("review_meta.json").exists()
}

fn read_session_result_completed_at(session_dir: &Path) -> Result<Option<DateTime<Utc>>> {
    let result_path = session_dir.join(csa_session::result::RESULT_FILE_NAME);
    if !result_path.exists() {
        return Ok(None);
    }
    let text = fs::read_to_string(&result_path)
        .with_context(|| format!("Failed to read {}", result_path.display()))?;
    let result: csa_session::SessionResult = toml::from_str(&text)
        .with_context(|| format!("Failed to parse {}", result_path.display()))?;
    Ok(Some(result.completed_at))
}

fn read_reviewed_head(session_dir: &Path) -> Result<Option<String>> {
    let meta_path = session_dir.join("review_meta.json");
    if !meta_path.exists() {
        return Ok(None);
    }
    let text = fs::read_to_string(&meta_path)
        .with_context(|| format!("Failed to read {}", meta_path.display()))?;
    let meta: csa_session::ReviewSessionMeta = serde_json::from_str(&text)
        .with_context(|| format!("Failed to parse {}", meta_path.display()))?;
    Ok(Some(meta.head_sha))
}

fn read_review_verdict(session_dir: &Path) -> Result<ParsedReviewVerdict> {
    let path = session_dir.join("output").join("review-verdict.json");
    let text =
        fs::read_to_string(&path).with_context(|| format!("Failed to read {}", path.display()))?;
    parse_review_verdict(&text).with_context(|| format!("Failed to parse {}", path.display()))
}

pub(crate) fn parse_review_verdict(text: &str) -> Result<ParsedReviewVerdict> {
    let value: Value = serde_json::from_str(text).context("review verdict is not valid JSON")?;
    let decision = parse_decision(&value)?;
    let severity_total = value
        .get("severity_counts")
        .and_then(Value::as_object)
        .map(|counts| {
            counts
                .values()
                .filter_map(Value::as_u64)
                .map(|count| count as u32)
                .sum()
        })
        .unwrap_or(0);
    let inline_findings_count = value
        .get("findings")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);

    Ok(ParsedReviewVerdict {
        decision,
        severity_total,
        inline_findings_count,
    })
}

fn parse_decision(value: &Value) -> Result<ReviewDecision> {
    for key in ["decision", "verdict", "verdict_legacy"] {
        if let Some(decision) = value.get(key).and_then(Value::as_str) {
            return decision
                .parse()
                .map_err(anyhow::Error::msg)
                .with_context(|| format!("Invalid review decision in `{key}`"));
        }
    }
    bail!("Missing review decision; expected `decision`, `verdict`, or `verdict_legacy`");
}

fn read_findings_count(session_dir: &Path) -> Result<usize> {
    let path = session_dir.join("output").join("findings.toml");
    if !path.exists() {
        return Ok(0);
    }
    let text =
        fs::read_to_string(&path).with_context(|| format!("Failed to read {}", path.display()))?;
    let findings: csa_session::FindingsFile =
        toml::from_str(&text).with_context(|| format!("Failed to parse {}", path.display()))?;
    Ok(findings.findings.len())
}

pub(crate) fn verdict_is_allowed(verdict: &ParsedReviewVerdict, findings_count: usize) -> bool {
    matches!(verdict.decision, ReviewDecision::Pass)
        || (matches!(verdict.decision, ReviewDecision::Fail)
            && verdict.severity_total == 0
            && verdict.inline_findings_count == 0
            && findings_count == 0)
}

fn branch_has_commits_after(completed_at: DateTime<Utc>) -> Result<bool> {
    let after = completed_at.to_rfc3339();
    let output = git_output(["log", "--after", after.as_str(), "--oneline", "main...HEAD"])?;
    Ok(!output.trim().is_empty())
}

fn latest_branch_commit_at_or_before(completed_at: DateTime<Utc>) -> Result<Option<String>> {
    let before = completed_at.to_rfc3339();
    let output = git_output([
        "log",
        "-1",
        "--format=%H",
        "--before",
        before.as_str(),
        "main...HEAD",
    ])?;
    let trimmed = output.trim();
    Ok((!trimmed.is_empty()).then(|| trimmed.to_string()))
}

fn current_branch() -> Result<String> {
    let branch = git_output(["rev-parse", "--abbrev-ref", "HEAD"])?;
    let branch = branch.trim();
    if branch == "HEAD" {
        bail!("Cannot run `csa push` from detached HEAD.");
    }
    Ok(branch.to_string())
}

fn run_git_push(remote: &str, refspec: &str, args: &PushArgs) -> Result<()> {
    let mut command = Command::new("git");
    command.arg("push");
    if args.force {
        command.arg("--force");
    }
    if args.force_with_lease {
        command.arg("--force-with-lease");
    }
    command.arg(remote).arg(refspec);
    command.args(&args.passthrough);

    let status = command.status().context("Failed to execute git push")?;
    if !status.success() {
        bail!("git push failed with status {status}");
    }
    Ok(())
}

fn git_output<const N: usize>(args: [&str; N]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .output()
        .context("Failed to execute git")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("git {} failed: {}", args.join(" "), stderr.trim());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_review_verdict_reads_decision_and_counts() {
        let verdict = parse_review_verdict(
            r#"{
                "decision": "pass",
                "severity_counts": {"critical": 0, "high": 1, "medium": 0, "low": 0},
                "findings": [{"id": "F1"}]
            }"#,
        )
        .expect("verdict should parse");

        assert_eq!(verdict.decision, ReviewDecision::Pass);
        assert_eq!(verdict.severity_total, 1);
        assert_eq!(verdict.inline_findings_count, 1);
    }

    #[test]
    fn parse_review_verdict_accepts_legacy_verdict() {
        let verdict = parse_review_verdict(
            r#"{
                "verdict": "CLEAN",
                "severity_counts": {"critical": 0, "high": 0, "medium": 0, "low": 0}
            }"#,
        )
        .expect("legacy verdict should parse");

        assert_eq!(verdict.decision, ReviewDecision::Pass);
        assert_eq!(verdict.severity_total, 0);
        assert_eq!(verdict.inline_findings_count, 0);
    }

    #[test]
    fn false_positive_failure_with_no_counts_or_findings_is_allowed() {
        let verdict = parse_review_verdict(
            r#"{
                "decision": "fail",
                "severity_counts": {"critical": 0, "high": 0, "medium": 0, "low": 0},
                "findings": []
            }"#,
        )
        .expect("verdict should parse");

        assert!(verdict_is_allowed(&verdict, 0));
        assert!(!verdict_is_allowed(&verdict, 1));
    }

    #[test]
    fn fail_with_actual_counts_is_blocked() {
        let verdict = parse_review_verdict(
            r#"{
                "decision": "fail",
                "severity_counts": {"critical": 0, "high": 1, "medium": 0, "low": 0},
                "findings": []
            }"#,
        )
        .expect("verdict should parse");

        assert!(!verdict_is_allowed(&verdict, 0));
    }

    #[test]
    fn push_cli_parses_check_only() {
        use clap::Parser;

        let cli = crate::cli::Cli::try_parse_from(["csa", "push", "--check-only"]).unwrap();
        match cli.command {
            crate::cli::Commands::Push(args) => {
                assert!(args.check_only);
                assert_eq!(args.remote, None);
                assert_eq!(args.refspec, None);
            }
            _ => panic!("expected push command"),
        }
    }
}
