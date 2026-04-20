use crate::pipeline_post_exec::PostExecContext;
use csa_session::MetaSessionState;
use csa_session::SessionArtifact;
use csa_session::SessionResult;

pub(crate) fn maybe_record_repo_write_audit(
    ctx: &PostExecContext<'_>,
    session: &MetaSessionState,
    session_result: &mut SessionResult,
) -> anyhow::Result<()> {
    if !should_audit_repo_tracked_writes(ctx.task_type, ctx.readonly_project_root, ctx.prompt) {
        return Ok(());
    }

    let Some((pre_session_head, pre_session_porcelain)) = pre_session_audit_baseline(session)
    else {
        tracing::warn!(
            session = %session.meta_session_id,
            session_dir = %ctx.session_dir.display(),
            "repo-write audit skipped because pre-session HEAD snapshot is unavailable"
        );
        return Ok(());
    };

    let audit = csa_session::compute_repo_write_audit(
        ctx.project_root,
        pre_session_head,
        pre_session_porcelain,
    )?;
    if audit.is_empty() {
        return Ok(());
    }

    tracing::warn!(
        session_dir = %ctx.session_dir.display(),
        added = ?audit.added,
        modified = ?audit.modified,
        deleted = ?audit.deleted,
        renamed = ?audit.renamed,
        "repo-tracked files mutated during read-only/recon-style session"
    );
    apply_repo_write_audit_to_result(session_result, &audit);
    if let Some(artifact_path) =
        csa_session::write_audit_warning_artifact(&ctx.session_dir, &audit)?
        && let Ok(rel_path) = artifact_path.strip_prefix(&ctx.session_dir)
    {
        session_result.artifacts.push(SessionArtifact::new(
            rel_path.to_string_lossy().into_owned(),
        ));
    }

    Ok(())
}

fn pre_session_audit_baseline(session: &MetaSessionState) -> Option<(&str, Option<&str>)> {
    session
        .git_head_at_creation
        .as_deref()
        .map(|head| (head, session.pre_session_porcelain.as_deref()))
}

