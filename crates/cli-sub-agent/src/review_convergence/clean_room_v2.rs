//! Host-authoritative clean-room v2 review artifacts.
//!
//! Provider JSON is parsed as bounded data only. The host supplies every artifact reference,
//! campaign/epoch binding, and model-evidence field before publishing a content-addressed record.

use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use csa_session::convergence::{
    ArtifactEvidenceRef, CampaignId, CsaSessionId, EpochRecord, ModelEvidence,
    SemanticFindingIdentity, SessionRelativeArtifactPath, Sha256Digest, StableFindingId,
};
use serde::{Deserialize, Serialize, de::Error as _};
use ulid::Ulid;

const PROVIDER_RESPONSE_MAX_BYTES: usize = 48 * 1024;
const REVIEW_ARTIFACT_MAX_BYTES: usize = 64 * 1024;
const REVIEW_ARTIFACT_RETENTION_LIMIT: usize = 32;
const MAX_JSON_NESTING: usize = 12;
const MAX_FINDINGS: usize = 64;
const MAX_QUESTIONS: usize = 32;
const MAX_UNCHECKED_ITEMS: usize = 32;
const MAX_REVIEW_TEXT_BYTES: usize = 2_000;
const MAX_SEMANTIC_FIELD_BYTES: usize = 500;
const CLEAN_ROOM_REVIEW_V2_SCHEMA_VERSION: u32 = 2;
const CLEAN_ROOM_REVIEW_V2_SCHEMA: &str = "csa.convergence.clean-room-review/v2";
const LEGACY_CLEAN_ROOM_REVIEW_V1_SCHEMA_VERSION: u32 = 1;
const LEGACY_CLEAN_ROOM_REVIEW_KIND: &str = "convergence_clean_room_review";
const REVIEW_ARTIFACT_FILE_PREFIX: &str = "clean-room-review-v2-";

/// A provider finding stripped of paths, commands, artifact references, and authority fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct CleanRoomFinding {
    stable_id: StableFindingId,
    semantic_identity: SemanticFindingIdentity,
    review_text: String,
}

impl CleanRoomFinding {
    pub(crate) fn stable_id(&self) -> &StableFindingId {
        &self.stable_id
    }

    pub(crate) fn semantic_identity(&self) -> &SemanticFindingIdentity {
        &self.semantic_identity
    }

    pub(crate) fn review_text(&self) -> &str {
        &self.review_text
    }
}

impl<'de> Deserialize<'de> for CleanRoomFinding {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct RawCleanRoomFinding {
            stable_id: StableFindingId,
            semantic_identity: SemanticFindingIdentity,
            review_text: String,
        }

        let raw = RawCleanRoomFinding::deserialize(deserializer)?;
        let semantic_identity =
            sanitize_semantic_identity(raw.semantic_identity).map_err(D::Error::custom)?;
        let expected = StableFindingId::compute(&semantic_identity);
        if raw.stable_id != expected {
            return Err(D::Error::custom("clean-room v2 finding stable ID mismatch"));
        }
        Ok(Self {
            stable_id: expected,
            semantic_identity,
            review_text: sanitize_review_text("clean-room finding review text", &raw.review_text)
                .map_err(D::Error::custom)?,
        })
    }
}

/// Host-bound clean-room review output accepted by the completion reducer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CleanRoomReviewOutput {
    artifact: ArtifactEvidenceRef,
    model_evidence: ModelEvidence,
    findings: Vec<CleanRoomFinding>,
    questions: Vec<String>,
    unchecked_items: Vec<String>,
    review_text: String,
}

impl CleanRoomReviewOutput {
    pub(crate) fn artifact(&self) -> &ArtifactEvidenceRef {
        &self.artifact
    }

    pub(crate) fn model_evidence(&self) -> &ModelEvidence {
        &self.model_evidence
    }

    pub(crate) fn findings(&self) -> &[CleanRoomFinding] {
        &self.findings
    }

    pub(crate) fn questions(&self) -> &[String] {
        &self.questions
    }

    pub(crate) fn unchecked_items(&self) -> &[String] {
        &self.unchecked_items
    }

