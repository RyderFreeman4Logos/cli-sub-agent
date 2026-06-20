use std::path::{Path, PathBuf};

use csa_session::MetaSessionState;

use super::reconcile_diagnostics::synthetic_failure_diagnostics;

const FIX_FINDING_TASK_TYPE: &str = "review_fix_finding";

pub(super) fn missing_result_summary_prefix(
    project_root: &Path,
    session: &MetaSessionState,
    session_dir: &Path,
    trigger: &str,
    output_log_mtime: &str,
    liveness_reason: &str,
) -> String {
    let diagnostics = synthetic_failure_diagnostics(session_dir, session, liveness_reason);
    let default = || {
        format!(
            "synthetic failure by {trigger}: process dead, result.toml missing \
             (reconciliation_reason=true_missing_result, output_log_mtime={output_log_mtime})\
             {diagnostics}"
        )
    };
    if session.task_context.task_type.as_deref() != Some(FIX_FINDING_TASK_TYPE) {
        return default();
    }

    let side_effects = fix_finding_side_effect_diagnostic(project_root, session)
        .unwrap_or_else(|| "repo_side_effects=unknown".to_string());
    format!(
        "`csa review --fix-finding` session failed closed: process dead before writing \
         result.toml for fix session {} (output_log_mtime={output_log_mtime}). \
         The original failed review verdict is not a fix-session result. {side_effects}. \
         Recovery: inspect `git status --short`, decide whether the dirty/staged work \
         is a complete fix to commit or incomplete work to salvage/revert, then run a \
         fresh `csa review` session for the next review round. Diagnostics are from {}.\
         {diagnostics}",
        session.meta_session_id,
        session_dir.display()
    )
}

fn fix_finding_side_effect_diagnostic(
    project_root: &Path,
    session: &MetaSessionState,
) -> Option<String> {
    let pre_head = session.git_head_at_creation.as_deref()?;
    let audit = csa_session::compute_repo_write_audit(
        project_root,
        pre_head,
        session.pre_session_porcelain.as_deref(),
    )
    .ok()?;
    if audit.is_empty() {
        return Some("repo_side_effects=none_detected".to_string());
    }

    let mut parts = Vec::new();
    push_path_group(&mut parts, "added", &audit.added);
    push_path_group(&mut parts, "modified", &audit.modified);
    push_path_group(&mut parts, "deleted", &audit.deleted);
    if !audit.renamed.is_empty() {
        let renames = audit
            .renamed
            .iter()
            .take(8)
            .map(|(from, to)| format!("{}->{}", from.display(), to.display()))
            .collect::<Vec<_>>()
            .join(",");
        let truncated = audit.renamed.len().saturating_sub(8);
        let suffix = if truncated > 0 {
            format!("(+{truncated} more)")
        } else {
            String::new()
        };
        parts.push(format!("renamed=[{renames}]{suffix}"));
    }
    Some(format!(
        "repo_side_effects=dirty_or_committed_tracked_changes {}",
        parts.join(" ")
    ))
}

fn push_path_group(parts: &mut Vec<String>, label: &str, paths: &[PathBuf]) {
    if paths.is_empty() {
        return;
    }
    let rendered = paths
        .iter()
        .take(8)
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(",");
    let truncated = paths.len().saturating_sub(8);
    let suffix = if truncated > 0 {
        format!("(+{truncated} more)")
    } else {
        String::new()
    };
    parts.push(format!("{label}=[{rendered}]{suffix}"));
}