fn apply_repo_write_audit_to_result(
    session_result: &mut SessionResult,
    audit: &csa_session::RepoWriteAudit,
) {
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
mod tests {
    use super::{
        apply_repo_write_audit_to_result, pre_session_audit_baseline,
        should_audit_repo_tracked_writes,
    };
    use csa_session::{MetaSessionState, RepoWriteAudit, SessionResult};
    use std::path::PathBuf;

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

    #[test]
    fn should_audit_repo_tracked_writes_for_plan_task_type() {
        assert!(should_audit_repo_tracked_writes(
            Some("plan"),
            false,
            "Analyze the workflow and summarize where files are written"
        ));
    }

    #[test]
    fn should_audit_repo_tracked_writes_for_plan_step_task_type() {
        assert!(should_audit_repo_tracked_writes(
            Some("plan-step"),
            false,
            "Read-only: inspect the task step and summarize the result"
        ));
    }

    #[test]
    fn should_not_audit_repo_tracked_writes_for_review_or_debate() {
        assert!(!should_audit_repo_tracked_writes(
            Some("review"),
            true,
            "Analyze the diff and summarize findings"
        ));
        assert!(!should_audit_repo_tracked_writes(
            Some("debate"),
            true,
            "Analyze the proposal and summarize tradeoffs"
        ));
    }

    #[test]
    fn should_not_audit_repo_tracked_writes_for_unknown_task_type() {
        assert!(!should_audit_repo_tracked_writes(
            None,
            true,
            "Analyze the module and summarize the control flow"
        ));
    }

    #[test]
    fn apply_repo_write_audit_to_result_populates_manager_sidecar_sections() {
        let mut session_result = SessionResult {
            status: "success".to_string(),
            exit_code: 0,
            summary: "ok".to_string(),
            tool: "codex".to_string(),
            started_at: chrono::Utc::now(),
            completed_at: chrono::Utc::now(),
            events_count: 0,
            artifacts: vec![],
            peak_memory_mb: None,
            manager_fields: Default::default(),
        };
        let audit = RepoWriteAudit {
            added: vec![PathBuf::from("new.txt")],
            modified: vec![PathBuf::from("tracked.txt")],
            deleted: vec![PathBuf::from("old.txt")],
            renamed: vec![(PathBuf::from("src/a.rs"), PathBuf::from("src/b.rs"))],
        };

        apply_repo_write_audit_to_result(&mut session_result, &audit);

        let repo_write_audit = session_result
            .manager_fields
            .artifacts
            .as_ref()
            .and_then(|value| value.get("repo_write_audit"))
            .expect("repo write audit sidecar");
        assert_eq!(
            repo_write_audit
                .get("added")
                .and_then(toml::Value::as_array),
            Some(&vec![toml::Value::String("new.txt".to_string())])
        );
        assert_eq!(
            repo_write_audit
                .get("modified")
                .and_then(toml::Value::as_array),
            Some(&vec![toml::Value::String("tracked.txt".to_string())])
        );
        assert_eq!(
            repo_write_audit
                .get("deleted")
                .and_then(toml::Value::as_array),
            Some(&vec![toml::Value::String("old.txt".to_string())])
        );
        let renamed = repo_write_audit
            .get("renamed")
            .and_then(toml::Value::as_array)
            .expect("renamed entries");
        assert_eq!(renamed.len(), 1);
        assert_eq!(
            renamed[0].get("from"),
            Some(&toml::Value::String("src/a.rs".to_string()))
        );
        assert_eq!(
            renamed[0].get("to"),
            Some(&toml::Value::String("src/b.rs".to_string()))
        );
    }

    #[test]
    fn pre_session_audit_baseline_returns_none_for_legacy_sessions() {
        let session = MetaSessionState {
            meta_session_id: "01TESTLEGACYAUDIT0000000000".to_string(),
            description: None,
            project_path: "/tmp/project".to_string(),
            branch: None,
            created_at: chrono::Utc::now(),
            last_accessed: chrono::Utc::now(),
            genealogy: Default::default(),
            tools: Default::default(),
            context_status: Default::default(),
            total_token_usage: None,
            phase: Default::default(),
            task_context: Default::default(),
            turn_count: 0,
            token_budget: None,
            sandbox_info: None,
            termination_reason: None,
            is_seed_candidate: false,
            git_head_at_creation: None,
            pre_session_porcelain: None,
            last_return_packet: None,
            change_id: None,
            spec_id: None,
            vcs_identity: None,
            identity_version: 2,
            fork_call_timestamps: Vec::new(),
        };

        assert_eq!(pre_session_audit_baseline(&session), None);
    }

    #[test]
    fn pre_session_audit_baseline_returns_head_and_optional_porcelain() {
        let session = MetaSessionState {
            meta_session_id: "01TESTAUDITBASELINE000000000".to_string(),
            description: None,
            project_path: "/tmp/project".to_string(),
            branch: None,
            created_at: chrono::Utc::now(),
            last_accessed: chrono::Utc::now(),
            genealogy: Default::default(),
            tools: Default::default(),
            context_status: Default::default(),
            total_token_usage: None,
            phase: Default::default(),
            task_context: Default::default(),
            turn_count: 0,
            token_budget: None,
            sandbox_info: None,
            termination_reason: None,
            is_seed_candidate: false,
            git_head_at_creation: Some("abc123".to_string()),
            pre_session_porcelain: Some(" M tracked.txt\0".to_string()),
            last_return_packet: None,
            change_id: None,
            spec_id: None,
            vcs_identity: None,
            identity_version: 2,
            fork_call_timestamps: Vec::new(),
        };

        assert_eq!(
            pre_session_audit_baseline(&session),
            Some(("abc123", Some(" M tracked.txt\0")))
        );
    }
}
