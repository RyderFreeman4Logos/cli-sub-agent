use std::collections::BTreeSet;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;

use crate::cli::{ReviewArgs, ReviewDepth, ReviewMode};

const GIT_COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_RISK_DIFF_BYTES: usize = 200_000;
pub(super) const REVIEW_HISTORY_MAX_CHARS: usize = 2_000;
const REVIEW_HISTORY_FILES_MAX: usize = 20;
const REVIEW_HISTORY_COMMITS_PER_FILE: &str = "-5";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ReviewDepthAssessment {
    pub(super) depth: ReviewDepth,
    pub(super) auto_escalated: bool,
    pub(super) risk_signals: Vec<RiskyDiffSignal>,
}

impl ReviewDepthAssessment {
    pub(super) fn effective_review_mode(&self, requested: ReviewMode) -> ReviewMode {
        if self.depth == ReviewDepth::Audit {
            ReviewMode::RedTeam
        } else {
            requested
        }
    }

    pub(super) fn auto_escalation_summary(&self) -> Option<String> {
        if !self.auto_escalated || self.risk_signals.is_empty() {
            return None;
        }
        Some(
            self.risk_signals
                .iter()
                .map(|signal| signal.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum RiskyDiffSignal {
    ShellProcessSpawning,
    CommandOutputWithoutObviousTimeout,
    TenantWildcardFilter,
    StringSlicing,
    ToolRoutingMatchGuard,
    ResourceLimitCode,
}

impl RiskyDiffSignal {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::ShellProcessSpawning => "shell/process spawning patterns",
            Self::CommandOutputWithoutObviousTimeout => "Command::output usage",
            Self::TenantWildcardFilter => "tenant wildcard filter",
            Self::StringSlicing => "string slicing",
            Self::ToolRoutingMatchGuard => "ACP/tool routing match guard",
            Self::ResourceLimitCode => "resource limit code",
        }
    }
}

pub(super) async fn resolve_review_depth_for_project(
    args: &ReviewArgs,
    project_root: &Path,
    scope: &str,
) -> ReviewDepthAssessment {
    if args.depth == ReviewDepth::Audit {
        return ReviewDepthAssessment {
            depth: ReviewDepth::Audit,
            auto_escalated: false,
            risk_signals: Vec::new(),
        };
    }

    // Respect an explicit security-off request for standard-depth reviews. An
    // explicit `--depth audit --security-mode off` is rejected during CLI validation.
    if args.security_mode == "off" {
        return ReviewDepthAssessment {
            depth: ReviewDepth::Standard,
            auto_escalated: false,
            risk_signals: Vec::new(),
        };
    }

    let risk_signals = collect_review_diff_text(project_root, scope)
        .await
        .map(|diff| risky_signals_from_diff(&diff))
        .unwrap_or_default();

    assessment_from_risk_signals(risk_signals)
}

fn assessment_from_risk_signals(risk_signals: Vec<RiskyDiffSignal>) -> ReviewDepthAssessment {
    ReviewDepthAssessment {
        depth: if risk_signals.is_empty() {
            ReviewDepth::Standard
        } else {
            ReviewDepth::Audit
        },
        auto_escalated: !risk_signals.is_empty(),
        risk_signals,
    }
}

async fn collect_review_diff_text(project_root: &Path, scope: &str) -> Option<String> {
    if scope == "uncommitted" {
        return run_git_stdout(
            project_root,
            &["diff", "HEAD", "--no-color", "--unified=0"],
            MAX_RISK_DIFF_BYTES,
        )
        .await;
    }

    if let Some(range) = scope.strip_prefix("range:") {
        return run_git_stdout(
            project_root,
            &["diff", "--no-color", "--unified=0", range],
            MAX_RISK_DIFF_BYTES,
        )
        .await;
    }

    if let Some(base) = scope.strip_prefix("base:") {
        let range = format!("{base}...HEAD");
        return run_git_stdout(
            project_root,
            &["diff", "--no-color", "--unified=0", &range],
            MAX_RISK_DIFF_BYTES,
        )
        .await;
    }

    if let Some(commit) = scope.strip_prefix("commit:") {
        return run_git_stdout(
            project_root,
            &["show", "--no-color", "--unified=0", commit],
            MAX_RISK_DIFF_BYTES,
        )
        .await;
    }

    if let Some(pathspec) = scope.strip_prefix("files:") {
        return run_git_stdout(
            project_root,
            &["diff", "--no-color", "--unified=0", "--", pathspec],
            MAX_RISK_DIFF_BYTES,
        )
        .await;
    }

    None
}

pub(super) fn risky_signals_from_diff(diff: &str) -> Vec<RiskyDiffSignal> {
    let mut signals = BTreeSet::new();
    let lower = diff.to_ascii_lowercase();

    if diff.contains("Command::new(")
        || diff.contains("std::process::Command")
        || diff.contains("tokio::process::Command")
        || lower.contains("sh -c")
        || lower.contains("bash -c")
    {
        signals.insert(RiskyDiffSignal::ShellProcessSpawning);
    }
    if diff.contains(".output().await")
        || diff.contains(".output()")
        || diff.contains("Command::output")
    {
        signals.insert(RiskyDiffSignal::CommandOutputWithoutObviousTimeout);
    }
    if diff.lines().any(line_has_wildcard_true_arm) {
        signals.insert(RiskyDiffSignal::TenantWildcardFilter);
    }
    if diff.contains("[..") || diff.contains("..]") {
        signals.insert(RiskyDiffSignal::StringSlicing);
    }
    if (lower.contains("match")
        && (lower.contains("toolname")
            || lower.contains("tool routing")
            || lower.contains("transport")
            || lower.contains("acp")
            || lower.contains("fallback")
            || lower.contains("model_spec")))
        || lower.contains("routing match")
    {
        signals.insert(RiskyDiffSignal::ToolRoutingMatchGuard);
    }
    if lower.contains("cgroup")
        || lower.contains("container")
        || lower.contains("memory_max")
        || lower.contains("min_free_memory")
        || lower.contains("pids_max")
        || lower.contains("resource limit")
        || lower.contains("ulimit")
    {
        signals.insert(RiskyDiffSignal::ResourceLimitCode);
    }

    signals.into_iter().collect()
}

fn line_has_wildcard_true_arm(line: &str) -> bool {
    let compact = line.split_whitespace().collect::<String>();
    compact.contains("_=>true")
}

pub(super) async fn collect_bounded_regression_context(
    project_root: &Path,
    scope: &str,
) -> Option<String> {
    let changed_files = collect_changed_files(project_root, scope).await;
    if changed_files.is_empty() {
        return None;
    }

    let mut entries = Vec::new();
    for file in changed_files.iter().take(REVIEW_HISTORY_FILES_MAX) {
        let log = run_git_stdout(
            project_root,
            &[
                "log",
                "--oneline",
                REVIEW_HISTORY_COMMITS_PER_FILE,
                "--",
                file,
            ],
            REVIEW_HISTORY_MAX_CHARS,
        )
        .await
        .unwrap_or_default();
        let commits = log
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>();
        if !commits.is_empty() {
            entries.push((file.clone(), commits));
        }
    }

    format_bounded_review_history(&entries, REVIEW_HISTORY_MAX_CHARS)
}

async fn collect_changed_files(project_root: &Path, scope: &str) -> Vec<String> {
    let mut files = BTreeSet::new();
    if scope == "uncommitted" {
        insert_name_only_output(
            &mut files,
            run_git_stdout(project_root, &["diff", "--name-only", "HEAD"], 50_000).await,
        );
        insert_name_only_output(
            &mut files,
            run_git_stdout(
                project_root,
                &["ls-files", "--others", "--exclude-standard"],
                50_000,
            )
            .await,
        );
    } else if let Some(range) = scope.strip_prefix("range:") {
        insert_name_only_output(
            &mut files,
            run_git_stdout(project_root, &["diff", "--name-only", range], 50_000).await,
        );
    } else if let Some(base) = scope.strip_prefix("base:") {
        let range = format!("{base}...HEAD");
        insert_name_only_output(
            &mut files,
            run_git_stdout(project_root, &["diff", "--name-only", &range], 50_000).await,
        );
    } else if let Some(commit) = scope.strip_prefix("commit:") {
        insert_name_only_output(
            &mut files,
            run_git_stdout(
                project_root,
                &["diff-tree", "--no-commit-id", "--name-only", "-r", commit],
                50_000,
            )
            .await,
        );
    } else if let Some(pathspec) = scope.strip_prefix("files:") {
        insert_name_only_output(
            &mut files,
            run_git_stdout(
                project_root,
                &["diff", "--name-only", "--", pathspec],
                50_000,
            )
            .await,
        );
    }

    files.into_iter().collect()
}

fn insert_name_only_output(files: &mut BTreeSet<String>, output: Option<String>) {
    let Some(output) = output else {
        return;
    };
    files.extend(
        output
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string),
    );
}

pub(super) fn format_bounded_review_history(
    entries: &[(String, Vec<String>)],
    max_chars: usize,
) -> Option<String> {
    if entries.is_empty() || max_chars == 0 {
        return None;
    }

    let mut rendered =
        String::from("Recent commit history for changed files (regression context):\n");
    for (path, commits) in entries {
        if commits.is_empty() {
            continue;
        }
        rendered.push_str("- ");
        rendered.push_str(path);
        rendered.push_str(":\n");
        for commit in commits {
            rendered.push_str("  ");
            rendered.push_str(commit);
            rendered.push('\n');
        }
        if rendered.len() >= max_chars {
            break;
        }
    }

    let rendered = truncate_to_char_boundary(&rendered, max_chars)
        .trim_end()
        .to_string();
    (!rendered.is_empty()).then_some(rendered)
}

async fn run_git_stdout(project_root: &Path, args: &[&str], max_bytes: usize) -> Option<String> {
    let mut command = Command::new("git");
    command
        .args(args)
        .current_dir(project_root)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let output = tokio::time::timeout(GIT_COMMAND_TIMEOUT, command.output())
        .await
        .ok()?
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let stdout = if output.stdout.len() > max_bytes {
        &output.stdout[..max_bytes]
    } else {
        &output.stdout
    };
    Some(String::from_utf8_lossy(stdout).into_owned())
}

fn truncate_to_char_boundary(text: &str, max_chars: usize) -> &str {
    if text.len() <= max_chars {
        return text;
    }
    let mut end = max_chars;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn risky_signals_detect_audit_categories() {
        let diff = r#"
+ let child = tokio::process::Command::new("git").output().await?;
+ match tenant {
+     "known" => true,
+     _ => true,
+ }
+ let prefix = &name[..12];
+ match tool_name {
+     ToolName::Codex => fallback(),
+ }
+ let memory_max_mb = cgroup_limit();
"#;

        let signals = risky_signals_from_diff(diff);

        assert!(signals.contains(&RiskyDiffSignal::ShellProcessSpawning));
        assert!(signals.contains(&RiskyDiffSignal::CommandOutputWithoutObviousTimeout));
        assert!(signals.contains(&RiskyDiffSignal::TenantWildcardFilter));
        assert!(signals.contains(&RiskyDiffSignal::StringSlicing));
        assert!(signals.contains(&RiskyDiffSignal::ToolRoutingMatchGuard));
        assert!(signals.contains(&RiskyDiffSignal::ResourceLimitCode));
    }

    #[test]
    fn bounded_history_formatter_caps_output() {
        let entries = vec![(
            "src/lib.rs".to_string(),
            vec![
                "abc1234 one subject".to_string(),
                "def5678 second subject".to_string(),
                "0123456 third subject".to_string(),
            ],
        )];

        let rendered = format_bounded_review_history(&entries, 80).expect("history");

        assert!(rendered.len() <= 80);
        assert!(rendered.starts_with("Recent commit history for changed files"));
    }

    #[test]
    fn risky_signal_detection_escalates_to_audit() {
        let assessment =
            assessment_from_risk_signals(vec![RiskyDiffSignal::CommandOutputWithoutObviousTimeout]);

        assert_eq!(assessment.depth, ReviewDepth::Audit);
        assert!(assessment.auto_escalated);
        assert_eq!(
            assessment.effective_review_mode(ReviewMode::Standard),
            ReviewMode::RedTeam
        );
    }

    #[tokio::test]
    async fn regression_context_collects_recent_git_log_for_changed_files() {
        let temp = tempfile::tempdir().expect("create temp repo");
        run_git(temp.path(), &["init"]);
        run_git(temp.path(), &["config", "user.email", "test@example.com"]);
        run_git(temp.path(), &["config", "user.name", "Test User"]);
        std::fs::write(temp.path().join("tracked.rs"), "fn main() {}\n").expect("write file");
        run_git(temp.path(), &["add", "tracked.rs"]);
        run_git(temp.path(), &["commit", "-m", "initial tracked file"]);
        std::fs::write(
            temp.path().join("tracked.rs"),
            "fn main() { println!(\"hi\"); }\n",
        )
        .expect("modify file");

        let context = collect_bounded_regression_context(temp.path(), "uncommitted")
            .await
            .expect("regression context");

        assert!(context.len() <= REVIEW_HISTORY_MAX_CHARS);
        assert!(context.contains("Recent commit history for changed files"));
        assert!(context.contains("tracked.rs"));
        assert!(context.contains("initial tracked file"));
    }

    fn run_git(repo: &Path, args: &[&str]) {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .expect("git command should execute");
        assert!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