    pub(crate) fn review_text(&self) -> &str {
        &self.review_text
    }
}

/// Host-owned bindings that must be present in every v2 review envelope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReviewEnvelopeContext {
    campaign_id: CampaignId,
    epoch: EpochRecord,
    gate_artifact: ArtifactEvidenceRef,
    model_evidence: ModelEvidence,
}

impl ReviewEnvelopeContext {
    pub(crate) fn new(
        campaign_id: CampaignId,
        epoch: EpochRecord,
        gate_artifact: ArtifactEvidenceRef,
        model_evidence: ModelEvidence,
    ) -> Self {
        Self {
            campaign_id,
            epoch,
            gate_artifact,
            model_evidence,
        }
    }
}

/// Trusted host-only artifact directory for one clean-room review session.
#[derive(Debug, Clone)]
pub(crate) struct HostReviewArtifactStore {
    directory: PathBuf,
    session_id: CsaSessionId,
    relative_directory: SessionRelativeArtifactPath,
}

impl HostReviewArtifactStore {
    /// Open a pre-existing host-owned output directory. The caller determines its lifecycle;
    /// provider input never contributes to this filesystem location.
    pub(crate) fn new(
        directory: &Path,
        session_id: CsaSessionId,
        relative_directory: SessionRelativeArtifactPath,
    ) -> Result<Self> {
        let directory = fs::canonicalize(directory).with_context(|| {
            format!(
                "canonicalize host clean-room review artifact directory {}",
                directory.display()
            )
        })?;
        if !directory.is_absolute() || !directory.is_dir() {
            bail!("host clean-room review artifact directory must be an absolute directory");
        }
        Ok(Self {
            directory,
            session_id,
            relative_directory,
        })
    }

    /// Parse, envelope, atomically publish once, and read back a v2 review artifact.
    pub(crate) fn publish(
        &self,
        context: &ReviewEnvelopeContext,
        provider_response: &str,
    ) -> Result<CleanRoomReviewOutput> {
        let review = parse_provider_clean_room_response(provider_response)?;
        let envelope = HostCleanRoomReviewEnvelope::new(context, review);
        let bytes = serde_json::to_vec(&envelope).context("serialize clean-room v2 envelope")?;
        if bytes.len() > REVIEW_ARTIFACT_MAX_BYTES {
            bail!("clean-room v2 envelope exceeds its artifact byte quota");
        }
        let digest = Sha256Digest::compute(&bytes);
        let file_name = artifact_file_name(&digest)?;
        let path = self.directory.join(&file_name);
        self.enforce_retention(&path)?;
        publish_bytes_once(&self.directory, &path, &bytes)?;
        let artifact = ArtifactEvidenceRef::new(
            self.session_id.clone(),
            SessionRelativeArtifactPath::new(&format!(
                "{}/{}",
                self.relative_directory.as_str(),
                file_name
            ))?,
            digest,
        );
        self.readback(context, &artifact)
    }

    /// Read and verify a previously host-published artifact against all authoritative bindings.
    pub(crate) fn readback(
        &self,
        context: &ReviewEnvelopeContext,
        artifact: &ArtifactEvidenceRef,
    ) -> Result<CleanRoomReviewOutput> {
        if artifact.csa_session_id() != &self.session_id {
            bail!("clean-room v2 artifact session identity does not match its host store");
        }
        let prefix = format!("{}/", self.relative_directory.as_str());
        let file_name = artifact
            .path()
            .as_str()
            .strip_prefix(&prefix)
            .context("clean-room v2 artifact path is outside its host output directory")?;
        if Path::new(file_name).components().count() != 1
            || !file_name.starts_with(REVIEW_ARTIFACT_FILE_PREFIX)
        {
            bail!("clean-room v2 artifact path is invalid");
        }
        let bytes = read_private_bounded(&self.directory.join(file_name))?;
        if Sha256Digest::compute(&bytes) != *artifact.digest() {
            bail!("clean-room v2 artifact digest mismatch");
        }
        let envelope = parse_host_envelope(&bytes)?;
        envelope.validate_context(context)?;
        Ok(envelope.into_output(artifact.clone()))
    }

