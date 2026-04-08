use crate::skill_resolver::ResolvedSkill;

/// Infer whether a prompt requires editing existing files.
///
/// Returns:
/// - `Some(true)` when the prompt clearly asks for implementation/editing.
/// - `Some(false)` when the prompt explicitly requests read-only execution.
/// - `None` when intent is ambiguous.
///
/// Prefer `resolve_task_edit_requirement()` when a resolved skill is available,
/// because skill-side `workspace_access` contracts override this heuristic.
pub(crate) fn infer_task_edit_requirement(prompt: &str) -> Option<bool> {
    let prompt_lower = prompt.to_lowercase();

    let explicit_read_only = [
        "read-only",
        "readonly",
        "do not edit",
        "don't edit",
        "must not edit",
        "without editing",
    ];
    if explicit_read_only
        .iter()
        .any(|marker| prompt_lower.contains(marker))
    {
        return Some(false);
    }

    if prompt_lower
        .split(|ch: char| !ch.is_alphanumeric())
        .any(|token| {
            matches!(
                token,
                "commit"
                    | "commits"
                    | "committed"
                    | "committing"
                    | "edit"
                    | "edits"
                    | "edited"
                    | "editing"
                    | "fix"
                    | "fixes"
                    | "fixed"
                    | "fixing"
                    | "merge"
                    | "merges"
                    | "merged"
                    | "merging"
            )
        })
    {
        return Some(true);
    }

    let edit_markers = [
        "implement",
        "refactor",
        "modify",
        "update",
        "patch",
        "write code",
        "create file",
        "rename",
    ];
    if edit_markers
        .iter()
        .any(|marker| prompt_lower.contains(marker))
    {
        return Some(true);
    }

    None
}

/// Resolve whether the current run should be treated as mutating.
///
/// Skill-side `workspace_access` contracts take precedence over prompt
/// heuristics so mutating skills cannot be misrouted onto read-only tools just
/// because the prompt wording is ambiguous.
pub(crate) fn resolve_task_edit_requirement(
    skill: Option<&ResolvedSkill>,
    prompt: &str,
) -> Option<bool> {
    skill
        .and_then(|resolved| resolved.agent_config())
        .and_then(|agent| {
            agent
                .workspace_access
                .map(|access| access.task_needs_edit())
        })
        .or_else(|| infer_task_edit_requirement(prompt))
}
