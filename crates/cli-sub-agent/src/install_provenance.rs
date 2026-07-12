//! Shared PATH/install provenance checks for installation and `csa doctor install`.
//!
//! Safety contract: never execute a PATH-resolved binary whose content bytes
//! differ from the trusted build artifact. Hash first; only run `--version`
//! against the artifact (or, when bytes match, treat the artifact version as
//! authoritative and skip redundant shadow execution).

use anyhow::{Context, Result, bail};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::audit::hash::hash_file;

/// Stable marker when a mismatched PATH binary was intentionally not executed.
pub(crate) const NOT_EXECUTED_MISMATCH: &str =
    "(not executed: PATH-resolved bytes differ from build artifact)";

/// Stable marker when full doctor would otherwise run an unverified PATH binary.
pub(crate) const NOT_EXECUTED_UNVERIFIED: &str =
    "(not executed: refuse to run unverified PATH-resolved binary)";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum InstallProvenanceStatus {
    Current,
    StaleShadow,
    UnsafeShadow,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InstallProvenanceReport {
    pub(crate) status: InstallProvenanceStatus,
    pub(crate) path_resolved: PathBuf,
    pub(crate) intended_target: PathBuf,
    pub(crate) artifact: PathBuf,
    pub(crate) artifact_hash: String,
    pub(crate) resolved_hash: String,
    pub(crate) artifact_version: String,
    /// Version banner from PATH-resolved binary, or a not-executed marker.
    pub(crate) version_output: String,
}

impl InstallProvenanceReport {
    pub(crate) fn is_current(&self) -> bool {
        self.status == InstallProvenanceStatus::Current
    }

    pub(crate) fn status_str(&self) -> &'static str {
        match self.status {
            InstallProvenanceStatus::Current => "current",
            InstallProvenanceStatus::StaleShadow => "stale_shadow",
            InstallProvenanceStatus::UnsafeShadow => "unsafe_shadow",
        }
    }

    pub(crate) fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "status": self.status_str(),
            "path_resolved": self.path_resolved.display().to_string(),
            "intended_target": self.intended_target.display().to_string(),
            "artifact": self.artifact.display().to_string(),
            "artifact_sha256": self.artifact_hash,
            "path_resolved_sha256": self.resolved_hash,
            "artifact_version": self.artifact_version,
            "path_resolved_version": self.version_output,
            "current": self.is_current(),
        })
    }

    pub(crate) fn diagnostic(&self) -> String {
        let summary = match self.status {
            InstallProvenanceStatus::Current => "active binary matches the newly built artifact",
            InstallProvenanceStatus::StaleShadow => {
                "PATH resolves a different executable; refusing to report installation success"
            }
            InstallProvenanceStatus::UnsafeShadow => {
                "PATH resolves a different executable that is not writable; refusing to report installation success"
            }
        };
        format!(
            "CSA install provenance: {summary}\n  PATH-resolved executable: {}\n  intended install target: {}\n  build artifact: {}\n  artifact sha256: {}\n  PATH-resolved sha256: {}\n  artifact version/source commit: {}\n  PATH-resolved version/source commit: {}\n{}",
            self.path_resolved.display(),
            self.intended_target.display(),
            self.artifact.display(),
            self.artifact_hash,
            self.resolved_hash,
            self.artifact_version,
            self.version_output,
            if self.is_current() {
                "  status: current"
            } else {
                "  remediation: update PATH so the intended target is first, then rerun `just install`; CSA will not overwrite arbitrary PATH entries."
            },
        )
    }
}

/// Default intended install target for `just install` / doctor surfaces.
///
/// Unix: `/usr/local/bin/csa`. Windows: `LOCALAPPDATA\\csa\\csa.exe` when set,
/// otherwise a non-Unix placeholder (the release `just install` recipe is
/// Unix-oriented).
pub(crate) fn default_intended_target() -> PathBuf {
    #[cfg(windows)]
    {
        env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"))
            .join("csa")
            .join("csa.exe")
    }
    #[cfg(not(windows))]
    {
        PathBuf::from("/usr/local/bin/csa")
    }
}

