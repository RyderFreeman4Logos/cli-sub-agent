//! Clean-room workspace and provider-session ports.
//!
//! The production adapters in this module are intentionally driver-injected: this slice can
//! construct and validate detached exact-OID process specifications without invoking `git` or an
//! AI provider. The repository's Rust 1.88 MSRV cannot adopt ADK-Rust 1.0 (Rust 1.94 MSRV), so the
//! provider boundary continues to use CSA's existing catalog-admitted executor.

#![expect(
    dead_code,
    reason = "B5 Slice 3B1 exposes audited clean-room provider authority before orchestration dispatch"
)]

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use csa_session::convergence::{
    AdmittedModelIdentity, CommandAuthoritySnapshot, EpochRecord, Sha256Digest,
};

use crate::pipeline::{AdmittedExecutor, ParentSessionSource, SessionCreationMode};
use crate::startup_env::StartupSubtreeEnv;

#[allow(unused_imports)]
pub(crate) use super::production_clean_room_provider::ProductionCleanRoomProvider;

/// Exact, inspectable subprocess specification. Constructing this value has no side effects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommandSpec {
    program: String,
    args: Vec<String>,
    env: BTreeMap<String, String>,
}

impl CommandSpec {
    fn new(program: &str, args: Vec<String>, env: BTreeMap<String, String>) -> Result<Self> {
        validate_process_component("program", program)?;
        for argument in &args {
            validate_process_component("argument", argument)?;
        }
        for (key, value) in &env {
            validate_process_component("environment key", key)?;
            validate_process_component("environment value", value)?;
        }
        Ok(Self {
            program: program.to_string(),
            args,
            env,
        })
    }

    pub(crate) fn program(&self) -> &str {
        &self.program
    }

    pub(crate) fn args(&self) -> &[String] {
        &self.args
    }

    pub(crate) fn env(&self) -> &BTreeMap<String, String> {
        &self.env
    }
}

/// Side-effect-free create and cleanup specifications for one detached exact-OID workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DetachedWorkspacePlan {
    create: CommandSpec,
    cleanup: CommandSpec,
}

impl DetachedWorkspacePlan {
    fn exact_oid(source_repo: &Path, root: &Path, head_oid: &str) -> Result<Self> {
        let source = absolute_utf8_path("source repository", source_repo)?;
        let root = absolute_utf8_path("clean-room root", root)?;
        validate_process_component("frozen head object ID", head_oid)?;
        let env = BTreeMap::from([
            ("GIT_CONFIG_NOSYSTEM".to_string(), "1".to_string()),
            ("GIT_TERMINAL_PROMPT".to_string(), "0".to_string()),
        ]);
        Ok(Self {
            create: CommandSpec::new(
                "git",
                [
                    "-c",
                    "advice.detachedHead=false",
                    "-C",
                    source,
                    "worktree",
                    "add",
                    "--detach",
                    root,
                    head_oid,
                ]
                .into_iter()
                .map(str::to_string)
                .collect(),
                env.clone(),
            )?,
            cleanup: CommandSpec::new(
                "git",
                ["-C", source, "worktree", "remove", "--force", root]
                    .into_iter()
                    .map(str::to_string)
                    .collect(),
                env,
            )?,
        })
    }

    pub(crate) fn create(&self) -> &CommandSpec {
        &self.create
    }

    pub(crate) fn cleanup(&self) -> &CommandSpec {
        &self.cleanup
    }
}

/// Cleanup capability returned only after a workspace driver materializes a workspace.
pub(crate) trait WorkspaceCleanup {
    fn cleanup(&mut self, timeout: Duration) -> Result<()>;
}

/// Materialization receipt from an injected driver.
pub(crate) struct MaterializedWorkspace<C> {
    observed_head: String,
    cleanup: C,
}

impl<C> MaterializedWorkspace<C> {
    pub(crate) fn new(observed_head: String, cleanup: C) -> Self {
        Self {
            observed_head,
            cleanup,
        }
    }
}

/// Injected side-effect boundary. Tests use recording drivers; no shell is spawned by this module.
pub(crate) trait DetachedWorkspaceDriver {
    type Cleanup: WorkspaceCleanup;

    fn materialize(
        &mut self,
        plan: &DetachedWorkspacePlan,
    ) -> Result<MaterializedWorkspace<Self::Cleanup>>;
}

/// Immutable identity and boundaries of one clean-room checkout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CleanRoomWorkspace {
    root: PathBuf,
    bundle_path: PathBuf,
    epoch: EpochRecord,
}

