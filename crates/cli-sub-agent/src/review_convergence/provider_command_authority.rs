//! Audited production authority for clean-room provider command construction.

#![expect(
    dead_code,
    reason = "B5 Slice 3B1 exposes an audited adapter seam before orchestration dispatch"
)]

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fmt;
use std::fs::{self, File, Metadata};
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use csa_core::types::ToolName;
use csa_executor::codex_runtime::CodexTransport;
use csa_executor::command_isolation::CleanCommandContract;
use csa_session::convergence::{AdmittedModelIdentity, CommandAuthoritySnapshot, Sha256Digest};
use sha2::{Digest, Sha256};

use super::clean_room::{ProviderSessionRequest, admitted_identity};
use crate::pipeline::AdmittedExecutor;

const COMMON_OPTIONAL_ENV_KEYS: &[&str] = &[
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "LANG",
    "LC_ALL",
    "NO_PROXY",
    "OPENSSL_CERT_DIR",
    "OPENSSL_CERT_FILE",
    "SSL_CERT_DIR",
    "SSL_CERT_FILE",
    "http_proxy",
    "https_proxy",
    "no_proxy",
];
const OPENAI_CREDENTIAL_KEY: &str = "OPENAI_API_KEY";
const ANTHROPIC_CREDENTIAL_KEY: &str = "ANTHROPIC_API_KEY";
const DERIVED_STATE_ENV: &[(&str, &str)] = &[
    ("HOME", "/tmp/csa-clean-room/home"),
    ("XDG_CACHE_HOME", "/tmp/csa-clean-room/xdg/cache"),
    ("XDG_CONFIG_HOME", "/tmp/csa-clean-room/xdg/config"),
    ("XDG_DATA_HOME", "/tmp/csa-clean-room/xdg/data"),
    ("XDG_STATE_HOME", "/tmp/csa-clean-room/xdg/state"),
];
const PROHIBITED_ENV_KEYS: &[&str] = &[
    "BASH_ENV",
    "CDPATH",
    "DYLD_INSERT_LIBRARIES",
    "ENV",
    "LD_LIBRARY_PATH",
    "LD_PRELOAD",
    "NODE_OPTIONS",
    "PYTHONPATH",
    "RUSTC_WRAPPER",
    "SHELLOPTS",
];

/// Explicitly captured environment sources. Construction never reads process environment.
pub(crate) struct ProviderEnvironmentInputs {
    audited_ambient: BTreeMap<String, String>,
    configured: BTreeMap<String, String>,
}

impl ProviderEnvironmentInputs {
    pub(crate) fn new(
        audited_ambient: BTreeMap<String, String>,
        configured: BTreeMap<String, String>,
    ) -> Self {
        Self {
            audited_ambient,
            configured,
        }
    }

    #[cfg(test)]
    pub(crate) fn insert_configured(&mut self, key: &str, value: &str) {
        self.configured.insert(key.to_string(), value.to_string());
    }
}

impl fmt::Debug for ProviderEnvironmentInputs {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProviderEnvironmentInputs")
            .field(
                "audited_ambient_keys",
                &self.audited_ambient.keys().collect::<Vec<_>>(),
            )
            .field(
                "configured_keys",
                &self.configured.keys().collect::<Vec<_>>(),
            )
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum EnvValueOrigin {
    AuditedAmbient,
    Configured,
    Derived,
}

impl EnvValueOrigin {
    fn label(self) -> &'static str {
        match self {
            Self::AuditedAmbient => "audited-ambient",
            Self::Configured => "configured",
            Self::Derived => "derived",
        }
    }
}

/// Closed, non-serializable provider environment with secret-safe diagnostics.
pub(crate) struct AuditedProviderEnvironment {
    values: BTreeMap<String, String>,
    origins: BTreeMap<String, EnvValueOrigin>,
    provenance_digest: Sha256Digest,
}

