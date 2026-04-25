use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::filesystem_sandbox::FilesystemCapability;

const CODEX_HOME_ENV: &str = "CODEX_HOME";
const CODEX_DEFAULT_HOME_REL: &str = ".codex";
const CODEX_SANDBOX_CONFIG_HINT: &str =
    "[tools.codex].filesystem_sandbox.writable_paths or [filesystem_sandbox].extra_writable";

#[derive(Debug, Clone)]
pub(super) struct RequiredWritableDir {
    path: PathBuf,
    source: &'static str,
    purpose: &'static str,
    config_hint: &'static str,
}

pub(super) fn add_codex_home_for_tool(
    tool_name: &str,
    home: &Path,
    writable_paths: &mut Vec<PathBuf>,
    required_writable_dirs: &mut Vec<RequiredWritableDir>,
) {
    let (codex_home, codex_home_source) = codex_home_dir(home);
    if tool_name == "codex" {
        if codex_home.is_absolute() {
            super::add_dir_or_creatable_parent(writable_paths, &codex_home);
        }
        required_writable_dirs.push(RequiredWritableDir {
            path: codex_home,
            source: codex_home_source,
            purpose: "Codex rollout recorder and arg0 PATH shim",
            config_hint: CODEX_SANDBOX_CONFIG_HINT,
        });
    } else if codex_home.is_absolute() && has_codex_on_path() {
        // Codex is installed — route through `add_dir_or_creatable_parent` so
        // the directory is pre-created when it doesn't exist yet (avoids
        // read-only-fs in nested CSA sessions spawning a codex child).
        super::add_dir_or_creatable_parent(writable_paths, &codex_home);
    }
}

pub(super) fn validate_required_writable_dirs(
    filesystem: FilesystemCapability,
    required_writable_dirs: &[RequiredWritableDir],
    writable_paths: &[PathBuf],
) -> anyhow::Result<()> {
    if filesystem == FilesystemCapability::None {
        return Ok(());
    }

    for required in required_writable_dirs {
        validate_required_writable_dir(required, writable_paths)?;
    }

    Ok(())
}

pub(super) fn codex_home_dir(home: &Path) -> (PathBuf, &'static str) {
    match std::env::var_os(CODEX_HOME_ENV) {
        Some(value) if !value.is_empty() => (PathBuf::from(value), CODEX_HOME_ENV),
        _ => (home.join(CODEX_DEFAULT_HOME_REL), "HOME/.codex"),
    }
}

/// Check whether any codex binary (`codex` or `codex-acp`) is on `PATH`.
pub(super) fn has_codex_on_path() -> bool {
    for binary in &["codex", "codex-acp"] {
        let found = Command::new("which")
            .arg(binary)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        if found {
            return true;
        }
    }
    false
}

fn path_is_covered_by_writable_mount(path: &Path, writable_paths: &[PathBuf]) -> bool {
    writable_paths
        .iter()
        .any(|candidate| path.starts_with(candidate))
}

fn validate_required_writable_dir(
    required: &RequiredWritableDir,
    writable_paths: &[PathBuf],
) -> anyhow::Result<()> {
    let path = &required.path;

    if !path.is_absolute() {
        anyhow::bail!(
            "codex sandbox preflight failed: required writable path {} ({}, source: {}) is not absolute. \
             Sandbox config key: {}",
            path.display(),
            required.purpose,
            required.source,
            required.config_hint
        );
    }

    if !path_is_covered_by_writable_mount(path, writable_paths) {
        anyhow::bail!(
            "codex sandbox preflight failed: required writable path {} ({}, source: {}) is missing from IsolationPlan.writable_paths. \
             Sandbox config key: {}",
            path.display(),
            required.purpose,
            required.source,
            required.config_hint
        );
    }

    if super::is_sensitive_system_path(path) {
        anyhow::bail!(
            "codex sandbox preflight failed: required writable path {} ({}, source: {}) is under a sensitive system directory. \
             Sandbox config key: {}",
            path.display(),
            required.purpose,
            required.source,
            required.config_hint
        );
    }

    if let Err(error) = fs::create_dir_all(path) {
        anyhow::bail!(
            "codex sandbox preflight failed: required writable path {} ({}, source: {}) could not be created before spawning Codex: {}. \
             Sandbox config key: {}",
            path.display(),
            required.purpose,
            required.source,
            error,
            required.config_hint
        );
    }

    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) => {
            anyhow::bail!(
                "codex sandbox preflight failed: required writable path {} ({}, source: {}) could not be inspected before spawning Codex: {}. \
                 Sandbox config key: {}",
                path.display(),
                required.purpose,
                required.source,
                error,
                required.config_hint
            );
        }
    };
    if !metadata.is_dir() {
        anyhow::bail!(
            "codex sandbox preflight failed: required writable path {} ({}, source: {}) exists but is not a directory. \
             Sandbox config key: {}",
            path.display(),
            required.purpose,
            required.source,
            required.config_hint
        );
    }

    probe_writable_dir(path, required)
}

fn probe_writable_dir(path: &Path, required: &RequiredWritableDir) -> anyhow::Result<()> {
    for attempt in 0..16 {
        let probe = path.join(format!(".csa-write-probe-{}-{attempt}", std::process::id()));
        match OpenOptions::new().write(true).create_new(true).open(&probe) {
            Ok(_) => {
                // Write succeeded — that is the actual signal.  Cleanup failure
                // is downgraded to a warning so flaky mounts (NFS, sshfs) don't
                // fail the preflight for a non-critical hygiene step.
                if let Err(error) = fs::remove_file(&probe) {
                    tracing::warn!(
                        probe = %probe.display(),
                        %error,
                        "could not remove write probe (non-fatal)"
                    );
                }
                return Ok(());
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                anyhow::bail!(
                    "codex sandbox preflight failed: required writable path {} ({}, source: {}) is not writable before spawning Codex: {}. \
                     Sandbox config key: {}. If this is a nested CSA session, ensure the parent filesystem sandbox also exposes this same canonical Codex home path.",
                    path.display(),
                    required.purpose,
                    required.source,
                    error,
                    required.config_hint
                );
            }
        }
    }

    anyhow::bail!(
        "codex sandbox preflight failed: could not allocate a unique write probe under {} ({}, source: {}). \
         Sandbox config key: {}",
        path.display(),
        required.purpose,
        required.source,
        required.config_hint
    )
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    #[test]
    fn root_writable_mount_covers_absolute_subpaths() {
        let writable_paths = [PathBuf::from("/")];

        assert!(super::path_is_covered_by_writable_mount(
            Path::new("/home/user/.codex"),
            &writable_paths
        ));
        assert!(super::path_is_covered_by_writable_mount(
            Path::new("/"),
            &writable_paths
        ));
    }

    #[test]
    fn writable_mount_covers_itself_and_descendants_only() {
        let writable_paths = [PathBuf::from("/home/user")];

        assert!(super::path_is_covered_by_writable_mount(
            Path::new("/home/user"),
            &writable_paths
        ));
        assert!(super::path_is_covered_by_writable_mount(
            Path::new("/home/user/.codex"),
            &writable_paths
        ));
        assert!(!super::path_is_covered_by_writable_mount(
            Path::new("/home/user2/.codex"),
            &writable_paths
        ));
    }
}