impl CleanRoomWorkspace {
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn bundle_path(&self) -> &Path {
        &self.bundle_path
    }

    pub(crate) fn epoch(&self) -> &EpochRecord {
        &self.epoch
    }
}

/// Shared audit channel for failures that can only be observed from `Drop`.
#[derive(Debug, Clone, Default)]
pub(crate) struct CleanupFailureLedger {
    failures: Arc<Mutex<Vec<String>>>,
}

impl CleanupFailureLedger {
    fn record(&self, error: &anyhow::Error) {
        self.failures
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(format!("{error:#}"));
    }

    pub(crate) fn failures(&self) -> Vec<String> {
        self.failures
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

/// RAII lease that requests bounded cleanup on every normal, error, and unwind drop path.
pub(crate) struct CleanRoomWorkspaceGuard<C: WorkspaceCleanup> {
    workspace: CleanRoomWorkspace,
    cleanup: Option<C>,
    cleanup_timeout: Duration,
    failure_ledger: CleanupFailureLedger,
}

impl<C: WorkspaceCleanup> fmt::Debug for CleanRoomWorkspaceGuard<C> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CleanRoomWorkspaceGuard")
            .field("workspace", &self.workspace)
            .field("cleanup_armed", &self.cleanup.is_some())
            .field("cleanup_timeout", &self.cleanup_timeout)
            .finish_non_exhaustive()
    }
}

impl<C: WorkspaceCleanup> CleanRoomWorkspaceGuard<C> {
    pub(crate) fn workspace(&self) -> &CleanRoomWorkspace {
        &self.workspace
    }

    /// Perform cleanup now so callers can surface a cleanup failure synchronously.
    pub(crate) fn close(mut self) -> Result<()> {
        let Some(mut cleanup) = self.cleanup.take() else {
            return Ok(());
        };
        cleanup
            .cleanup(self.cleanup_timeout)
            .context("clean-room workspace cleanup failed")
    }
}

impl<C: WorkspaceCleanup> Drop for CleanRoomWorkspaceGuard<C> {
    fn drop(&mut self) {
        let Some(mut cleanup) = self.cleanup.take() else {
            return;
        };
        if let Err(error) = cleanup
            .cleanup(self.cleanup_timeout)
            .context("clean-room workspace cleanup failed during drop")
        {
            self.failure_ledger.record(&error);
        }
    }
}

pub(crate) trait CleanRoomWorkspaceFactory {
    type Cleanup: WorkspaceCleanup;

    fn create(
        &mut self,
        source_repo: &Path,
        root: &Path,
        bundle_path: &Path,
        epoch: EpochRecord,
    ) -> Result<CleanRoomWorkspaceGuard<Self::Cleanup>>;
}

/// Production plan adapter. The injected driver owns any eventual process execution.
pub(crate) struct ExactOidWorkspaceFactory<D> {
    driver: D,
    cleanup_timeout: Duration,
    failure_ledger: CleanupFailureLedger,
}

impl<D> ExactOidWorkspaceFactory<D> {
    pub(crate) fn new(
        driver: D,
        cleanup_timeout: Duration,
        failure_ledger: CleanupFailureLedger,
    ) -> Self {
        Self {
            driver,
            cleanup_timeout,
            failure_ledger,
        }
    }
}

impl<D: DetachedWorkspaceDriver> CleanRoomWorkspaceFactory for ExactOidWorkspaceFactory<D> {
    type Cleanup = D::Cleanup;

    fn create(
        &mut self,
        source_repo: &Path,
        root: &Path,
        bundle_path: &Path,
        epoch: EpochRecord,
    ) -> Result<CleanRoomWorkspaceGuard<Self::Cleanup>> {
        absolute_utf8_path("source repository", source_repo)?;
        absolute_utf8_path("clean-room root", root)?;
        absolute_utf8_path("provider evidence bundle", bundle_path)?;
        epoch
            .validate()
            .context("validate frozen clean-room epoch")?;
        let expected_head = epoch.head_oid().as_str().to_string();
        let plan = DetachedWorkspacePlan::exact_oid(source_repo, root, &expected_head)?;
        let materialized = self
            .driver
            .materialize(&plan)
            .context("materialize detached exact-OID clean-room workspace")?;
        let guard = CleanRoomWorkspaceGuard {
            workspace: CleanRoomWorkspace {
                root: root.to_path_buf(),
                bundle_path: bundle_path.to_path_buf(),
                epoch,
            },
            cleanup: Some(materialized.cleanup),
            cleanup_timeout: self.cleanup_timeout,
            failure_ledger: self.failure_ledger.clone(),
        };
        if materialized.observed_head != expected_head {
            let mismatch = anyhow!(
                "clean-room workspace did not materialize the exact frozen head: expected {expected_head}, observed {}",
                materialized.observed_head
            );
            return match guard.close() {
                Ok(()) => Err(mismatch),
                Err(cleanup_error) => Err(mismatch.context(format!(
                    "cleanup after exact-OID mismatch also failed: {cleanup_error:#}"
                ))),
            };
        }
        Ok(guard)
    }
}