impl AuditedProviderEnvironment {
    pub(crate) fn capture(
        tool: &str,
        provider: &str,
        inputs: ProviderEnvironmentInputs,
    ) -> Result<Self> {
        let credential_key = credential_key_for(tool, provider)?;
        validate_environment_source(
            tool,
            provider,
            credential_key,
            "audited ambient",
            &inputs.audited_ambient,
        )?;
        validate_environment_source(
            tool,
            provider,
            credential_key,
            "configured",
            &inputs.configured,
        )?;

        let mut values = BTreeMap::new();
        let mut origins = BTreeMap::new();
        merge_environment_source(
            &mut values,
            &mut origins,
            inputs.audited_ambient,
            EnvValueOrigin::AuditedAmbient,
        );
        merge_environment_source(
            &mut values,
            &mut origins,
            inputs.configured,
            EnvValueOrigin::Configured,
        );
        for (key, value) in DERIVED_STATE_ENV {
            values.insert((*key).to_string(), (*value).to_string());
            origins.insert((*key).to_string(), EnvValueOrigin::Derived);
        }
        if tool == "codex" {
            values.insert(
                "CODEX_HOME".to_string(),
                "/tmp/csa-clean-room/codex".to_string(),
            );
            origins.insert("CODEX_HOME".to_string(), EnvValueOrigin::Derived);
        }

        let path = values
            .get("PATH")
            .context("clean-room provider environment requires PATH")?;
        validate_audited_path(path)?;
        let credential = values.get(credential_key).with_context(|| {
            format!(
                "clean-room provider environment requires credential key {credential_key} for {tool}/{provider}"
            )
        })?;
        if credential.is_empty() {
            bail!(
                "clean-room provider environment credential key {credential_key} must not be empty"
            );
        }

        let provenance_digest = environment_provenance_digest(tool, provider, &origins);
        Ok(Self {
            values,
            origins,
            provenance_digest,
        })
    }

    pub(crate) fn contains_key(&self, key: &str) -> bool {
        self.values.contains_key(key)
    }

    pub(crate) fn origin(&self, key: &str) -> Option<EnvValueOrigin> {
        self.origins.get(key).copied()
    }

    pub(crate) fn provenance_digest(&self) -> &Sha256Digest {
        &self.provenance_digest
    }

    fn value(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(String::as_str)
    }

    fn clean_values(&self) -> BTreeMap<String, String> {
        self.values.clone()
    }
}

impl fmt::Debug for AuditedProviderEnvironment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AuditedProviderEnvironment")
            .field("origins", &self.origins)
            .field("provenance_digest", &self.provenance_digest)
            .finish()
    }
}

fn credential_key_for(tool: &str, provider: &str) -> Result<&'static str> {
    match (tool, provider) {
        ("codex", "openai") | ("opencode", "openai") => Ok(OPENAI_CREDENTIAL_KEY),
        ("opencode", "anthropic") => Ok(ANTHROPIC_CREDENTIAL_KEY),
        ("codex" | "opencode", _) => {
            bail!("unsupported clean-room provider matrix entry {tool}/{provider}")
        }
        _ => bail!("unsupported clean-room provider tool {tool}"),
    }
}

fn validate_environment_source(
    tool: &str,
    provider: &str,
    credential_key: &str,
    source: &str,
    values: &BTreeMap<String, String>,
) -> Result<()> {
    for (key, value) in values {
        if key.is_empty() || key.contains(['=', '\0']) {
            bail!("invalid environment key in {source}");
        }
        if value.contains('\0') {
            bail!("environment key {key} has an invalid NUL value in {source}");
        }
        if is_prohibited_environment_key(key) {
            bail!("prohibited environment key {key} is not accepted from {source}");
        }
        let allowed = key == "PATH"
            || key == credential_key
            || COMMON_OPTIONAL_ENV_KEYS.contains(&key.as_str());
        if !allowed {
            bail!(
                "unknown environment key {key} is not allowed for clean-room provider {tool}/{provider} from {source}"
            );
        }
    }
    Ok(())
}

fn is_prohibited_environment_key(key: &str) -> bool {
    key.starts_with("CSA_")
        || key.starts_with("DYLD_")
        || key.starts_with("GIT_")
        || key.starts_with("LD_")
        || PROHIBITED_ENV_KEYS.contains(&key)
}

fn merge_environment_source(
    values: &mut BTreeMap<String, String>,
    origins: &mut BTreeMap<String, EnvValueOrigin>,
    source: BTreeMap<String, String>,
    origin: EnvValueOrigin,
) {
    for (key, value) in source {
        origins.insert(key.clone(), origin);
        values.insert(key, value);
    }
}

