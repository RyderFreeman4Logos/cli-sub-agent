//! Plan-run journaling, resume-context loading, and repository fingerprinting.
//!
//! Split out of `plan_cmd` to keep that module within the per-file token
//! budget. These primitives persist `csa plan run` progress to a JSON journal
//! under `.csa/state/plan/`, decide whether an interrupted run may resume, and
//! capture a lightweight git fingerprint so a resume is only attempted when the
//! repository state still matches. Symbols are re-exported from `plan_cmd`
//! (`crate::plan_cmd`) so existing callers, the daemon dispatch, and the
//! in-module test submodules keep their original paths.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use weave::compiler::ExecutionPlan;

pub(crate) const PLAN_JOURNAL_SCHEMA_VERSION: u8 = 1;
pub(crate) const PLAN_PIPELINE_SOURCE_DIRECT: &str = "direct-plan-run";
pub(crate) const PLAN_PIPELINE_SOURCE_CLI_ALIAS: &str = "cli-alias";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PlanRunPipelineSource {
    DirectPlanRun,
    CliAlias,
}

impl PlanRunPipelineSource {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::DirectPlanRun => PLAN_PIPELINE_SOURCE_DIRECT,
            Self::CliAlias => PLAN_PIPELINE_SOURCE_CLI_ALIAS,
        }
    }

    pub(crate) fn from_str(value: &str) -> Option<Self> {
        match value {
            PLAN_PIPELINE_SOURCE_DIRECT => Some(Self::DirectPlanRun),
            PLAN_PIPELINE_SOURCE_CLI_ALIAS => Some(Self::CliAlias),
            _ => None,
        }
    }
}

pub(crate) fn default_plan_pipeline_source() -> String {
    PLAN_PIPELINE_SOURCE_DIRECT.to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PlanRunJournal {
    pub(crate) schema_version: u8,
    pub(crate) workflow_name: String,
    pub(crate) workflow_path: String,
    #[serde(default = "default_plan_pipeline_source")]
    pub(crate) pipeline_source: String,
    pub(crate) status: String,
    pub(crate) vars: HashMap<String, String>,
    pub(crate) completed_steps: Vec<usize>,
    pub(crate) last_error: Option<String>,
    #[serde(default)]
    pub(crate) repo_head: Option<String>,
    #[serde(default)]
    pub(crate) repo_dirty: Option<bool>,
}

impl PlanRunJournal {
    pub(crate) fn new(
        workflow_name: &str,
        workflow_path: &Path,
        vars: HashMap<String, String>,
    ) -> Self {
        Self {
            schema_version: PLAN_JOURNAL_SCHEMA_VERSION,
            workflow_name: workflow_name.to_string(),
            workflow_path: normalize_path(workflow_path),
            pipeline_source: default_plan_pipeline_source(),
            status: "running".to_string(),
            vars,
            completed_steps: Vec::new(),
            last_error: None,
            repo_head: None,
            repo_dirty: None,
        }
    }
}

pub(crate) struct PlanResumeContext {
    pub(crate) initial_vars: HashMap<String, String>,
    pub(crate) completed_steps: HashSet<usize>,
    pub(crate) pipeline_source: Option<String>,
    pub(crate) resumed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RepoFingerprint {
    pub(crate) head: Option<String>,
    pub(crate) dirty: Option<bool>,
}

pub(crate) fn normalize_path(path: &Path) -> String {
    path.canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .to_string()
}

pub(crate) fn safe_plan_name(plan_name: &str) -> String {
    let mut normalized: String = plan_name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    while normalized.contains("__") {
        normalized = normalized.replace("__", "_");
    }
    normalized.trim_matches('_').to_string()
}

pub(crate) fn plan_journal_path(project_root: &Path, plan_name: &str) -> PathBuf {
    let safe_name = safe_plan_name(plan_name);
    project_root
        .join(".csa")
        .join("state")
        .join("plan")
        .join(format!("{safe_name}.journal.json"))
}

pub(crate) fn persist_plan_journal(path: &Path, journal: &PlanRunJournal) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| {
            format!(
                "Failed to create plan state directory: {}",
                parent.display()
            )
        })?;
    }
    let encoded = serde_json::to_vec_pretty(journal).context("Failed to encode plan journal")?;
    std::fs::write(path, encoded)
        .with_context(|| format!("Failed to write plan journal: {}", path.display()))?;
    Ok(())
}