pub(crate) fn inspect_current_path(
    artifact: &Path,
    intended_target: &Path,
) -> Result<InstallProvenanceReport> {
    let path = env::var_os("PATH").context("PATH is not set")?;
    inspect(&path.to_string_lossy(), artifact, intended_target)
}

/// Resolve the PATH-first executable named `csa` (for doctor diagnostics).
pub(crate) fn resolve_current_path() -> Result<PathBuf> {
    let path = env::var_os("PATH").context("PATH is not set")?;
    resolve_from_path(&path.to_string_lossy())
}

pub(crate) fn inspect(
    path: &str,
    artifact: &Path,
    intended_target: &Path,
) -> Result<InstallProvenanceReport> {
    let path_resolved = resolve_from_path(path)?;
    let artifact_hash = hash_file(artifact)
        .with_context(|| format!("failed to hash artifact {}", artifact.display()))?;
    let resolved_hash = hash_file(&path_resolved).with_context(|| {
        format!(
            "failed to hash PATH-resolved executable {}",
            path_resolved.display()
        )
    })?;

    // Always version the trusted artifact only.
    let artifact_version = version_output(artifact)?;

    // Hash-first gate: never execute a PATH shadow whose bytes differ.
    // When bytes match, the artifact version is authoritative — skip redundant
    // shadow execution (same content cannot yield a different --version banner).
    let (status, version_output) = if artifact_hash == resolved_hash {
        (InstallProvenanceStatus::Current, artifact_version.clone())
    } else if is_writable(&path_resolved)? {
        (
            InstallProvenanceStatus::StaleShadow,
            NOT_EXECUTED_MISMATCH.to_string(),
        )
    } else {
        (
            InstallProvenanceStatus::UnsafeShadow,
            NOT_EXECUTED_MISMATCH.to_string(),
        )
    };

    Ok(InstallProvenanceReport {
        status,
        path_resolved,
        intended_target: intended_target.to_path_buf(),
        artifact: artifact.to_path_buf(),
        artifact_hash,
        resolved_hash,
        artifact_version,
        version_output,
    })
}

fn resolve_from_path(path: &str) -> Result<PathBuf> {
    for directory in env::split_paths(path) {
        for name in binary_candidates() {
            let candidate = directory.join(name);
            if is_executable_file(&candidate)? {
                return Ok(candidate);
            }
        }
    }
    bail!("could not resolve `csa` from PATH")
}

/// Platform-aware executable names searched on PATH.
fn binary_candidates() -> &'static [&'static str] {
    #[cfg(windows)]
    {
        // Prefer PATHEXT-typical names; bare `csa` last for rare no-extension cases.
        &["csa.exe", "csa.cmd", "csa.bat", "csa"]
    }
    #[cfg(not(windows))]
    {
        &["csa"]
    }
}

fn is_executable_file(path: &Path) -> Result<bool> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to inspect {}", path.display()));
        }
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        Ok(metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        Ok(metadata.is_file())
    }
}

fn is_writable(path: &Path) -> Result<bool> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        Ok(fs::metadata(path)?.permissions().mode() & 0o222 != 0)
    }
    #[cfg(not(unix))]
    {
        Ok(!fs::metadata(path)?.permissions().readonly())
    }
}

fn version_output(path: &Path) -> Result<String> {
    // Trusted paths only (callers enforce). Never mutates PATH entries.
    let output = Command::new(path)
        .arg("--version")
        .output()
        .with_context(|| format!("failed to run {} --version", path.display()))?;
    if !output.status.success() {
        bail!("{} --version exited with {}", path.display(), output.status);
    }
    String::from_utf8(output.stdout)
        .map(|value| value.trim().to_string())
        .context("csa --version returned non-UTF-8 output")
}

#[cfg(all(test, unix))]
#[path = "install_provenance_tests.rs"]
mod install_provenance_tests;