    fn enforce_retention(&self, destination: &Path) -> Result<()> {
        if destination.exists() {
            return Ok(());
        }
        let mut retained = 0_usize;
        for entry in fs::read_dir(&self.directory).with_context(|| {
            format!(
                "list clean-room review artifacts in {}",
                self.directory.display()
            )
        })? {
            let entry = entry.with_context(|| {
                format!(
                    "read clean-room review artifact directory entry from {}",
                    self.directory.display()
                )
            })?;
            if entry
                .file_name()
                .to_string_lossy()
                .starts_with(REVIEW_ARTIFACT_FILE_PREFIX)
            {
                retained = retained.saturating_add(1);
            }
        }
        if retained >= REVIEW_ARTIFACT_RETENTION_LIMIT {
            bail!("clean-room v2 artifact retention quota is exhausted");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct HostCleanRoomReviewEnvelope {
    schema_version: u32,
    schema: String,
    campaign_id: CampaignId,
    epoch: EpochRecord,
    gate_artifact: ArtifactEvidenceRef,
    model_evidence: ModelEvidence,
    review: ProviderCleanRoomReview,
}

impl HostCleanRoomReviewEnvelope {
    fn new(context: &ReviewEnvelopeContext, review: ProviderCleanRoomReview) -> Self {
        Self {
            schema_version: CLEAN_ROOM_REVIEW_V2_SCHEMA_VERSION,
            schema: CLEAN_ROOM_REVIEW_V2_SCHEMA.to_string(),
            campaign_id: context.campaign_id.clone(),
            epoch: context.epoch.clone(),
            gate_artifact: context.gate_artifact.clone(),
            model_evidence: context.model_evidence.clone(),
            review,
        }
    }

    fn validate_context(&self, context: &ReviewEnvelopeContext) -> Result<()> {
        if self.schema_version != CLEAN_ROOM_REVIEW_V2_SCHEMA_VERSION
            || self.schema != CLEAN_ROOM_REVIEW_V2_SCHEMA
        {
            bail!("unsupported clean-room review envelope schema");
        }
        if self.campaign_id != context.campaign_id
            || self.epoch != context.epoch
            || self.gate_artifact != context.gate_artifact
            || self.model_evidence != context.model_evidence
        {
            bail!("clean-room v2 envelope does not match its host-authoritative bindings");
        }
        Ok(())
    }

    fn into_output(self, artifact: ArtifactEvidenceRef) -> CleanRoomReviewOutput {
        CleanRoomReviewOutput {
            artifact,
            model_evidence: self.model_evidence,
            findings: self.review.findings,
            questions: self.review.questions,
            unchecked_items: self.review.unchecked_items,
            review_text: self.review.review_text,
        }
    }
}

/// Provider-admitted review data. It intentionally has no artifact, model, path, digest,
/// schema, command, policy, or privilege field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ProviderCleanRoomReview {
    pub(super) findings: Vec<CleanRoomFinding>,
    pub(super) questions: Vec<String>,
    pub(super) unchecked_items: Vec<String>,
    pub(super) review_text: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawProviderCleanRoomReview {
    findings: Vec<RawProviderFinding>,
    questions: Vec<String>,
    unchecked_items: Vec<String>,
    review_text: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawProviderFinding {
    semantic_identity: SemanticFindingIdentity,
    review_text: String,
}

/// Parse the only JSON schema a provider may return for a v2 clean-room review.
pub(super) fn parse_provider_clean_room_response(raw: &str) -> Result<ProviderCleanRoomReview> {
    if raw.len() > PROVIDER_RESPONSE_MAX_BYTES {
        bail!("clean-room provider response exceeds its byte quota");
    }
    validate_json_nesting(raw)?;
    let mut deserializer = serde_json::Deserializer::from_str(raw);
    let parsed = RawProviderCleanRoomReview::deserialize(&mut deserializer)
        .context("invalid clean-room v2 provider JSON")?;
    deserializer
        .end()
        .context("clean-room v2 provider response contains trailing content")?;
    if parsed.findings.len() > MAX_FINDINGS {
        bail!("clean-room v2 finding count exceeds the bounded maximum");
    }
    let mut stable_ids = HashSet::new();
    let findings = parsed
        .findings
        .into_iter()
        .map(|finding| {
            let semantic_identity = sanitize_semantic_identity(finding.semantic_identity)?;
            let stable_id = StableFindingId::compute(&semantic_identity);
            if !stable_ids.insert(stable_id.as_str().to_string()) {
                bail!("duplicate semantic identity in clean-room v2 provider response");
            }
            Ok(CleanRoomFinding {
                stable_id,
                semantic_identity,
                review_text: sanitize_review_text(
                    "clean-room finding review text",
                    &finding.review_text,
                )?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(ProviderCleanRoomReview {
        findings,
        questions: sanitize_list("clean-room question", parsed.questions, MAX_QUESTIONS)?,
        unchecked_items: sanitize_list(
            "clean-room unchecked item",
            parsed.unchecked_items,
            MAX_UNCHECKED_ITEMS,
        )?,
        review_text: sanitize_review_text("clean-room review text", &parsed.review_text)?,
    })
}

/// Parse a v1 envelope only for inspection. This type cannot be turned into v2 terminal data.
pub(super) fn parse_legacy_v1_clean_room_review_for_read_only(raw: &str) -> Result<()> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct LegacyReview {
        schema_version: u32,
        kind: String,
        artifact: ArtifactEvidenceRef,
        model_identity: csa_session::convergence::AdmittedModelIdentity,
        findings: Vec<serde_json::Value>,
        questions: Vec<String>,
        unchecked_items: Vec<String>,
    }

    let mut deserializer = serde_json::Deserializer::from_str(raw);
    let legacy = LegacyReview::deserialize(&mut deserializer)
        .context("invalid legacy clean-room v1 review JSON")?;
    deserializer
        .end()
        .context("legacy clean-room v1 review contains trailing content")?;
    if legacy.schema_version != LEGACY_CLEAN_ROOM_REVIEW_V1_SCHEMA_VERSION
        || legacy.kind != LEGACY_CLEAN_ROOM_REVIEW_KIND
    {
        bail!("unsupported legacy clean-room v1 review schema");
    }
    let _ = (
        legacy.artifact,
        legacy.model_identity,
        legacy.findings,
        legacy.questions,
        legacy.unchecked_items,
    );
    Ok(())
}

fn parse_host_envelope(bytes: &[u8]) -> Result<HostCleanRoomReviewEnvelope> {
    if bytes.len() > REVIEW_ARTIFACT_MAX_BYTES {
        bail!("clean-room v2 artifact exceeds its byte quota");
    }
    let raw = std::str::from_utf8(bytes).context("clean-room v2 artifact is not UTF-8 JSON")?;
    validate_json_nesting(raw)?;
    let mut deserializer = serde_json::Deserializer::from_str(raw);
    let envelope = HostCleanRoomReviewEnvelope::deserialize(&mut deserializer)
        .context("invalid clean-room v2 artifact envelope")?;
    deserializer
        .end()
        .context("clean-room v2 artifact contains trailing content")?;
    Ok(envelope)
}

fn sanitize_semantic_identity(
    identity: SemanticFindingIdentity,
) -> Result<SemanticFindingIdentity> {
    SemanticFindingIdentity::new(
        &sanitize_semantic_field(identity.violated_invariant())?,
        &sanitize_semantic_field(identity.trigger_failure_mode())?,
        &sanitize_semantic_field(identity.primary_component())?,
        &sanitize_semantic_field(identity.bug_class())?,
    )
}

fn sanitize_semantic_field(value: &str) -> Result<String> {
    let value = sanitize_review_text("clean-room semantic identity", value)?;
    if value.len() > MAX_SEMANTIC_FIELD_BYTES {
        bail!("clean-room semantic identity exceeds its byte quota");
    }
    Ok(value)
}

fn sanitize_list(label: &str, values: Vec<String>, maximum: usize) -> Result<Vec<String>> {
    if values.len() > maximum {
        bail!("{label} count exceeds the bounded maximum");
    }
    let mut seen = HashSet::new();
    values
        .into_iter()
        .map(|value| {
            let value = sanitize_review_text(label, &value)?;
            if !seen.insert(value.clone()) {
                bail!("duplicate {label}");
            }
            Ok(value)
        })
        .collect()
}

fn sanitize_review_text(label: &str, value: &str) -> Result<String> {
    if value.is_empty()
        || value.trim() != value
        || value.chars().any(char::is_control)
        || value.len() > MAX_REVIEW_TEXT_BYTES
    {
        bail!("{label} must be nonblank, control-free, and within the byte quota");
    }
    Ok(redact_secret_like_text(value))
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
            let lowercase = token.to_ascii_lowercase();
            if lowercase == "bearer" {
                redact_next = true;
                return "Bearer".to_string();
            }
            if let Some((key, _)) = token.split_once('=') {
                let lower_key = key.to_ascii_lowercase();
                if matches!(
                    lower_key.as_str(),
                    "api_key" | "apikey" | "token" | "secret" | "password"
                ) {
                    return format!("{key}=[REDACTED]");
                }
            }
            if token.starts_with("sk-") && token.len() > 10
                || token.starts_with("AKIA") && token.len() >= 16
            {
                return "[REDACTED]".to_string();
            }
            token.to_string()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn validate_json_nesting(raw: &str) -> Result<()> {
    let mut depth = 0_usize;
    let mut in_string = false;
    let mut escaped = false;
    for byte in raw.bytes() {
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'\"' {
                in_string = false;
            }
            continue;
        }
        match byte {
            b'\"' => in_string = true,
            b'{' | b'[' => {
                depth = depth
                    .checked_add(1)
                    .context("clean-room JSON nesting overflow")?;
                if depth > MAX_JSON_NESTING {
                    bail!("clean-room JSON exceeds the nesting quota");
                }
            }
            b'}' | b']' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    Ok(())
}

fn artifact_file_name(digest: &Sha256Digest) -> Result<String> {
    let suffix = digest
        .as_str()
        .strip_prefix("sha256:")
        .context("clean-room artifact digest lacks the sha256 prefix")?;
    Ok(format!("{REVIEW_ARTIFACT_FILE_PREFIX}{suffix}.json"))
}

fn publish_bytes_once(directory: &Path, destination: &Path, bytes: &[u8]) -> Result<()> {
    let temporary = directory.join(format!(".clean-room-v2-{}.tmp", Ulid::new()));
    let result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&temporary)
            .with_context(|| {
                format!(
                    "create clean-room v2 temporary artifact {}",
                    temporary.display()
                )
            })?;
        file.write_all(bytes)
            .context("write clean-room v2 temporary artifact")?;
        file.sync_all()
            .context("sync clean-room v2 temporary artifact")?;
        match fs::hard_link(&temporary, destination) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let existing = read_private_bounded(destination)?;
                if existing != bytes {
                    bail!("clean-room v2 artifact name already exists with different bytes");
                }
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("publish clean-room v2 artifact {}", destination.display())
                });
            }
        }
        File::open(directory)
            .with_context(|| {
                format!(
                    "open clean-room v2 artifact directory {}",
                    directory.display()
                )
            })?
            .sync_all()
            .context("sync clean-room v2 artifact directory")?;
        Ok(())
    })();
    let cleanup = fs::remove_file(&temporary);
    result?;
    match cleanup {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(anyhow!(error).context("remove clean-room v2 temporary artifact")),
    }
}

fn read_private_bounded(path: &Path) -> Result<Vec<u8>> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .with_context(|| format!("open clean-room v2 artifact {}", path.display()))?;
    let mode = file
        .metadata()
        .context("inspect clean-room v2 artifact permissions")?
        .permissions()
        .mode();
    if mode & 0o077 != 0 {
        bail!("clean-room v2 artifact is not private (0600)");
    }
    let mut bytes = Vec::new();
    file.take((REVIEW_ARTIFACT_MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .with_context(|| format!("read clean-room v2 artifact {}", path.display()))?;
    if bytes.len() > REVIEW_ARTIFACT_MAX_BYTES {
        bail!("clean-room v2 artifact exceeds its byte quota");
    }
    Ok(bytes)
}
