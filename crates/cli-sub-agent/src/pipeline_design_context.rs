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
    pub design_context: Option<String>,
}

/// Load project context and design context for the first turn of a session.
pub(crate) fn load_first_turn_context(
    session_project_path: &str,
    project_root: &Path,
    context_load_options: Option<&csa_executor::ContextLoadOptions>,
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

    FirstTurnContext {
        project_context,
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