/// Exact provider prompt bytes. This type deliberately exposes no transformation API.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ExactProviderPrompt(String);

impl ExactProviderPrompt {
    pub(crate) fn new(prompt: impl Into<String>) -> Self {
        Self(prompt.into())
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    pub(crate) fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

impl fmt::Debug for ExactProviderPrompt {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExactProviderPrompt")
            .field("byte_len", &self.0.len())
            .finish_non_exhaustive()
    }
}

/// Fully resolved provider invocation contract for a fresh read-only clean-room session.
#[derive(Debug, Clone)]
pub(crate) struct ProviderSessionRequest {
    cwd: PathBuf,
    selected_model: AdmittedModelIdentity,
    authority_digest: Sha256Digest,
    exact_prompt: ExactProviderPrompt,
    evidence_bundle: PathBuf,
    readonly_project_root: bool,
    extra_writable: Vec<PathBuf>,
    extra_readable: Vec<PathBuf>,
    parent_session_source: ParentSessionSource,
    session_creation_mode: SessionCreationMode,
    parent: Option<String>,
    resume_session: Option<String>,
    epoch: EpochRecord,
    startup_env: StartupSubtreeEnv,
}

impl ProviderSessionRequest {
    pub(crate) fn from_authority(
        workspace: &CleanRoomWorkspace,
        authority: &CommandAuthoritySnapshot,
        exact_prompt: ExactProviderPrompt,
    ) -> Result<Self> {
        let strongest = authority
            .ordered_admitted()
            .first()
            .context("clean-room provider authority has no admitted model")?;
        if strongest.reasoning() != "xhigh" {
            bail!(
                "clean-room review requires the strongest admitted model at xhigh reasoning; got {}/{}/{}/{}",
                strongest.tool(),
                strongest.provider(),
                strongest.model(),
                strongest.reasoning()
            );
        }
        Ok(Self {
            cwd: workspace.root.clone(),
            selected_model: strongest.clone(),
            authority_digest: authority.digest().clone(),
            exact_prompt,
            evidence_bundle: workspace.bundle_path.clone(),
            readonly_project_root: true,
            extra_writable: Vec::new(),
            extra_readable: vec![workspace.bundle_path.clone()],
            parent_session_source: ParentSessionSource::ExplicitOnly,
            session_creation_mode: SessionCreationMode::FreshChild,
            parent: None,
            resume_session: None,
            epoch: workspace.epoch.clone(),
            startup_env: StartupSubtreeEnv::from_values(HashMap::new()),
        })
    }

    pub(crate) fn startup_env(&self) -> &StartupSubtreeEnv {
        &self.startup_env
    }

    pub(crate) fn cwd(&self) -> &Path {
        &self.cwd
    }

    pub(crate) fn authority_digest(&self) -> &Sha256Digest {
        &self.authority_digest
    }

    pub(crate) fn exact_prompt(&self) -> &str {
        self.exact_prompt.as_str()
    }

    pub(crate) fn evidence_bundle(&self) -> &Path {
        &self.evidence_bundle
    }

    pub(crate) fn selected_model(&self) -> &AdmittedModelIdentity {
        &self.selected_model
    }

    pub(crate) fn readonly_project_root(&self) -> bool {
        self.readonly_project_root
    }

    pub(crate) fn extra_writable(&self) -> &[PathBuf] {
        &self.extra_writable
    }

    pub(crate) fn extra_readable(&self) -> &[PathBuf] {
        &self.extra_readable
    }

    pub(crate) fn parent_session_source(&self) -> ParentSessionSource {
        self.parent_session_source
    }

    pub(crate) fn session_creation_mode(&self) -> SessionCreationMode {
        self.session_creation_mode
    }

    pub(crate) fn parent(&self) -> Option<&str> {
        self.parent.as_deref()
    }