fn validate_audited_path(path: &str) -> Result<()> {
    if path.is_empty() {
        bail!("clean-room provider PATH must not be empty");
    }
    let components = std::env::split_paths(OsStr::new(path)).collect::<Vec<_>>();
    if components.is_empty() {
        bail!("clean-room provider PATH must contain an absolute component");
    }
    for component in components {
        if component.as_os_str().is_empty() || component == Path::new(".") {
            bail!("clean-room provider PATH must not contain empty or '.' components");
        }
        if !component.is_absolute() {
            bail!(
                "clean-room provider PATH component must be absolute: {}",
                component.display()
            );
        }
    }
    Ok(())
}

fn environment_provenance_digest(
    tool: &str,
    provider: &str,
    origins: &BTreeMap<String, EnvValueOrigin>,
) -> Sha256Digest {
    let mut canonical = Vec::new();
    canonical.extend_from_slice(b"csa-clean-room-provider-env-v1\0");
    canonical.extend_from_slice(tool.as_bytes());
    canonical.push(0);
    canonical.extend_from_slice(provider.as_bytes());
    canonical.push(0);
    for (key, origin) in origins {
        canonical.extend_from_slice(key.as_bytes());
        canonical.push(0);
        canonical.extend_from_slice(origin.label().as_bytes());
        canonical.extend_from_slice(b"\0present\0");
    }
    Sha256Digest::compute(&canonical)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProgramFingerprint {
    device: u64,
    inode: u64,
    length: u64,
    modified_seconds: i64,
    modified_nanoseconds: i64,
    content_digest: Sha256Digest,
}

/// Canonical executable identity captured from the audited PATH.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AbsoluteProviderProgram {
    expected_runtime: String,
    resolution_path: PathBuf,
    canonical_path: PathBuf,
    fingerprint: ProgramFingerprint,
}

impl AbsoluteProviderProgram {
    pub(crate) fn capture(expected_runtime: &str, resolution_path: &Path) -> Result<Self> {
        if expected_runtime.is_empty() || expected_runtime.contains(['/', '\0']) {
            bail!("provider runtime basename is invalid");
        }
        if !resolution_path.is_absolute() {
            bail!(
                "resolved provider program must be absolute: {}",
                resolution_path.display()
            );
        }
        if resolution_path.file_name() != Some(OsStr::new(expected_runtime)) {
            bail!(
                "resolved provider program basename must be {expected_runtime}: {}",
                resolution_path.display()
            );
        }
        let canonical_path = fs::canonicalize(resolution_path).with_context(|| {
            format!(
                "failed to canonicalize resolved provider program {}",
                resolution_path.display()
            )
        })?;
        if !canonical_path.is_absolute() {
            bail!("canonical provider program must be absolute");
        }
        let fingerprint = fingerprint_executable(&canonical_path)?;
        Ok(Self {
            expected_runtime: expected_runtime.to_string(),
            resolution_path: resolution_path.to_path_buf(),
            canonical_path,
            fingerprint,
        })
    }

    fn verify_integrity(&self) -> Result<()> {
        let current_target = fs::canonicalize(&self.resolution_path).with_context(|| {
            format!(
                "failed to re-resolve provider program {}",
                self.resolution_path.display()
            )
        })?;
        if current_target != self.canonical_path {
            bail!("provider program symlink target changed after authority capture");
        }
        let current = fingerprint_executable(&self.canonical_path)?;
        if current != self.fingerprint {
            bail!("provider program fingerprint changed after authority capture");
        }
        Ok(())
    }
}

fn fingerprint_executable(path: &Path) -> Result<ProgramFingerprint> {
    let metadata = fs::metadata(path)
        .with_context(|| format!("failed to inspect provider program {}", path.display()))?;
    validate_executable_metadata(path, &metadata)?;
    let content_digest = hash_file_streaming(path)?;
    program_fingerprint(&metadata, content_digest)
}

#[cfg(unix)]
fn validate_executable_metadata(path: &Path, metadata: &Metadata) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    if !metadata.is_file() {
        bail!(
            "provider program must be a regular file: {}",
            path.display()
        );
    }
    if metadata.permissions().mode() & 0o111 == 0 {
        bail!("provider program must be executable: {}", path.display());
    }
    Ok(())
}

#[cfg(not(unix))]
fn validate_executable_metadata(path: &Path, metadata: &Metadata) -> Result<()> {
    if !metadata.is_file() {
        bail!(
            "provider program must be a regular file: {}",
            path.display()
        );
    }
    Ok(())
}

