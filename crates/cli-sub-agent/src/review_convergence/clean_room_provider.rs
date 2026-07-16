//! Provider-session contracts for clean-room completion execution.

#![cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "B5 Slice 3B1 exposes audited clean-room provider authority before orchestration dispatch"
    )
)]

use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;

use anyhow::{Context, Result, bail};
use csa_session::convergence::{
    AdmittedModelIdentity, CommandAuthoritySnapshot, EpochRecord, Sha256Digest,
};

use crate::pipeline::{AdmittedExecutor, ParentSessionSource, SessionCreationMode};
use crate::startup_env::StartupSubtreeEnv;

use super::clean_room::CleanRoomWorkspace;

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
            cwd: workspace.root().to_path_buf(),
            selected_model: strongest.clone(),
            authority_digest: authority.digest().clone(),
            exact_prompt,
            evidence_bundle: workspace.bundle_path().to_path_buf(),
            readonly_project_root: true,
            extra_writable: Vec::new(),
            extra_readable: vec![workspace.bundle_path().to_path_buf()],
            parent_session_source: ParentSessionSource::ExplicitOnly,
            session_creation_mode: SessionCreationMode::FreshChild,
            parent: None,
            resume_session: None,
            epoch: workspace.epoch().clone(),
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
    #[expect(
        dead_code,
        reason = "factory construction is retained for the completion-port wiring slice"
    )]
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
