use std::path::Path;

use anyhow::Result;

pub(super) fn finalize_prompt_text(
    project_root: &Path,
    prompt_text: String,
    inline_context_from_review_session: Option<&str>,
) -> Result<String> {
    let prompt_with_review_context = crate::run_helpers::prepend_review_context_to_prompt(
        project_root,
        prompt_text,
        inline_context_from_review_session,
    )?;

    Ok(crate::run_helpers::prepend_atomic_commit_discipline_to_prompt(prompt_with_review_context))
}

pub(super) fn current_branch_name(project_root: &Path) -> String {
    csa_session::vcs_backends::create_vcs_backend(project_root)
        .current_branch(project_root)
        .ok()
        .flatten()
        .unwrap_or_else(|| "(unknown)".to_string())
}