    pub(crate) fn resume_session(&self) -> Option<&str> {
        self.resume_session.as_deref()
    }

    pub(crate) fn epoch(&self) -> &EpochRecord {
        &self.epoch
    }
}

/// Provider output retained by the caller before it can become review evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProviderSessionOutcome {
    session_id: String,
    artifact: Vec<u8>,
    artifact_digest: Sha256Digest,
}

impl ProviderSessionOutcome {
    pub(crate) fn new(session_id: &str, artifact: &[u8]) -> Self {
        Self {
            session_id: session_id.to_string(),
            artifact: artifact.to_vec(),
            artifact_digest: Sha256Digest::compute(artifact),
        }
    }

    pub(crate) fn session_id(&self) -> &str {
        &self.session_id
    }

    pub(crate) fn artifact(&self) -> &[u8] {
        &self.artifact
    }

    pub(crate) fn artifact_digest(&self) -> &Sha256Digest {
        &self.artifact_digest
    }
}

pub(crate) type ProviderSessionFuture<'a> =
    Pin<Box<dyn Future<Output = Result<ProviderSessionOutcome>> + 'a>>;

pub(crate) trait ProviderSessionFactory {
    fn run<'a>(&'a mut self, request: &'a ProviderSessionRequest) -> ProviderSessionFuture<'a>;
}

/// Injected provider boundary that receives CSA's existing admitted executor.
pub(crate) trait ProviderSessionDriver {
    fn run<'a>(
        &'a mut self,
        admitted: &'a AdmittedExecutor,
        request: &'a ProviderSessionRequest,
    ) -> ProviderSessionFuture<'a>;
}

/// Production adapter that cannot bypass catalog admission or replace the selected identity.
pub(crate) struct AdmittedProviderSessionFactory<'a, D> {
    admitted: &'a AdmittedExecutor,
    driver: D,
}

impl<'a, D> AdmittedProviderSessionFactory<'a, D> {
    pub(crate) fn new(admitted: &'a AdmittedExecutor, driver: D) -> Self {
        Self { admitted, driver }
    }
}

impl<D: ProviderSessionDriver> ProviderSessionFactory for AdmittedProviderSessionFactory<'_, D> {
    fn run<'a>(&'a mut self, request: &'a ProviderSessionRequest) -> ProviderSessionFuture<'a> {
        Box::pin(async move {
            validate_admitted_identity(self.admitted, request.selected_model())?;
            self.driver.run(self.admitted, request).await
        })
    }
}

fn validate_admitted_identity(
    admitted: &AdmittedExecutor,
    expected: &AdmittedModelIdentity,
) -> Result<()> {
    let actual = admitted_identity(admitted)?;
    if &actual != expected {
        bail!("clean-room provider request differs from the catalog-admitted executor identity");
    }
    Ok(())
}

pub(super) fn admitted_identity(admitted: &AdmittedExecutor) -> Result<AdmittedModelIdentity> {
    let spec = admitted.resolved_model_spec();
    AdmittedModelIdentity::new(
        &spec.tool,
        &spec.provider,
        &spec.model,
        &reasoning_label(&spec.thinking_budget),
    )
}

fn reasoning_label(budget: &csa_executor::ThinkingBudget) -> String {
    match budget {
        csa_executor::ThinkingBudget::DefaultBudget => "default".to_string(),
        csa_executor::ThinkingBudget::Low => "low".to_string(),
        csa_executor::ThinkingBudget::Medium => "medium".to_string(),
        csa_executor::ThinkingBudget::High => "high".to_string(),
        csa_executor::ThinkingBudget::Xhigh => "xhigh".to_string(),
        csa_executor::ThinkingBudget::Max => "max".to_string(),
        csa_executor::ThinkingBudget::Custom(value) => value.to_string(),
    }
}

fn absolute_utf8_path<'a>(label: &str, path: &'a Path) -> Result<&'a str> {
    if !path.is_absolute() {
        bail!("{label} must be an absolute path: {}", path.display());
    }
    let value = path
        .to_str()
        .with_context(|| format!("{label} must be valid UTF-8: {}", path.display()))?;
    validate_process_component(label, value)?;
    Ok(value)
}

fn validate_process_component(label: &str, value: &str) -> Result<()> {
    if value.is_empty() {
        bail!("{label} must not be empty");
    }
    if value.contains('\0') {
        bail!("{label} must not contain NUL");
    }
    Ok(())
}
