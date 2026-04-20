use crate::pipeline_post_exec::PostExecContext;
use csa_session::SessionArtifact;
use csa_session::SessionResult;

pub(crate) fn maybe_record_repo_write_audit(
    ctx: &PostExecContext<'_>,
    session_result: &mut SessionResult,
) -> anyhow::Result<()> {
    if !should_audit_repo_tracked_writes(ctx.task_type, ctx.readonly_project_root, ctx.prompt) {
        return Ok(());
    }

    let session_start_time: std::time::SystemTime = ctx.execution_start_time.into();
    let mutated_paths =
        csa_session::audit_repo_tracked_writes(ctx.project_root, session_start_time)?;
    if mutated_paths.is_empty() {
        return Ok(());
    }

    tracing::warn!(
        session_dir = %ctx.session_dir.display(),
        mutated_paths = ?mutated_paths,
        "repo-tracked files mutated during read-only/recon-style session"
    );
    if let Some(artifact_path) =
        csa_session::write_audit_warning_artifact(&ctx.session_dir, &mutated_paths)?
        && let Ok(rel_path) = artifact_path.strip_prefix(&ctx.session_dir)
    {
        session_result.artifacts.push(SessionArtifact::new(
            rel_path.to_string_lossy().into_owned(),
        ));
    }

    Ok(())
}

pub(crate) fn should_audit_repo_tracked_writes(
    task_type: Option<&str>,
    readonly_project_root: bool,
    prompt: &str,
) -> bool {
    if !matches!(task_type, Some("run")) {
        return false;
    }
    if readonly_project_root {
        return true;
    }

    let prompt_lower = prompt.to_ascii_lowercase();
    let explicit_readonly = [
        "read-only",
        "readonly",
        "do not edit",
        "don't edit",
        "must not edit",
        "without editing",
        "do not modify",
        "don't modify",
    ];
    if explicit_readonly
        .iter()
        .any(|marker| prompt_lower.contains(marker))
    {
        return true;
    }

    let recon_markers = [
        "recon",
        "reconnaissance",
        "analyze",
        "analyse",
        "analysis",
        "summarize",
        "summary",
        "inspect",
        "investigate",
    ];
    let write_markers = [
        "implement",
        "edit",
        "fix",
        "modify",
        "update",
        "patch",
        "write code",
        "create file",
        "commit",
        "merge",
        "refactor",
    ];
    recon_markers
        .iter()
        .any(|marker| prompt_lower.contains(marker))
        && !write_markers
            .iter()
            .any(|marker| prompt_lower.contains(marker))
}

#[cfg(test)]
mod tests {
    use super::should_audit_repo_tracked_writes;

    #[test]
    fn should_audit_repo_tracked_writes_for_explicit_readonly_run() {
        assert!(should_audit_repo_tracked_writes(
            Some("run"),
            false,
            "Read-only: inspect src/main.rs and summarize what it does"
        ));
    }

    #[test]
    fn should_audit_repo_tracked_writes_for_recon_style_run() {
        assert!(should_audit_repo_tracked_writes(
            Some("run"),
            false,
            "Analyze the main module and summarize the control flow"
        ));
    }

    #[test]
    fn should_not_audit_repo_tracked_writes_for_mutating_run() {
        assert!(!should_audit_repo_tracked_writes(
            Some("run"),
            false,
            "Implement the fix in src/main.rs and update tests"
        ));
    }
}
