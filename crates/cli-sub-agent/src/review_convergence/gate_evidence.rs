//! Host-authoritative final-gate execution and immutable evidence publication.

#![cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the production driver cannot synthesize cancellation-only outcomes used by isolated port tests"
    )
)]

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use csa_session::convergence::{
    ArtifactEvidenceRef, CsaSessionId, GATE_EVIDENCE_SCHEMA_ID, SessionRelativeArtifactPath,
    Sha256Digest, WorkspaceLeaseIdentity,
};
use serde::{Deserialize, Serialize};
use ulid::Ulid;

use super::gate_authority::{
    FinalGatePlan, GateCommandAuthority, GateCredentialPolicy, GateNetworkPolicy,
};

const GATE_ARTIFACT_SCHEMA_VERSION: u32 = 2;
const GATE_ARTIFACT_FILE_PREFIX: &str = "final-gate-v2-";
const MAX_GATE_ARTIFACT_BYTES: usize = 160 * 1024;
const MAX_GATE_OUTPUT_BYTES: usize = 4 * 1024;
const MAX_GATE_ARTIFACTS: usize = 32;

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum GateArtifactWriteFault {
    BeforeLink,
    AfterLink,
    BeforeDirectorySync,
    AfterDirectorySync,
}

/// An invocation whose argv, environment, and process isolation are controlled by the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GateInvocation {
    command: GateCommandAuthority,
    cwd: PathBuf,
    env: Vec<(String, String)>,
    independent_process_group: bool,
}

impl GateInvocation {
    fn from_plan(plan: &FinalGatePlan, command: &GateCommandAuthority) -> Self {
        Self {
            command: command.clone(),
            cwd: plan.lease().workspace_root().to_path_buf(),
            env: plan
                .minimal_env()
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
            independent_process_group: true,
        }
    }

    pub(crate) fn command(&self) -> &GateCommandAuthority {
        &self.command
    }

    pub(crate) fn cwd(&self) -> &Path {
        &self.cwd
    }

    pub(crate) fn env(&self) -> &[(String, String)] {
        &self.env
    }

    pub(crate) fn independent_process_group(&self) -> bool {
        self.independent_process_group
    }
}

/// Host-observed process completion. Only `Exited(0)` with a reaped group can pass a gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GateProcessTermination {
    Exited(i32),
    TimedOut,
    Cancelled,
    ChildSurvivor,
    ReapFailed,
}

/// Raw process output supplied by a driver. The port immediately bounds and redacts it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GateProcessOutcome {
    termination: GateProcessTermination,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

impl GateProcessOutcome {
    pub(crate) fn new(
        termination: GateProcessTermination,
        stdout: impl Into<Vec<u8>>,
        stderr: impl Into<Vec<u8>>,
    ) -> Self {
        Self {
            termination,
            stdout: stdout.into(),
            stderr: stderr.into(),
        }
    }
}

/// Side-effect boundary for direct-argv process execution.
///
/// Implementations must clear inherited environment, enforce the declared network and credential
/// policies, create an independent process group, and kill then reap that group on cancellation
/// or timeout. They must report any uncertain cleanup as `ReapFailed` or `ChildSurvivor`.
pub(crate) trait FinalGateDriver {
    fn run(&mut self, invocation: &GateInvocation) -> Result<GateProcessOutcome>;
}

/// Current owned lease boundary. A failure means the gate result cannot be trusted.
pub(crate) trait FinalGateLease {
    fn identity(&self) -> &WorkspaceLeaseIdentity;
    fn validate_current(&self) -> Result<()>;
}

