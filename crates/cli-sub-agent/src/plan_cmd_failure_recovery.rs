use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};

const PR_BOT_WORKFLOW_NAME: &str = "pr-bot";
const DEV2MERGE_WORKFLOW_NAME: &str = "dev2merge";
pub(super) const WEAVE_LOCK: &str = "weave.lock";
const CSA_PLAN_STATE_PREFIX: &str = ".csa/state/plan/";

pub(crate) fn capture_failure_recovery_snapshot(
    workflow_name: &str,
    project_root: &Path,
) -> Option<PlanFailureRecoverySnapshot> {
    matches!(
        workflow_name,
        PR_BOT_WORKFLOW_NAME | DEV2MERGE_WORKFLOW_NAME
    )
    .then(|| PlanFailureRecoverySnapshot::capture_for_workflow(project_root, workflow_name))
}

#[derive(Debug, Clone)]
pub(crate) struct PlanFailureRecoverySnapshot {
    workflow_name: String,
    initial_ref: Option<CheckoutRef>,
    initial_status: Option<Vec<String>>,
    capture_error: Option<String>,
}

impl PlanFailureRecoverySnapshot {
    #[cfg(test)]
    pub(super) fn capture(project_root: &Path) -> Self {
        Self::capture_for_workflow(project_root, PR_BOT_WORKFLOW_NAME)
    }

    fn capture_for_workflow(project_root: &Path, workflow_name: &str) -> Self {
        let workflow_name = workflow_name.to_string();
        if let Err(error) = run_git(project_root, &["rev-parse", "--is-inside-work-tree"]) {
            return Self {
                workflow_name,
                initial_ref: None,
                initial_status: None,
                capture_error: Some(format!("not a git worktree: {error}")),
            };
        }

        let initial_ref = match current_checkout_ref(project_root) {
            Ok(value) => Some(value),
            Err(error) => {
                return Self {
                    workflow_name,
                    initial_ref: None,
                    initial_status: None,
                    capture_error: Some(format!("failed to capture initial checkout: {error}")),
                };
            }
        };
        let initial_status = match git_recovery_status_lines(project_root) {
            Ok(value) => Some(value),
            Err(error) => {
                return Self {
                    workflow_name,
                    initial_ref,
                    initial_status: None,
                    capture_error: Some(format!("failed to capture initial git status: {error}")),
                };
            }
        };

        Self {
            workflow_name,
            initial_ref,
            initial_status,
            capture_error: None,
        }
    }

