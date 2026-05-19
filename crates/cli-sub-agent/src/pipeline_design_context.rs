//! First-turn context injection for CSA prompts.
//!
//! Handles two kinds of context injection on the first turn of a session:
//! 1. Project context (CLAUDE.md, AGENTS.md) — always injected if available
//! 2. Design context from TODO plan's `design.md` reference — injected when the
//!    current branch has a TODO plan with design sections (Key Decisions, Constraints,
//!    Threats, Codebase Structure, Existing Patterns, Threat Model, Debate Evidence)

use std::path::Path;

use tracing::{debug, info};

#[derive(Debug, Default)]
pub(crate) struct FirstTurnContext {
    pub project_context: Option<String>,
    pub plan_context: Option<String>,
    pub design_context: Option<String>,
}

/// Load project context and design context for the first turn of a session.
pub(crate) fn load_first_turn_context(
    session_project_path: &str,
    project_root: &Path,
    context_load_options: Option<&csa_executor::ContextLoadOptions>,
    plan_injection_enabled: bool,
) -> FirstTurnContext {
    // Project context (CLAUDE.md, AGENTS.md).
    let opts = context_load_options.cloned().unwrap_or_default();
    let files = csa_executor::load_project_context(Path::new(session_project_path), &opts);
    let project_context = if files.is_empty() {
        None
    } else {
        let ctx = csa_executor::format_context_for_prompt(&files);
        info!(
            files = files.len(),
            bytes = ctx.len(),
            "Injecting project context"
        );
        Some(ctx)
    };

    // Design context from TODO plan's design.md reference.
    let design_context = load_design_context(project_root).inspect(|dc| {
        info!(bytes = dc.len(), "Injecting design context into prompt");
    });

    let plan_context = plan_injection_enabled
        .then(|| super::plan_context::load_plan_context(project_root))
        .flatten()
        .inspect(|pc| {
            info!(bytes = pc.len(), "Injecting plan context into prompt");
        });

    FirstTurnContext {
        project_context,
        plan_context,
        design_context,
    }
}

/// Load design context from the current branch's TODO plan.
///
/// Returns `None` silently on any failure (no plan, no design.md, no sections).
fn load_design_context(project_root: &Path) -> Option<String> {
    let branch = detect_branch(project_root)?;
    debug!(branch = %branch, "Checking for design context");

    let manager = csa_todo::TodoManager::new(project_root).ok()?;
    let plans = manager.find_by_branch(&branch).ok()?;
    let plan = plans.first()?;
    debug!(plan = %plan.timestamp, "Found TODO plan for branch");

    let content = manager.read_reference(plan, "design.md", None).ok()?;
    debug!(bytes = content.len(), "Read design.md reference");

    let sections = csa_executor::extract_design_sections(&content, None)?;
    Some(csa_executor::format_design_context(&branch, &sections))
}

/// Auto-detect current branch via VCS abstraction (supports both git and jj).
///
/// Returns `None` on detached HEAD or error.
fn detect_branch(project_root: &Path) -> Option<String> {
    let backend = csa_session::vcs_backends::create_vcs_backend(project_root);
    backend.current_branch(project_root).ok().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_env_lock::{ScopedEnvVarRestore, TEST_ENV_LOCK};
    use csa_todo::{CriterionKind, CriterionStatus, SpecCriterion, SpecDocument, TodoManager};
    use std::{fs, path::Path, process::Command};
    use tempfile::tempdir;

    const TEST_BRANCH: &str = "feat/plan-context";

    fn run_git(project_root: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(project_root)
            .output()
            .expect("git should run");
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn init_git_branch(project_root: &Path, branch: &str) {
        fs::create_dir_all(project_root).expect("create project root");
        run_git(project_root, &["init", "-q"]);
        run_git(project_root, &["config", "user.email", "test@example.com"]);
        run_git(project_root, &["config", "user.name", "Test User"]);
        run_git(project_root, &["commit", "--allow-empty", "-m", "initial"]);
        run_git(project_root, &["checkout", "-q", "-b", branch]);
    }

    fn criterion() -> SpecCriterion {
        SpecCriterion {
            kind: CriterionKind::Scenario,
            id: "scenario-plan-context".to_string(),
            description: "Active plan context is injected on the first turn.".to_string(),
            status: CriterionStatus::Pending,
        }
    }

    #[test]
    fn first_turn_context_respects_plan_injection_flag() {
        let _env_lock = TEST_ENV_LOCK.clone().blocking_lock_owned();
        let temp = tempdir().expect("tempdir");
        let state_home = temp.path().join("state");
        fs::create_dir_all(&state_home).expect("create state dir");
        let _home = ScopedEnvVarRestore::set("HOME", temp.path());
        let _state = ScopedEnvVarRestore::set("XDG_STATE_HOME", &state_home);

        let project_root = temp.path().join("project");
        init_git_branch(&project_root, TEST_BRANCH);
        let manager = TodoManager::new(&project_root).expect("todo manager");
        let plan = manager
            .create("Goal drift prevention", Some(TEST_BRANCH))
            .expect("plan");
        manager
            .save_spec(
                &plan.timestamp,
                &SpecDocument {
                    plan_ulid: plan.timestamp.clone(),
                    summary: "Keep first-turn prompts aligned with the active plan.".to_string(),
                    criteria: vec![criterion()],
                    ..SpecDocument::default()
                },
            )
            .expect("save spec");

        let enabled_context = load_first_turn_context(
            project_root.to_str().expect("utf-8 project path"),
            &project_root,
            None,
            true,
        );
        let plan_context = enabled_context
            .plan_context
            .expect("enabled plan injection should load plan context");
        assert!(plan_context.contains("<plan-context>"));
        assert!(plan_context.contains("Plan: Goal drift prevention"));
        assert!(plan_context.contains("Branch: feat/plan-context"));
        assert!(plan_context.contains("scenario-plan-context"));

        let disabled_context = load_first_turn_context(
            project_root.to_str().expect("utf-8 project path"),
            &project_root,
            None,
            false,
        );
        assert!(disabled_context.plan_context.is_none());
    }
}
