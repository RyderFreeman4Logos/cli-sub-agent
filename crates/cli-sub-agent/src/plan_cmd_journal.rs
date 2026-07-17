//! Plan-run journaling, resume-context loading, and repository fingerprinting.
//!
//! Split out of `plan_cmd` to keep that module within the per-file token
//! budget. These primitives persist `csa plan run` progress to a JSON journal
//! under `.csa/state/plan/`, decide whether an explicit resume may continue,
//! and capture a lightweight git fingerprint for audit. Symbols are re-exported
//! from `plan_cmd` (`crate::plan_cmd`) so existing callers, the daemon
//! dispatch, and the in-module test submodules keep their original paths.

use std::collections::{HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use weave::compiler::ExecutionPlan;

use crate::run_resource_overrides::RunResourceOverrides;

pub(crate) const PLAN_JOURNAL_SCHEMA_VERSION: u8 = 2;
pub(crate) const PLAN_PIPELINE_SOURCE_DIRECT: &str = "direct-plan-run";
pub(crate) const PLAN_PIPELINE_SOURCE_CLI_ALIAS: &str = "cli-alias";
const ACTIVE_PLAN_JOURNAL_WARNING: &str = "dev2merge: journal is actively in use by another plan run; use --resume to continue or wait for it to complete";

static ACTIVE_PLAN_JOURNAL_LOCKS: OnceLock<Mutex<HashMap<PathBuf, PlanJournalFileLock>>> =
    OnceLock::new();

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
    #[serde(default = "RunResourceOverrides::absent")]
    pub(crate) resource_overrides: RunResourceOverrides,
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
            resource_overrides: RunResourceOverrides::absent(),
        }
    }
}

pub(crate) struct PlanResumeContext {
    pub(crate) initial_vars: HashMap<String, String>,
    pub(crate) completed_steps: HashSet<usize>,
    pub(crate) pipeline_source: Option<String>,
    pub(crate) resource_overrides: RunResourceOverrides,
    pub(crate) resumed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RepoFingerprint {
    pub(crate) head: Option<String>,
    pub(crate) dirty: Option<bool>,
}

struct PlanJournalFileLock {
    file: File,
}

impl Drop for PlanJournalFileLock {
    fn drop(&mut self) {
        // SAFETY: `self.file` owns a valid fd for the locked journal file.
        unsafe {
            libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

fn active_plan_journal_locks() -> &'static Mutex<HashMap<PathBuf, PlanJournalFileLock>> {
    ACTIVE_PLAN_JOURNAL_LOCKS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn ensure_active_plan_journal_lock(path: &Path) -> Result<()> {
    let lock_path = path.to_path_buf();
    let mut locks = active_plan_journal_locks()
        .lock()
        .map_err(|_| anyhow::anyhow!("plan journal lock registry is poisoned"))?;
    if locks.contains_key(&lock_path) {
        return Ok(());
    }

    let lock = try_acquire_plan_journal_file_lock(path, true)?
        .ok_or_else(|| anyhow::anyhow!(ACTIVE_PLAN_JOURNAL_WARNING))?;
    locks.insert(lock_path, lock);
    Ok(())
}

fn release_active_plan_journal_lock(path: &Path) -> Result<()> {
    let mut locks = active_plan_journal_locks()
        .lock()
        .map_err(|_| anyhow::anyhow!("plan journal lock registry is poisoned"))?;
    locks.remove(path);
    Ok(())
}

fn try_acquire_plan_journal_file_lock(
    path: &Path,
    create: bool,
) -> Result<Option<PlanJournalFileLock>> {
    let file = OpenOptions::new()
        .create(create)
        .read(true)
        .write(true)
        .truncate(false)
        .open(path)
        .with_context(|| format!("Failed to open plan journal lock: {}", path.display()))?;

    if try_flock_plan_journal_file(&file)? {
        return Ok(Some(PlanJournalFileLock { file }));
    }

    Ok(None)
}

fn try_flock_plan_journal_file(file: &File) -> Result<bool> {
    // SAFETY: `file` owns a valid fd and `LOCK_EX | LOCK_NB` is a standard
    // non-blocking advisory exclusive lock request.
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc == 0 {
        return Ok(true);
    }

    let err = std::io::Error::last_os_error();
    if matches!(
        err.raw_os_error(),
        Some(code) if code == libc::EWOULDBLOCK || code == libc::EAGAIN
    ) {
        return Ok(false);
    }

    Err(err.into())
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
    if journal.status == "running" {
        ensure_active_plan_journal_lock(path)?;
    }
    let encoded = serde_json::to_vec_pretty(journal).context("Failed to encode plan journal")?;
    std::fs::write(path, encoded)
        .with_context(|| format!("Failed to write plan journal: {}", path.display()))?;
    if journal.status != "running" {
        release_active_plan_journal_lock(path)?;
    }
    Ok(())
}

pub(crate) fn complete_pending_manual_step(
    plan: &ExecutionPlan,
    workflow_path: &Path,
    journal_path: &Path,
    step_id: usize,
) -> Result<()> {
    let _completion_lock = match try_acquire_plan_journal_file_lock(journal_path, false)? {
        Some(lock) => lock,
        None => {
            warn!(
                path = %journal_path.display(),
                ACTIVE_PLAN_JOURNAL_WARNING
            );
            bail!(ACTIVE_PLAN_JOURNAL_WARNING);
        }
    };
    let bytes = std::fs::read(journal_path)
        .with_context(|| format!("Failed to read plan journal: {}", journal_path.display()))?;
    let mut journal: PlanRunJournal = serde_json::from_slice(&bytes)
        .with_context(|| format!("Failed to parse plan journal: {}", journal_path.display()))?;

    if journal.schema_version != PLAN_JOURNAL_SCHEMA_VERSION {
        bail!(
            "Plan journal has unsupported schema version {} (expected {})",
            journal.schema_version,
            PLAN_JOURNAL_SCHEMA_VERSION
        );
    }
    let same_workflow = journal.workflow_name == plan.name
        && journal.workflow_path == normalize_path(workflow_path);
    if !same_workflow {
        bail!(
            "Plan journal {} does not match workflow '{}'",
            journal_path.display(),
            plan.name
        );
    }
    if journal.status != "manual-handoff" {
        bail!(
            "Cannot complete manual step {step_id}: journal status is '{}' (expected manual-handoff)",
            journal.status
        );
    }

    let completed_steps: HashSet<usize> = journal.completed_steps.iter().copied().collect();
    let pending_step = plan
        .steps
        .iter()
        .find(|step| !completed_steps.contains(&step.id))
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Cannot complete manual step {step_id}: journal has no pending workflow step"
            )
        })?;
    if pending_step.id != step_id {
        bail!(
            "Cannot complete manual step {step_id}: pending step is {} ('{}')",
            pending_step.id,
            pending_step.title
        );
    }
    let is_manual = pending_step
        .tool
        .as_deref()
        .is_some_and(|tool| tool.trim().eq_ignore_ascii_case("manual"));
    if !is_manual {
        bail!(
            "Cannot complete step {step_id}: pending step '{}' uses tool {:?}, not manual",
            pending_step.title,
            pending_step.tool
        );
    }