pub(crate) fn detect_repo_fingerprint(project_root: &Path) -> RepoFingerprint {
    let head = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["rev-parse", "--verify", "HEAD"])
        .output()
        .ok()
        .and_then(|out| {
            if out.status.success() {
                let value = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if value.is_empty() { None } else { Some(value) }
            } else {
                None
            }
        });

    let dirty = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["status", "--porcelain", "--untracked-files=normal"])
        .output()
        .ok()
        .and_then(|out| {
            if out.status.success() {
                Some(!String::from_utf8_lossy(&out.stdout).trim().is_empty())
            } else {
                None
            }
        });

    RepoFingerprint { head, dirty }
}

pub(crate) fn apply_repo_fingerprint(journal: &mut PlanRunJournal, fingerprint: &RepoFingerprint) {
    journal.repo_head = fingerprint.head.clone();
    journal.repo_dirty = fingerprint.dirty;
}

pub(crate) fn load_plan_resume_context(
    plan: &ExecutionPlan,
    workflow_path: &Path,
    journal_path: &Path,
    cli_vars: &HashMap<String, String>,
    repo_fingerprint: &RepoFingerprint,
    explicit_resume: bool,
) -> Result<PlanResumeContext> {
    let mut initial_vars = cli_vars.clone();
    if !journal_path.exists() {
        return Ok(PlanResumeContext {
            initial_vars,
            completed_steps: HashSet::new(),
            pipeline_source: None,
            resumed: false,
        });
    }

    let bytes = std::fs::read(journal_path)
        .with_context(|| format!("Failed to read plan journal: {}", journal_path.display()))?;
    let journal: PlanRunJournal = serde_json::from_slice(&bytes)
        .with_context(|| format!("Failed to parse plan journal: {}", journal_path.display()))?;

    if journal.schema_version != PLAN_JOURNAL_SCHEMA_VERSION {
        warn!(
            path = %journal_path.display(),
            found = journal.schema_version,
            expected = PLAN_JOURNAL_SCHEMA_VERSION,
            "Ignoring plan journal with unsupported schema version"
        );
        return Ok(PlanResumeContext {
            initial_vars,
            completed_steps: HashSet::new(),
            pipeline_source: None,
            resumed: false,
        });
    }

    let same_workflow = journal.workflow_name == plan.name
        && journal.workflow_path == normalize_path(workflow_path);
    let status_prevents_resume = matches!(
        journal.status.as_str(),
        "completed" | "awaiting-user" | "manual-handoff"
    );
    if !same_workflow
        || status_prevents_resume && !(explicit_resume && journal.status == "manual-handoff")
    {
        return Ok(PlanResumeContext {
            initial_vars,
            completed_steps: HashSet::new(),
            pipeline_source: None,
            resumed: false,
        });
    }

    if !explicit_resume {
        let fingerprint_matches = match (
            journal.repo_head.as_ref(),
            journal.repo_dirty,
            repo_fingerprint.head.as_ref(),
            repo_fingerprint.dirty,
        ) {
            (Some(saved_head), Some(saved_dirty), Some(current_head), Some(current_dirty)) => {
                saved_head == current_head && saved_dirty == current_dirty
            }
            _ => false,
        };
        if !fingerprint_matches {
            warn!(
                path = %journal_path.display(),
                "Ignoring plan journal because repository state changed (or fingerprint unavailable)"
            );
            return Ok(PlanResumeContext {
                initial_vars,
                completed_steps: HashSet::new(),
                pipeline_source: None,
                resumed: false,
            });
        }
    } else {
        info!(
            path = %journal_path.display(),
            "Explicit --resume: bypassing repository fingerprint check"
        );
    }

    let pipeline_source = journal.pipeline_source.clone();
    for (key, value) in journal.vars {
        initial_vars.insert(key, value);
    }
    // CLI-provided vars remain authoritative for declared variables.
    for (key, value) in cli_vars {
        initial_vars.insert(key.clone(), value.clone());
    }

    Ok(PlanResumeContext {
        initial_vars,
        completed_steps: journal.completed_steps.into_iter().collect(),
        pipeline_source: Some(pipeline_source),
        resumed: true,
    })
}
