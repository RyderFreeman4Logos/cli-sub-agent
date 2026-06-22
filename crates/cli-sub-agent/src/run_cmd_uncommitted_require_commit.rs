use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone)]
pub(super) enum DirtyTrackedWorktree {
    Clean,
    Dirty(csa_session::UncommittedChanges),
    Unknown { blocker_summary: String },
}

impl DirtyTrackedWorktree {
    pub(super) fn is_clean(&self) -> bool {
        matches!(self, Self::Clean)
    }

    pub(super) fn changes(&self) -> Option<&csa_session::UncommittedChanges> {
        match self {
            Self::Dirty(changes) => Some(changes),
            Self::Clean | Self::Unknown { .. } => None,
        }
    }

    pub(super) fn blocker_summary(&self) -> Option<&str> {
        match self {
            Self::Unknown { blocker_summary } => Some(blocker_summary.as_str()),
            Self::Clean | Self::Dirty(_) => None,
        }
    }
}

pub(super) fn build_blocker_summary(
    result: &csa_session::SessionResult,
    gate_failure: Option<&str>,
    clean_tree_verification_failure: Option<&str>,
) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(gate_failure) = gate_failure
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        parts.push(format!("gate={gate_failure}"));
    }
    if let Some(clean_tree_verification_failure) = clean_tree_verification_failure
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        parts.push(format!(
            "clean_tree_verification={clean_tree_verification_failure}"
        ));
    }
    let trimmed_summary = result.summary.trim();
    let summary = trimmed_summary
        .strip_prefix("Summary: ")
        .unwrap_or(trimmed_summary)
        .trim();
    if !summary.is_empty() && summary != super::REQUIRE_COMMIT_REASON {
        parts.push(format!("summary={summary}"));
    }
    if parts.is_empty() {
        return None;
    }
    Some(bound_redacted_one_line(
        &parts.join("; "),
        super::REQUIRE_COMMIT_BLOCKER_SUMMARY_MAX_CHARS,
    ))
}

fn bound_redacted_one_line(value: &str, max_chars: usize) -> String {
    let redacted = csa_session::redact_text_content(value);
    let one_line = redacted.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() <= max_chars {
        return one_line;
    }
    let keep_chars = max_chars.saturating_sub(3);
    let mut truncated = one_line.chars().take(keep_chars).collect::<String>();
    truncated = truncated.trim_end().to_string();
    truncated.push_str("...");
    truncated
}

pub(super) fn inspect_dirty_tracked_changes(project_root: &Path) -> DirtyTrackedWorktree {
    let porcelain = match run_git_status_porcelain(
        project_root,
        &[
            "status",
            "--porcelain=v1",
            "--untracked-files=no",
            "--no-renames",
            "-z",
        ],
    ) {
        Ok(porcelain) => porcelain,
        Err(blocker_summary) => {
            return DirtyTrackedWorktree::Unknown { blocker_summary };
        }
    };
    if porcelain.is_empty() {
        return DirtyTrackedWorktree::Clean;
    }
    let numstat = super::run_git_diff_capture(project_root, &["diff", "--numstat", "HEAD"], None)
        .unwrap_or_default();
    match super::summarize_uncommitted_changes_with_stats(&porcelain, &numstat, 0, 0, None) {
        Some(changes) => DirtyTrackedWorktree::Dirty(changes),
        None => DirtyTrackedWorktree::Unknown {
            blocker_summary: "git-status-probe-unparseable".to_string(),
        },
    }
}

fn run_git_status_porcelain(project_root: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(args)
        .output()
        .map_err(|_| "git-status-probe-spawn-failed".to_string())?;
    if !output.status.success() {
        let exit_code = output
            .status
            .code()
            .map(|code| code.to_string())
            .unwrap_or_else(|| "unknown".to_string());
        return Err(format!("git-status-probe-failed exit_code={exit_code}"));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}