    journal.completed_steps.push(step_id);
    journal.completed_steps.sort_unstable();
    journal.completed_steps.dedup();
    journal.status = "manual-completed".to_string();
    journal.last_error = None;
    persist_plan_journal(journal_path, &journal)
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
    explicit_resume: bool,
) -> Result<PlanResumeContext> {
    let mut initial_vars = cli_vars.clone();
    if !journal_path.exists() {
        return Ok(PlanResumeContext {
            initial_vars,
            completed_steps: HashSet::new(),
            pipeline_source: None,
            resource_overrides: RunResourceOverrides::absent(),
            resumed: false,
        });
    }
    let _fresh_journal_lock = if explicit_resume {
        None
    } else {
        match try_acquire_plan_journal_file_lock(journal_path, false)? {
            Some(lock) => Some(lock),
            None => {
                warn!(
                    path = %journal_path.display(),
                    ACTIVE_PLAN_JOURNAL_WARNING
                );
                bail!(ACTIVE_PLAN_JOURNAL_WARNING);
            }
        }
    };

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
            resource_overrides: RunResourceOverrides::absent(),
            resumed: false,
        });
    }

    let same_workflow = journal.workflow_name == plan.name
        && journal.workflow_path == normalize_path(workflow_path);
    if !same_workflow {
        return Ok(PlanResumeContext {
            initial_vars,
            completed_steps: HashSet::new(),
            pipeline_source: None,
            resource_overrides: RunResourceOverrides::absent(),
            resumed: false,
        });
    }

    if !explicit_resume {
        warn!(
            path = %journal_path.display(),
            "dev2merge: clearing stale journal from previous run (use --resume to continue)"
        );
        std::fs::remove_file(journal_path).with_context(|| {
            format!(
                "Failed to clear stale plan journal: {}",
                journal_path.display()
            )
        })?;
        return Ok(PlanResumeContext {
            initial_vars,
            completed_steps: HashSet::new(),
            pipeline_source: None,
            resource_overrides: RunResourceOverrides::absent(),
            resumed: false,
        });
    }

    let status_prevents_resume = matches!(journal.status.as_str(), "completed" | "awaiting-user");
    if status_prevents_resume {
        return Ok(PlanResumeContext {
            initial_vars,
            completed_steps: HashSet::new(),
            pipeline_source: None,
            resource_overrides: RunResourceOverrides::absent(),
            resumed: false,
        });
    }
    info!(
        path = %journal_path.display(),
        "Explicit --resume: bypassing repository fingerprint check"
    );

    let pipeline_source = journal.pipeline_source.clone();
    let resource_overrides = journal.resource_overrides;
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
        resource_overrides,
        resumed: true,
    })
}