    pub(crate) fn recover_after_failure(&self, project_root: &Path) -> PlanFailureRecoveryReport {
        let initial_ref_label = self.initial_ref.as_ref().map(CheckoutRef::label);
        let current_ref_before = current_checkout_ref(project_root)
            .ok()
            .map(|value| value.label());

        if let Some(error) = &self.capture_error {
            return PlanFailureRecoveryReport::manual(
                initial_ref_label,
                current_ref_before,
                vec![format!(
                    "Automatic recovery skipped because the initial checkout snapshot failed: {error}."
                )],
                vec!["git status --short --branch".to_string()],
            );
        }

        let Some(initial_status) = &self.initial_status else {
            return PlanFailureRecoveryReport::manual(
                initial_ref_label,
                current_ref_before,
                vec![
                    "Automatic recovery skipped because the initial git status is unavailable."
                        .to_string(),
                ],
                vec!["git status --short --branch".to_string()],
            );
        };
        if !initial_status.is_empty() {
            let pre_existing = initial_status
                .iter()
                .map(|line| line.trim_start())
                .collect::<Vec<_>>()
                .join("; ");
            return PlanFailureRecoveryReport::manual(
                initial_ref_label.clone(),
                current_ref_before,
                vec![
                    format!(
                        "Automatic recovery skipped because the worktree was already dirty before {} started.",
                        self.workflow_name
                    ),
                    format!("Pre-existing dirty paths: {pre_existing}"),
                    format!(
                        "Resolve the pre-existing changes before retrying {}.",
                        self.workflow_name
                    ),
                ],
                recovery_commands(initial_ref_label.as_deref()),
            );
        }

        let mut actions = Vec::new();
        let status_before_cleanup = match git_recovery_status_lines(project_root) {
            Ok(lines) => lines,
            Err(error) => {
                return PlanFailureRecoveryReport::manual(
                    initial_ref_label.clone(),
                    current_ref_before,
                    vec![format!(
                        "Could not inspect failed worktree status: {error}."
                    )],
                    recovery_commands(initial_ref_label.as_deref()),
                );
            }
        };

        let weave_lock_changed_after_snapshot = only_weave_lock_dirty(&status_before_cleanup);
        if weave_lock_changed_after_snapshot {
            actions.push(format!(
                "Preserved dirty weave.lock because automatic recovery cannot prove it was \
                 created by this {} run.",
                self.workflow_name
            ));
        } else if !status_before_cleanup.is_empty() {
            return PlanFailureRecoveryReport::manual(
                initial_ref_label.clone(),
                current_ref_before,
                vec![
                    format!(
                        "Automatic recovery will not modify dirty paths that it cannot prove were created by {}.",
                        self.workflow_name
                    ),
                    format!(
                        "Current dirty paths require manual review: {}",
                        status_before_cleanup.join("; ")
                    ),
                ],
                recovery_commands(initial_ref_label.as_deref()),
            );
        }

        if let Some(initial_ref) = &self.initial_ref {
            match current_checkout_ref(project_root) {
                Ok(current_ref) if &current_ref == initial_ref => {}
                Ok(_) | Err(_) => match restore_checkout(project_root, initial_ref) {
                    Ok(()) => {
                        actions.push(format!("Restored checkout to {}.", initial_ref.label()))
                    }
                    Err(error) => {
                        let mut messages = actions;
                        messages.push(format!(
                            "Could not restore checkout to {} automatically: {error}.",
                            initial_ref.label()
                        ));
                        return PlanFailureRecoveryReport::manual(
                            initial_ref_label.clone(),
                            current_ref_before,
                            messages,
                            recovery_commands(initial_ref_label.as_deref()),
                        );
                    }
                },
            }
        }

        let final_status = git_recovery_status_lines(project_root).unwrap_or_else(|error| {
            vec![format!(
                "failed to inspect final status after recovery: {error}"
            )]
        });
        let current_ref_after = current_checkout_ref(project_root)
            .ok()
            .map(|value| value.label());
        let checkout_restored = match &self.initial_ref {
            Some(initial_ref) => {
                current_checkout_ref(project_root).is_ok_and(|current| &current == initial_ref)
            }
            None => true,
        };
        if final_status.is_empty() && checkout_restored {
            let status = if actions.is_empty() {
                PlanRecoveryStatus::NotNeeded
            } else {
                PlanRecoveryStatus::Restored
            };
            return PlanFailureRecoveryReport {
                status,
                initial_ref: initial_ref_label,
                current_ref_before,
                current_ref_after,
                messages: if actions.is_empty() {
                    vec!["No checkout or weave.lock cleanup was needed.".to_string()]
                } else {
                    actions
                },
                recovery_commands: Vec::new(),
                final_status,
            };
        }
        if weave_lock_changed_after_snapshot
            && checkout_restored
            && only_weave_lock_dirty(&final_status)
        {
            let mut messages = actions;
            messages.push(format!(
                "Review weave.lock before retrying {}.",
                self.workflow_name
            ));
            return PlanFailureRecoveryReport::manual_with_refs(
                initial_ref_label,
                current_ref_before,
                current_ref_after,
                messages,
                vec!["git status --short --branch".to_string()],
                final_status,
            );
        }

        PlanFailureRecoveryReport::manual_with_refs(
            initial_ref_label,
            current_ref_before,
            current_ref_after,
            vec![
                "Automatic recovery ran but the checkout or worktree is still not restored."
                    .to_string(),
            ],
            vec!["git status --short --branch".to_string()],
            final_status,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CheckoutRef {
    Branch(String),
    Detached(String),
}

impl CheckoutRef {
    fn label(&self) -> String {
        match self {
            Self::Branch(branch) => branch.clone(),
            Self::Detached(head) => head.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct PlanFailureRecoveryReport {
    pub(super) status: PlanRecoveryStatus,
    pub(super) initial_ref: Option<String>,
    pub(super) current_ref_before: Option<String>,
    pub(super) current_ref_after: Option<String>,
    pub(super) messages: Vec<String>,
    pub(super) recovery_commands: Vec<String>,
    pub(super) final_status: Vec<String>,
}

impl PlanFailureRecoveryReport {
    fn manual(
        initial_ref: Option<String>,
        current_ref: Option<String>,
        messages: Vec<String>,
        recovery_commands: Vec<String>,
    ) -> Self {
        Self::manual_with_refs(
            initial_ref,
            current_ref.clone(),
            current_ref,
            messages,
            recovery_commands,
            Vec::new(),
        )
    }

    fn manual_with_refs(
        initial_ref: Option<String>,
        current_ref_before: Option<String>,
        current_ref_after: Option<String>,
        messages: Vec<String>,
        recovery_commands: Vec<String>,
        final_status: Vec<String>,
    ) -> Self {
        Self {
            status: PlanRecoveryStatus::ManualRequired,
            initial_ref,
            current_ref_before,
            current_ref_after,
            messages,
            recovery_commands,
            final_status,
        }
    }

    pub(super) fn render_markdown_into(&self, output: &mut String) {
        output.push_str(&format!("- Recovery status: `{}`\n", self.status.as_str()));
        if let Some(initial_ref) = &self.initial_ref {
            output.push_str(&format!("- Initial checkout: `{initial_ref}`\n"));
        }
        if let Some(current_ref) = &self.current_ref_before {
            output.push_str(&format!("- Checkout at failure: `{current_ref}`\n"));
        }
        if let Some(current_ref) = &self.current_ref_after {
            output.push_str(&format!("- Checkout after recovery: `{current_ref}`\n"));
        }
        if !self.messages.is_empty() {
            output.push_str("\nMessages:\n");
            for message in &self.messages {
                output.push_str(&format!("- {message}\n"));
            }
        }
        if !self.final_status.is_empty() {
            output.push_str("\nRemaining git status:\n\n```text\n");
            output.push_str(&self.final_status.join("\n"));
            output.push_str("\n```\n");
        }
        if !self.recovery_commands.is_empty() {
            output.push_str("\nManual recovery commands:\n\n```bash\n");
            for command in &self.recovery_commands {
                output.push_str(command);
                output.push('\n');
            }
            output.push_str("```\n");
        }
    }

    pub(super) fn render_summary_lines(&self) -> Vec<String> {
        let mut lines = vec![format!("Recovery status: {}", self.status.as_str())];
        for message in self.messages.iter().take(2) {
            lines.push(format!("Recovery detail: {message}"));
        }
        if !self.final_status.is_empty() {
            lines.push(format!(
                "Remaining git status: {}",
                self.final_status.join("; ")
            ));
        }
        if let Some(command) = self.recovery_commands.first() {
            lines.push(format!("Recovery command: {command}"));
        }
        lines
    }

    pub(super) fn compact_summary_fragment(&self) -> Option<String> {
        let mut parts = vec![self.status.as_str().to_string()];
        if let Some(message) = self.messages.first() {
            parts.push(message.clone());
        }
        if !self.final_status.is_empty() {
            parts.push(format!("remaining={}", self.final_status.join("; ")));
        }
        (!parts.is_empty()).then(|| parts.join(" | "))
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) enum PlanRecoveryStatus {
    Restored,
    NotNeeded,
    ManualRequired,
}

impl PlanRecoveryStatus {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Restored => "restored",
            Self::NotNeeded => "not-needed",
            Self::ManualRequired => "manual-required",
        }
    }
}

fn current_checkout_ref(project_root: &Path) -> Result<CheckoutRef> {
    match run_git(
        project_root,
        &["symbolic-ref", "--quiet", "--short", "HEAD"],
    ) {
        Ok(branch) if !branch.trim().is_empty() => {
            Ok(CheckoutRef::Branch(branch.trim().to_string()))
        }
        Ok(_) | Err(_) => {
            let head = run_git(project_root, &["rev-parse", "--verify", "HEAD"])?;
            Ok(CheckoutRef::Detached(head.trim().to_string()))
        }
    }
}

fn git_status_lines(project_root: &Path) -> Result<Vec<String>> {
    let status = run_git(
        project_root,
        &["status", "--porcelain=v1", "--untracked-files=all"],
    )?;
    Ok(status
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(ToOwned::to_owned)
        .collect())
}

fn git_recovery_status_lines(project_root: &Path) -> Result<Vec<String>> {
    Ok(git_status_lines(project_root)?
        .into_iter()
        .filter(|line| !is_csa_plan_state_status(line))
        .collect())
}

fn is_csa_plan_state_status(line: &str) -> bool {
    porcelain_path(line).is_some_and(|path| path.starts_with(CSA_PLAN_STATE_PREFIX))
}

fn only_weave_lock_dirty(status_lines: &[String]) -> bool {
    !status_lines.is_empty()
        && status_lines
            .iter()
            .all(|line| porcelain_path(line).is_some_and(|path| path == WEAVE_LOCK))
}

fn porcelain_path(line: &str) -> Option<&str> {
    line.get(3..).map(str::trim).map(|path| {
        path.strip_prefix('"')
            .and_then(|path| path.strip_suffix('"'))
            .unwrap_or(path)
    })
}

fn restore_checkout(project_root: &Path, initial_ref: &CheckoutRef) -> Result<()> {
    match initial_ref {
        CheckoutRef::Branch(branch) => run_git_status(project_root, &["switch", branch]),
        CheckoutRef::Detached(head) => {
            run_git_status(project_root, &["checkout", "--detach", head])
        }
    }
}

fn recovery_commands(initial_ref: Option<&str>) -> Vec<String> {
    let mut commands = Vec::new();
    if let Some(initial_ref) = initial_ref {
        commands.push(format!(
            "git switch {}",
            super::super::shell_escape_for_command(initial_ref)
        ));
    }
    commands.push("git status --short --branch".to_string());
    commands
}

fn run_git(project_root: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(args)
        .output()
        .with_context(|| format!("failed to run git {}", args.join(" ")))?;
    if !output.status.success() {
        anyhow::bail!(
            "git {} failed with status {}: {}",
            args.join(" "),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .trim_end()
        .to_string())
}

fn run_git_status(project_root: &Path, args: &[&str]) -> Result<()> {
    run_git(project_root, args).map(|_| ())
}
