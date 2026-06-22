use std::fs;
use std::path::{Path, PathBuf};

use csa_session::MetaSessionState;
use serde::{Deserialize, Serialize};
use tracing::warn;

pub(crate) const FIX_FINDING_RECOVERY_SIDECAR_PATH: &str = "output/fix_finding_recovery.json";

const FIX_FINDING_TASK_TYPE: &str = "review_fix_finding";
const RECOVERY_OUTCOME: &str = "failed_closed_missing_result";
const MAX_RECORDED_PATHS: usize = 8;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct FixFindingRecoverySidecar {
    pub(crate) schema_version: u8,
    pub(crate) kind: String,
    pub(crate) session_id: String,
    pub(crate) outcome: String,
    pub(crate) side_effects: FixFindingSideEffects,
    pub(crate) allow_required_push_next_step: bool,
    pub(crate) requires_fresh_exact_head_review: bool,
    pub(crate) recovery_actions: Vec<String>,
    pub(crate) git_inspection_commands: Vec<String>,
    pub(crate) guidance: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct FixFindingSideEffects {
    pub(crate) status: String,
    pub(crate) added: BoundedPathGroup,
    pub(crate) modified: BoundedPathGroup,
    pub(crate) deleted: BoundedPathGroup,
    pub(crate) renamed: BoundedRenameGroup,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub(crate) struct BoundedPathGroup {
    pub(crate) paths: Vec<String>,
    pub(crate) truncated: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub(crate) struct BoundedRenameGroup {
    pub(crate) paths: Vec<BoundedRename>,
    pub(crate) truncated: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct BoundedRename {
    pub(crate) from: String,
    pub(crate) to: String,
}

impl FixFindingSideEffects {
    fn none_detected() -> Self {
        Self {
            status: "none_detected".to_string(),
            added: BoundedPathGroup::default(),
            modified: BoundedPathGroup::default(),
            deleted: BoundedPathGroup::default(),
            renamed: BoundedRenameGroup::default(),
        }
    }

    fn unknown() -> Self {
        Self {
            status: "unknown".to_string(),
            added: BoundedPathGroup::default(),
            modified: BoundedPathGroup::default(),
            deleted: BoundedPathGroup::default(),
            renamed: BoundedRenameGroup::default(),
        }
    }

    pub(crate) fn diagnostic_label(&self) -> String {
        let mut parts = Vec::new();
        push_path_group_label(&mut parts, "added", &self.added);
        push_path_group_label(&mut parts, "modified", &self.modified);
        push_path_group_label(&mut parts, "deleted", &self.deleted);
        if !self.renamed.paths.is_empty() {
            let renames = self
                .renamed
                .paths
                .iter()
                .map(|path| format!("{}->{}", path.from, path.to))
                .collect::<Vec<_>>()
                .join(",");
            let suffix = truncated_suffix(self.renamed.truncated);
            parts.push(format!("renamed=[{renames}]{suffix}"));
        }
        if parts.is_empty() {
            format!("repo_side_effects={}", self.status)
        } else {
            format!("repo_side_effects={} {}", self.status, parts.join(" "))
        }
    }
}

pub(crate) fn is_fix_finding_session(session: &MetaSessionState) -> bool {
    session.task_context.task_type.as_deref() == Some(FIX_FINDING_TASK_TYPE)
}

pub(crate) fn recovery_sidecar_path(session_dir: &Path) -> PathBuf {
    session_dir.join(FIX_FINDING_RECOVERY_SIDECAR_PATH)
}

pub(crate) fn build_recovery_sidecar(
    project_root: &Path,
    session: &MetaSessionState,
) -> Option<FixFindingRecoverySidecar> {
    if !is_fix_finding_session(session) {
        return None;
    }
    Some(FixFindingRecoverySidecar {
        schema_version: 1,
        kind: "fix_finding_failed_closed_recovery".to_string(),
        session_id: session.meta_session_id.clone(),
        outcome: RECOVERY_OUTCOME.to_string(),
        side_effects: side_effects(project_root, session),
        allow_required_push_next_step: false,
        requires_fresh_exact_head_review: true,
        recovery_actions: vec![
            "inspect_git_metadata".to_string(),
            "preserve_finish_or_discard_dirty_side_effects".to_string(),
            "create_hook_enabled_commit_if_appropriate".to_string(),
            "run_fresh_exact_head_review_before_push_or_pr".to_string(),
        ],
        git_inspection_commands: vec![
            "git status --short".to_string(),
            "git diff".to_string(),
            "git diff --staged".to_string(),
            "git log --oneline -5".to_string(),
        ],
        guidance: "Inspect git metadata, preserve and finish or discard dirty/staged side effects, create a hook-enabled commit if appropriate, and run a fresh exact-head review before push/PR.".to_string(),
    })
}

pub(crate) fn side_effect_diagnostic(project_root: &Path, session: &MetaSessionState) -> String {
    side_effects(project_root, session).diagnostic_label()
}

pub(crate) fn read_recovery_sidecar(session_dir: &Path) -> Option<FixFindingRecoverySidecar> {
    let path = recovery_sidecar_path(session_dir);
    let contents = fs::read_to_string(&path).ok()?;
    match serde_json::from_str::<FixFindingRecoverySidecar>(&contents) {
        Ok(sidecar) => Some(sidecar),
        Err(err) => {
            warn!(
                sidecar_path = %path.display(),
                error = %err,
                "Ignoring malformed fix-finding recovery sidecar"
            );
            None
        }
    }
}

pub(crate) fn suppresses_required_push_next_step(session_dir: &Path) -> bool {
    let path = recovery_sidecar_path(session_dir);
    if !path.is_file() {
        return false;
    }
    read_recovery_sidecar(session_dir)
        .map(|sidecar| !sidecar.allow_required_push_next_step)
        .unwrap_or(true)
}

pub(crate) fn wait_summary_lines(session_dir: &Path) -> Vec<String> {
    let Some(sidecar) = read_recovery_sidecar(session_dir) else {
        return Vec::new();
    };
    vec![
        format!(
            "Fix-finding recovery: {} (required push next-step suppressed)",
            sidecar.outcome
        ),
        format!("Side effects: {}", sidecar.side_effects.diagnostic_label()),
        "Recovery action: inspect git status/diff/staged; preserve/finish or discard dirty side effects; create a hook-enabled commit if appropriate; run a fresh exact-head review before push/PR".to_string(),
    ]
}

fn side_effects(project_root: &Path, session: &MetaSessionState) -> FixFindingSideEffects {
    let Some(pre_head) = session.git_head_at_creation.as_deref() else {
        return FixFindingSideEffects::unknown();
    };
    let Ok(audit) = csa_session::compute_repo_write_audit(
        project_root,
        pre_head,
        session.pre_session_porcelain.as_deref(),
    ) else {
        return FixFindingSideEffects::unknown();
    };
    if audit.is_empty() {
        return FixFindingSideEffects::none_detected();
    }
    FixFindingSideEffects {
        status: "dirty_or_committed_tracked_changes".to_string(),
        added: bounded_paths(&audit.added),
        modified: bounded_paths(&audit.modified),
        deleted: bounded_paths(&audit.deleted),
        renamed: bounded_renames(&audit.renamed),
    }
}

fn bounded_paths(paths: &[PathBuf]) -> BoundedPathGroup {
    BoundedPathGroup {
        paths: paths
            .iter()
            .take(MAX_RECORDED_PATHS)
            .map(|path| path.display().to_string())
            .collect(),
        truncated: paths.len().saturating_sub(MAX_RECORDED_PATHS),
    }
}

fn bounded_renames(paths: &[(PathBuf, PathBuf)]) -> BoundedRenameGroup {
    BoundedRenameGroup {
        paths: paths
            .iter()
            .take(MAX_RECORDED_PATHS)
            .map(|(from, to)| BoundedRename {
                from: from.display().to_string(),
                to: to.display().to_string(),
            })
            .collect(),
        truncated: paths.len().saturating_sub(MAX_RECORDED_PATHS),
    }
}

fn push_path_group_label(parts: &mut Vec<String>, label: &str, group: &BoundedPathGroup) {
    if group.paths.is_empty() {
        return;
    }
    let suffix = truncated_suffix(group.truncated);
    parts.push(format!("{label}=[{}]{suffix}", group.paths.join(",")));
}

fn truncated_suffix(truncated: usize) -> String {
    if truncated > 0 {
        format!("(+{truncated} more)")
    } else {
        String::new()
    }
}