impl<C: super::clean_room::WorkspaceCleanup> FinalGateLease
    for super::clean_room::DetachedWorkspaceLease<C>
{
    fn identity(&self) -> &WorkspaceLeaseIdentity {
        self.identity()
    }

    fn validate_current(&self) -> Result<()> {
        self.validate_current()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct GateOutputSummary {
    observed_bytes: u64,
    truncated: bool,
    text: String,
}

impl GateOutputSummary {
    fn from_bytes(bytes: &[u8]) -> Self {
        let observed_bytes = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        let mut truncated = bytes.len() > MAX_GATE_OUTPUT_BYTES;
        let retained = if truncated {
            &bytes[bytes.len() - MAX_GATE_OUTPUT_BYTES..]
        } else {
            bytes
        };
        let mut text = redact_secret_like_text(&escape_control_characters(
            &String::from_utf8_lossy(retained),
        ));
        if text.len() > MAX_GATE_OUTPUT_BYTES {
            text = truncate_utf8_tail(&text, MAX_GATE_OUTPUT_BYTES);
            truncated = true;
        }
        if truncated {
            let marker = format!("[csa: output truncated after {observed_bytes} bytes]\n");
            let body_limit = MAX_GATE_OUTPUT_BYTES.saturating_sub(marker.len());
            text = format!("{marker}{}", truncate_utf8_tail(&text, body_limit));
        }
        Self {
            observed_bytes,
            truncated,
            text,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct GateArtifactCommand {
    order: u32,
    command_id: String,
    program: String,
    argv: Vec<String>,
    network: GateNetworkPolicy,
    credentials: GateCredentialPolicy,
    timeout_seconds: u64,
    exit_code: i32,
    stdout: GateOutputSummary,
    stderr: GateOutputSummary,
}

impl GateArtifactCommand {
    fn from_outcome(
        order: u32,
        command: &GateCommandAuthority,
        outcome: GateProcessOutcome,
    ) -> Result<Self> {
        let GateProcessOutcome {
            termination,
            stdout,
            stderr,
        } = outcome;
        let exit_code = match termination {
            GateProcessTermination::Exited(0) => 0,
            GateProcessTermination::Exited(code) => bail!(
                "required final gate '{}' exited with status {code}",
                command.command_id()
            ),
            GateProcessTermination::TimedOut => {
                bail!("required final gate '{}' timed out", command.command_id())
            }
            GateProcessTermination::Cancelled => {
                bail!(
                    "required final gate '{}' was cancelled",
                    command.command_id()
                )
            }
            GateProcessTermination::ChildSurvivor => bail!(
                "required final gate '{}' left a child process survivor",
                command.command_id()
            ),
            GateProcessTermination::ReapFailed => bail!(
                "required final gate '{}' could not be reaped",
                command.command_id()
            ),
        };
        Ok(Self {
            order,
            command_id: command.command_id().to_string(),
            program: command.program().to_string(),
            argv: command.argv().to_vec(),
            network: command.network(),
            credentials: command.credentials(),
            timeout_seconds: command.timeout().as_secs(),
            exit_code,
            stdout: GateOutputSummary::from_bytes(&stdout),
            stderr: GateOutputSummary::from_bytes(&stderr),
        })
    }
}

/// The host artifact deliberately has no artifact reference or digest field, avoiding a
/// self-referential digest. The reference is computed only after bytes are durable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct GateArtifactEnvelope {
    schema_version: u32,
    schema: String,
    campaign_id: csa_session::convergence::CampaignId,
    epoch: csa_session::convergence::EpochRecord,
    policy_digest: Sha256Digest,
    final_gate_authority_digest: Sha256Digest,
    authority_version: String,
    commands: Vec<GateArtifactCommand>,
}

impl GateArtifactEnvelope {
    fn new(plan: &FinalGatePlan, commands: Vec<GateArtifactCommand>) -> Result<Self> {
        if commands.len() != plan.commands().len() {
            bail!("final-gate artifact is missing a required command result");
        }
        for (order, (actual, expected)) in commands.iter().zip(plan.commands()).enumerate() {
            if actual.order != u32::try_from(order).context("final-gate command order overflow")?
                || actual.command_id != expected.command_id()
                || actual.program != expected.program()
                || actual.argv != expected.argv()
                || actual.network != expected.network()
                || actual.credentials != expected.credentials()
                || actual.timeout_seconds != expected.timeout().as_secs()
                || actual.exit_code != 0
            {
                bail!("final-gate artifact command order or authority binding changed");
            }
        }
        Ok(Self {
            schema_version: GATE_ARTIFACT_SCHEMA_VERSION,
            schema: GATE_EVIDENCE_SCHEMA_ID.to_string(),
            campaign_id: plan.lease().campaign_id().clone(),
            epoch: plan.lease().epoch().clone(),
            policy_digest: plan.policy_digest().clone(),
            final_gate_authority_digest: plan.final_gate_authority_digest().clone(),
            authority_version: plan.authority_version().to_string(),
            commands,
        })
    }

    fn validate_plan(&self, plan: &FinalGatePlan) -> Result<()> {
        if self.schema_version != GATE_ARTIFACT_SCHEMA_VERSION
            || self.schema != GATE_EVIDENCE_SCHEMA_ID
            || self.campaign_id != *plan.lease().campaign_id()
            || self.epoch != *plan.lease().epoch()
            || self.policy_digest != *plan.policy_digest()
            || self.final_gate_authority_digest != *plan.final_gate_authority_digest()
            || self.authority_version != plan.authority_version()
        {
            bail!("final-gate artifact does not match its authority-bound plan");
        }
        Self::new(plan, self.commands.clone()).map(|_| ())
    }
}

/// A successful readback of immutable final-gate evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FinalGateEvidence {
    artifact: ArtifactEvidenceRef,
    commands: Vec<String>,
}

impl FinalGateEvidence {
    pub(crate) fn artifact(&self) -> &ArtifactEvidenceRef {
        &self.artifact
    }

    pub(crate) fn commands(&self) -> &[String] {
        &self.commands
    }
}

/// Host-owned content-addressed output directory for final-gate evidence.
#[derive(Debug, Clone)]
pub(crate) struct HostGateArtifactStore {
    directory: PathBuf,
    session_id: CsaSessionId,
    relative_directory: SessionRelativeArtifactPath,
}

impl HostGateArtifactStore {
    pub(crate) fn new(
        directory: &Path,
        session_id: CsaSessionId,
        relative_directory: SessionRelativeArtifactPath,
    ) -> Result<Self> {
        let metadata = fs::symlink_metadata(directory).with_context(|| {
            format!("inspect host final-gate directory {}", directory.display())
        })?;
        if !directory.is_absolute() || !metadata.is_dir() || metadata.file_type().is_symlink() {
            bail!("host final-gate artifact directory must be an absolute direct directory");
        }
        let canonical = fs::canonicalize(directory).with_context(|| {
            format!(
                "canonicalize host final-gate directory {}",
                directory.display()
            )
        })?;
        if canonical != directory {
            bail!("host final-gate artifact directory must not resolve through a symlink");
        }
        Ok(Self {
            directory: canonical,
            session_id,
            relative_directory,
        })
    }

    fn publish(
        &self,
        plan: &FinalGatePlan,
        commands: Vec<GateArtifactCommand>,
    ) -> Result<ArtifactEvidenceRef> {
        let envelope = GateArtifactEnvelope::new(plan, commands)?;
        let bytes = serde_json::to_vec(&envelope).context("serialize final-gate artifact")?;
        if bytes.len() > MAX_GATE_ARTIFACT_BYTES {
            bail!("final-gate artifact exceeds its byte quota");
        }
        let digest = Sha256Digest::compute(&bytes);
        let file_name = artifact_file_name(&digest)?;
        let destination = self.directory.join(&file_name);
        self.enforce_retention(&destination)?;
        publish_bytes_once(&self.directory, &destination, &bytes)?;
        Ok(ArtifactEvidenceRef::new(
            self.session_id.clone(),
            SessionRelativeArtifactPath::new(&format!(
                "{}/{}",
                self.relative_directory.as_str(),
                file_name
            ))?,
            digest,
        ))
    }

    pub(crate) fn readback(
        &self,
        plan: &FinalGatePlan,
        artifact: &ArtifactEvidenceRef,
    ) -> Result<FinalGateEvidence> {
        if artifact.csa_session_id() != &self.session_id {
            bail!("final-gate artifact session identity does not match its host store");
        }
        let prefix = format!("{}/", self.relative_directory.as_str());
        let name = artifact
            .path()
            .as_str()
            .strip_prefix(&prefix)
            .context("final-gate artifact path is outside its host directory")?;
        if Path::new(name).components().count() != 1 || !name.starts_with(GATE_ARTIFACT_FILE_PREFIX)
        {
            bail!("final-gate artifact path is invalid");
        }
        let bytes = read_private_bounded(&self.directory.join(name))?;
        if Sha256Digest::compute(&bytes) != *artifact.digest() {
            bail!("final-gate artifact digest mismatch");
        }
        let envelope: GateArtifactEnvelope =
            serde_json::from_slice(&bytes).context("parse final-gate artifact")?;
        envelope.validate_plan(plan)?;
        Ok(FinalGateEvidence {
            artifact: artifact.clone(),
            commands: envelope
                .commands
                .into_iter()
                .map(|command| command.command_id)
                .collect(),
        })
    }

    fn enforce_retention(&self, destination: &Path) -> Result<()> {
        if destination.exists() {
            return Ok(());
        }
        let retained = fs::read_dir(&self.directory)
            .with_context(|| format!("list final-gate artifacts in {}", self.directory.display()))?
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(GATE_ARTIFACT_FILE_PREFIX)
            })
            .count();
        if retained >= MAX_GATE_ARTIFACTS {
            bail!("final-gate artifact retention quota is exhausted");
        }
        Ok(())
    }
}

/// Executes one authority-bound plan serially under a verified owned workspace lease.
pub(crate) struct HostFinalGatePort<D> {
    pub(super) driver: D,
    artifacts: HostGateArtifactStore,
}

impl<D> HostFinalGatePort<D> {
    pub(crate) fn new(driver: D, artifacts: HostGateArtifactStore) -> Self {
        Self { driver, artifacts }
    }

    pub(crate) fn readback(
        &self,
        plan: &FinalGatePlan,
        artifact: &ArtifactEvidenceRef,
    ) -> Result<FinalGateEvidence> {
        self.artifacts.readback(plan, artifact)
    }
}

impl<D: FinalGateDriver> HostFinalGatePort<D> {
    pub(crate) fn run<L: FinalGateLease>(
        &mut self,
        plan: &FinalGatePlan,
        lease: &L,
    ) -> Result<FinalGateEvidence> {
        validate_lease(plan, lease)?;
        let mut commands = Vec::with_capacity(plan.commands().len());
        for (order, command) in plan.commands().iter().enumerate() {
            validate_lease(plan, lease)?;
            let outcome = self
                .driver
                .run(&GateInvocation::from_plan(plan, command))
                .with_context(|| {
                    format!("execute required final gate '{}'", command.command_id())
                })?;
            commands.push(GateArtifactCommand::from_outcome(
                u32::try_from(order).context("final-gate command order overflow")?,
                command,
                outcome,
            )?);
        }
        validate_lease(plan, lease)?;
        let artifact = self.artifacts.publish(plan, commands)?;
        validate_lease(plan, lease)?;
        let evidence = self.artifacts.readback(plan, &artifact)?;
        validate_lease(plan, lease)?;
        Ok(evidence)
    }
}

fn validate_lease<L: FinalGateLease>(plan: &FinalGatePlan, lease: &L) -> Result<()> {
    if lease.identity() != plan.lease() {
        bail!("final-gate lease identity does not match the authority-bound plan");
    }
    lease
        .validate_current()
        .context("final-gate epoch lease drift")
}

fn artifact_file_name(digest: &Sha256Digest) -> Result<String> {
    let suffix = digest
        .as_str()
        .strip_prefix("sha256:")
        .context("final-gate artifact digest lacks its sha256 prefix")?;
    Ok(format!("{GATE_ARTIFACT_FILE_PREFIX}{suffix}.json"))
}

fn publish_bytes_once(directory: &Path, destination: &Path, bytes: &[u8]) -> Result<()> {
    publish_bytes_once_impl(
        directory,
        destination,
        bytes,
        #[cfg(test)]
        None,
    )
}

#[cfg(test)]
pub(super) fn publish_bytes_once_with_fault(
    directory: &Path,
    destination: &Path,
    bytes: &[u8],
    fault: GateArtifactWriteFault,
) -> Result<()> {
    publish_bytes_once_impl(directory, destination, bytes, Some(fault))
}

fn publish_bytes_once_impl(
    directory: &Path,
    destination: &Path,
    bytes: &[u8],
    #[cfg(test)] fault: Option<GateArtifactWriteFault>,
) -> Result<()> {
    let temporary = directory.join(format!(".final-gate-{}.tmp", Ulid::new()));
    let result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&temporary)
            .with_context(|| format!("create final-gate artifact {}", temporary.display()))?;
        file.write_all(bytes).context("write final-gate artifact")?;
        file.sync_all().context("sync final-gate artifact")?;
        #[cfg(test)]
        if fault == Some(GateArtifactWriteFault::BeforeLink) {
            bail!("fault injection before final-gate artifact link");
        }
        match fs::hard_link(&temporary, destination) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                if read_private_bounded(destination)? != bytes {
                    bail!("final-gate artifact name exists with different bytes");
                }
            }
            Err(error) => return Err(error).context("publish final-gate artifact"),
        }
        #[cfg(test)]
        if fault == Some(GateArtifactWriteFault::AfterLink) {
            bail!("fault injection after final-gate artifact link");
        }
        let directory_file = File::open(directory).context("open final-gate artifact directory")?;
        #[cfg(test)]
        if fault == Some(GateArtifactWriteFault::BeforeDirectorySync) {
            bail!("fault injection before final-gate artifact directory sync");
        }
        directory_file
            .sync_all()
            .context("sync final-gate artifact directory")?;
        #[cfg(test)]
        if fault == Some(GateArtifactWriteFault::AfterDirectorySync) {
            bail!("fault injection after final-gate artifact directory sync");
        }
        Ok(())
    })();
    let cleanup = fs::remove_file(&temporary);
    result?;
    match cleanup {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).context("remove final-gate artifact temporary file"),
    }
}