#[cfg(unix)]
fn program_fingerprint(
    metadata: &Metadata,
    content_digest: Sha256Digest,
) -> Result<ProgramFingerprint> {
    use std::os::unix::fs::MetadataExt;

    Ok(ProgramFingerprint {
        device: metadata.dev(),
        inode: metadata.ino(),
        length: metadata.len(),
        modified_seconds: metadata.mtime(),
        modified_nanoseconds: metadata.mtime_nsec(),
        content_digest,
    })
}

#[cfg(not(unix))]
fn program_fingerprint(
    metadata: &Metadata,
    content_digest: Sha256Digest,
) -> Result<ProgramFingerprint> {
    use std::time::UNIX_EPOCH;

    let modified = metadata
        .modified()
        .context("provider program modification time is unavailable")?
        .duration_since(UNIX_EPOCH)
        .context("provider program modification time predates the Unix epoch")?;
    Ok(ProgramFingerprint {
        device: 0,
        inode: 0,
        length: metadata.len(),
        modified_seconds: i64::try_from(modified.as_secs())
            .context("provider program modification time exceeds i64")?,
        modified_nanoseconds: i64::from(modified.subsec_nanos()),
        content_digest,
    })
}

fn hash_file_streaming(path: &Path) -> Result<Sha256Digest> {
    let mut file = File::open(path)
        .with_context(|| format!("failed to open provider program {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .with_context(|| format!("failed to hash provider program {}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let digest = hasher.finalize();
    Sha256Digest::parse(&format!("sha256:{digest:x}"))
}

pub(crate) trait ProviderProgramResolver {
    fn resolve(
        &self,
        expected_runtime: &str,
        audited_path: &str,
        capture_cwd: &Path,
    ) -> Result<AbsoluteProviderProgram>;
}

pub(crate) struct SystemProviderProgramResolver;

impl ProviderProgramResolver for SystemProviderProgramResolver {
    fn resolve(
        &self,
        expected_runtime: &str,
        audited_path: &str,
        capture_cwd: &Path,
    ) -> Result<AbsoluteProviderProgram> {
        let resolved = which::which_in(expected_runtime, Some(audited_path), capture_cwd)
            .with_context(|| format!("failed to resolve provider runtime {expected_runtime}"))?;
        AbsoluteProviderProgram::capture(expected_runtime, &resolved)
    }
}

/// Deep binding between admission, immutable authority, environment, and executable identity.
pub(crate) struct ProviderCommandAuthority {
    selected_identity: AdmittedModelIdentity,
    authority_digest: Sha256Digest,
    tool: ToolName,
    runtime_binary: String,
    environment: AuditedProviderEnvironment,
    program: AbsoluteProviderProgram,
    binding_digest: Sha256Digest,
}

impl ProviderCommandAuthority {
    pub(crate) fn capture(
        admitted: &AdmittedExecutor,
        authority: &CommandAuthoritySnapshot,
        environment_inputs: ProviderEnvironmentInputs,
        capture_cwd: &Path,
        resolver: &dyn ProviderProgramResolver,
    ) -> Result<Self> {
        if !capture_cwd.is_absolute() || !capture_cwd.is_dir() {
            bail!(
                "provider authority capture cwd must be an existing absolute directory: {}",
                capture_cwd.display()
            );
        }
        let selected_identity = authority
            .ordered_admitted()
            .first()
            .context("provider command authority has no admitted model")?
            .clone();
        let actual_identity = admitted_identity(admitted)?;
        if actual_identity != selected_identity {
            bail!("provider command authority differs from the actual admitted executor identity");
        }
        let tool = clean_room_tool(&actual_identity)?;
        validate_direct_runtime(admitted, tool)?;
        let runtime_binary = admitted.runtime_binary_name().to_string();
        if runtime_binary != tool.as_str() {
            bail!(
                "provider runtime {runtime_binary} does not match admitted tool {}",
                tool.as_str()
            );
        }
        let environment = AuditedProviderEnvironment::capture(
            actual_identity.tool(),
            actual_identity.provider(),
            environment_inputs,
        )?;
        let audited_path = environment
            .value("PATH")
            .context("audited provider PATH disappeared during authority capture")?;
        let program = resolver.resolve(&runtime_binary, audited_path, capture_cwd)?;
        if program.expected_runtime != runtime_binary {
            bail!(
                "resolved provider program runtime {} does not match admitted runtime {runtime_binary}",
                program.expected_runtime
            );
        }
        let authority_digest = authority.digest();
        let binding_digest = binding_digest(
            &selected_identity,
            &authority_digest,
            &runtime_binary,
            environment.provenance_digest(),
            &program,
        );
        Ok(Self {
            selected_identity,
            authority_digest,
            tool,
            runtime_binary,
            environment,
            program,
            binding_digest,
        })
    }

    pub(crate) fn selected_identity(&self) -> &AdmittedModelIdentity {
        &self.selected_identity
    }

    pub(crate) fn authority_digest(&self) -> &Sha256Digest {
        &self.authority_digest
    }

    pub(crate) fn tool(&self) -> ToolName {
        self.tool
    }

    pub(crate) fn runtime_binary(&self) -> &str {
        &self.runtime_binary
    }

    pub(crate) fn program_path(&self) -> &Path {
        &self.program.canonical_path
    }

    pub(crate) fn environment_provenance_digest(&self) -> &Sha256Digest {
        self.environment.provenance_digest()
    }

    pub(crate) fn verify_program_integrity(&self) -> Result<()> {
        self.program.verify_integrity()
    }

    pub(crate) fn validate_request(
        &self,
        admitted: &AdmittedExecutor,
        request: &ProviderSessionRequest,
    ) -> Result<()> {
        if admitted_identity(admitted)? != self.selected_identity {
            bail!("provider command authority no longer matches the actual admitted executor");
        }
        if request.selected_model() != &self.selected_identity {
            bail!("clean-room request selected identity differs from provider command authority");
        }
        if request.authority_digest() != &self.authority_digest {
            bail!("clean-room request authority digest differs from provider command authority");
        }
        Ok(())
    }

    pub(super) fn validate_admitted_executor(&self, admitted: &AdmittedExecutor) -> Result<()> {
        if admitted_identity(admitted)? != self.selected_identity {
            bail!("provider command authority differs from the actual admitted executor");
        }
        validate_direct_runtime(admitted, self.tool)?;
        if admitted.runtime_binary_name() != self.runtime_binary {
            bail!("provider runtime drifted after authority capture");
        }
        Ok(())
    }

    pub(crate) fn clean_command_contract(
        &self,
        admitted: &AdmittedExecutor,
        request: &ProviderSessionRequest,
    ) -> Result<CleanCommandContract> {
        self.validate_request(admitted, request)?;
        self.verify_program_integrity()?;
        CleanCommandContract::try_new(
            &self.program.canonical_path,
            request.cwd(),
            self.environment.clean_values(),
        )
        .map_err(anyhow::Error::from)
    }
}

impl fmt::Debug for ProviderCommandAuthority {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProviderCommandAuthority")
            .field("selected_identity", &self.selected_identity)
            .field("authority_digest", &self.authority_digest)
            .field("tool", &self.tool)
            .field("runtime_binary", &self.runtime_binary)
            .field("environment", &self.environment)
            .field("program", &self.program)
            .field("binding_digest", &self.binding_digest)
            .finish()
    }
}

fn clean_room_tool(identity: &AdmittedModelIdentity) -> Result<ToolName> {
    match identity.tool() {
        "opencode" => Ok(ToolName::Opencode),
        "codex" => Ok(ToolName::Codex),
        tool => bail!("unsupported clean-room provider tool {tool}"),
    }
}

fn validate_direct_runtime(admitted: &AdmittedExecutor, tool: ToolName) -> Result<()> {
    if tool == ToolName::Codex {
        if admitted.codex_transport() != Some(CodexTransport::Cli) {
            bail!("clean-room Codex provider requires direct CLI transport");
        }
        if admitted.codex_tmux_mode_enabled() {
            bail!("clean-room Codex provider rejects tmux wrapping");
        }
    }
    Ok(())
}

fn binding_digest(
    identity: &AdmittedModelIdentity,
    authority_digest: &Sha256Digest,
    runtime_binary: &str,
    environment_digest: &Sha256Digest,
    program: &AbsoluteProviderProgram,
) -> Sha256Digest {
    let mut canonical = Vec::new();
    canonical.extend_from_slice(b"csa-provider-command-authority-v1\0");
    for value in [
        identity.tool(),
        identity.provider(),
        identity.model(),
        identity.reasoning(),
        authority_digest.as_str(),
        runtime_binary,
        environment_digest.as_str(),
        program.canonical_path.to_string_lossy().as_ref(),
        program.fingerprint.content_digest.as_str(),
    ] {
        canonical.extend_from_slice(value.as_bytes());
        canonical.push(0);
    }
    Sha256Digest::compute(&canonical)
}
