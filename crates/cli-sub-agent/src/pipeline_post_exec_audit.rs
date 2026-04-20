use crate::pipeline_post_exec::{PostExecContext, PreExecutionSnapshot};
use csa_session::MetaSessionState;
use csa_session::SessionArtifact;
use csa_session::SessionResult;

pub(crate) fn maybe_record_repo_write_audit(
    ctx: &PostExecContext<'_>,
    session: &MetaSessionState,
    session_result: &mut SessionResult,
) {
    if !should_audit_repo_tracked_writes(ctx.task_type, ctx.readonly_project_root, ctx.prompt) {
        return;
    }

    let Some((pre_session_head, pre_session_porcelain)) =
        pre_execution_audit_baseline(ctx, session)
    else {
        tracing::warn!(
            session = %session.meta_session_id,
            session_dir = %ctx.session_dir.display(),
            "repo-write audit skipped because no baseline snapshot is available"
        );
        return;
    };

    let audit = match csa_session::compute_repo_write_audit(
        ctx.project_root,
        pre_session_head,
        pre_session_porcelain,
    ) {
        Ok(audit) => audit,
        Err(error) => {
            tracing::warn!(
                session = %session.meta_session_id,
                session_dir = %ctx.session_dir.display(),
                error = ?error,
                "repo-write audit failed during compute; ignoring (audit is best-effort)"
            );
            return;
        }
    };
    if audit.is_empty() {
        return;
    }

    tracing::warn!(
        session_dir = %ctx.session_dir.display(),
        added = ?audit.added,
        modified = ?audit.modified,
        deleted = ?audit.deleted,
        renamed = ?audit.renamed,
        "repo-tracked files mutated during read-only/recon-style session"
    );
    if let Err(error) = apply_repo_write_audit_to_result(session_result, &audit) {
        tracing::warn!(
            session = %session.meta_session_id,
            session_dir = %ctx.session_dir.display(),
            error = ?error,
            added = ?audit.added,
            modified = ?audit.modified,
            deleted = ?audit.deleted,
            renamed = ?audit.renamed,
            "repo-write audit failed during result mutation; ignoring (audit is best-effort)"
        );
    }
    match csa_session::write_audit_warning_artifact(&ctx.session_dir, &audit) {
        Ok(Some(artifact_path)) => {
            if let Ok(rel_path) = artifact_path.strip_prefix(&ctx.session_dir) {
                session_result.artifacts.push(SessionArtifact::new(
                    rel_path.to_string_lossy().into_owned(),
                ));
            }
        }
        Ok(None) => {}
        Err(error) => {
            tracing::warn!(
                session = %session.meta_session_id,
                session_dir = %ctx.session_dir.display(),
                error = ?error,
                added = ?audit.added,
                modified = ?audit.modified,
                deleted = ?audit.deleted,
                renamed = ?audit.renamed,
                "repo-write audit warning artifact failed to persist; ignoring (audit is best-effort)"
            );
        }
    }
}

fn pre_execution_audit_baseline<'a>(
    ctx: &'a PostExecContext<'_>,
    session: &'a MetaSessionState,
) -> Option<(&'a str, Option<&'a str>)> {
    if let Some(PreExecutionSnapshot { head, porcelain }) = ctx.pre_exec_snapshot.as_ref() {
        return Some((head.as_str(), porcelain.as_deref()));
    }

    if let Some(head) = session.git_head_at_creation.as_deref() {
        tracing::warn!(
            session = %session.meta_session_id,
            session_dir = %ctx.session_dir.display(),
            "repo-write audit falling back to session-creation baseline because per-execution capture is unavailable"
        );
        return Some((head, session.pre_session_porcelain.as_deref()));
    }

    None
}

fn apply_repo_write_audit_to_result(
    session_result: &mut SessionResult,
    audit: &csa_session::RepoWriteAudit,
) -> anyhow::Result<()> {
    let renamed = audit
        .renamed
        .iter()
        .map(|(from, to)| {
            let mut rename = toml::map::Map::new();
            rename.insert(
                "from".to_string(),
                toml::Value::String(from.display().to_string()),
            );
            rename.insert(
                "to".to_string(),
                toml::Value::String(to.display().to_string()),
            );
            toml::Value::Table(rename)
        })
        .collect::<Vec<_>>();
    let mut repo_write_audit = toml::map::Map::new();
    repo_write_audit.insert("added".to_string(), string_array_value(&audit.added));
    repo_write_audit.insert("modified".to_string(), string_array_value(&audit.modified));
    repo_write_audit.insert("deleted".to_string(), string_array_value(&audit.deleted));
    repo_write_audit.insert("renamed".to_string(), toml::Value::Array(renamed));

    let mut artifacts_table = session_result
        .manager_fields
        .artifacts
        .as_ref()
        .and_then(toml::Value::as_table)
        .cloned()
        .unwrap_or_default();
    artifacts_table.insert(
        "repo_write_audit".to_string(),
        toml::Value::Table(repo_write_audit),
    );
    session_result.manager_fields.artifacts = Some(toml::Value::Table(artifacts_table));
    Ok(())
}

fn string_array_value(paths: &[std::path::PathBuf]) -> toml::Value {
    toml::Value::Array(
        paths
            .iter()
            .map(|path| toml::Value::String(path.display().to_string()))
            .collect(),
    )
}

pub(crate) fn should_audit_repo_tracked_writes(
    task_type: Option<&str>,
    readonly_project_root: bool,
    prompt: &str,
) -> bool {
    if !matches!(task_type, Some("run" | "plan" | "plan-step")) {
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
#[path = "pipeline_tests_post_exec_audit.rs"]
mod tests;