fn read_private_bounded(path: &Path) -> Result<Vec<u8>> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .with_context(|| format!("open final-gate artifact {}", path.display()))?;
    let mode = file.metadata()?.permissions().mode();
    if mode & 0o077 != 0 {
        bail!("final-gate artifact is not private (0600)");
    }
    let mut bytes = Vec::new();
    file.take((MAX_GATE_ARTIFACT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_GATE_ARTIFACT_BYTES {
        bail!("final-gate artifact exceeds its byte quota");
    }
    Ok(bytes)
}

fn escape_control_characters(value: &str) -> String {
    value.chars().fold(String::new(), |mut safe, character| {
        match character {
            '\n' | '\t' => safe.push(character),
            character if character.is_control() => {
                safe.push_str(&format!("\\u{{{:04X}}}", character as u32))
            }
            character => safe.push(character),
        }
        safe
    })
}

fn redact_secret_like_text(value: &str) -> String {
    let mut redact_next = false;
    value
        .split_whitespace()
        .map(|token| {
            if redact_next {
                redact_next = false;
                return "[REDACTED]".to_string();
            }
            let lower = token.to_ascii_lowercase();
            if token.starts_with("sk-") && token.len() > 10
                || token.starts_with("AKIA") && token.len() >= 16
            {
                "[REDACTED]".to_string()
            } else if let Some((key, _)) = token.split_once('=') {
                if matches!(
                    key.to_ascii_lowercase().as_str(),
                    "api_key" | "apikey" | "token" | "secret" | "password"
                ) {
                    format!("{key}=[REDACTED]")
                } else {
                    token.to_string()
                }
            } else if lower == "bearer" {
                redact_next = true;
                "Bearer".to_string()
            } else {
                token.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn truncate_utf8_tail(value: &str, maximum: usize) -> String {
    if value.len() <= maximum {
        return value.to_string();
    }
    let mut start = value.len().saturating_sub(maximum);
    while !value.is_char_boundary(start) {
        start += 1;
    }
    value[start..].to_string()
}
