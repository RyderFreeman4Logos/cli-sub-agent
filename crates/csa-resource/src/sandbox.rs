//! Sandbox capability detection.
//!
//! Probes the host environment to determine which resource isolation
//! mechanism is available: cgroup v2 (via systemd user scope), POSIX
//! `setrlimit`, or nothing.  The result is cached for the lifetime of the
//! process via `OnceLock`.

use std::path::Path;
use std::process::Command;
use std::sync::OnceLock;

/// Resource-isolation mechanism available on this host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxCapability {
    /// cgroup v2 with systemd user-scope support (best isolation).
    CgroupV2,
    /// POSIX `setrlimit` — PID limit only (`RLIMIT_NPROC`).
    Setrlimit,
    /// No usable isolation mechanism detected.
    None,
}

impl std::fmt::Display for SandboxCapability {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CgroupV2 => write!(f, "CgroupV2"),
            Self::Setrlimit => write!(f, "Setrlimit"),
            Self::None => write!(f, "None"),
        }
    }
}

/// Process-wide cached probe result.
static CAPABILITY: OnceLock<SandboxCapability> = OnceLock::new();

/// Return the detected sandbox capability, probing only once per process.
pub fn detect_sandbox_capability() -> SandboxCapability {
    *CAPABILITY.get_or_init(probe_capability)
}

/// Perform the actual detection (called at most once).
fn probe_capability() -> SandboxCapability {
    if has_cgroup_v2() && has_systemd_user_scope() {
        return SandboxCapability::CgroupV2;
    }

    if has_setrlimit() {
        return SandboxCapability::Setrlimit;
    }

    SandboxCapability::None
}

/// Check whether cgroup v2 unified hierarchy is mounted.
fn has_cgroup_v2() -> bool {
    Path::new("/sys/fs/cgroup/cgroup.controllers").exists()
}

/// Check whether `systemd-run --user --scope` is functional.
///
/// Runs a trivial command (`/bin/true`) inside a transient scope to verify
/// that the systemd user instance is available and scope creation works.
/// Previously used `--dry-run` which requires systemd >= 253 (not 236 as
/// documented) and silently fails on older versions like Debian 12 (systemd 252).
fn has_systemd_user_scope() -> bool {
    Command::new("systemd-run")
        .args(["--user", "--scope", "--quiet", "/bin/true"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Check whether `setrlimit` is available (always true on Linux/macOS).
fn has_setrlimit() -> bool {
    cfg!(any(target_os = "linux", target_os = "macos"))
}

/// Return the systemd version string, if `systemd-run` is present.
pub fn systemd_version() -> Option<String> {
    let output = Command::new("systemd-run")
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout.lines().next().map(|s| s.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_returns_consistent_result() {
        let first = detect_sandbox_capability();
        let second = detect_sandbox_capability();
        assert_eq!(first, second, "cached result must be stable");
    }

    #[test]
    fn test_display_variants() {
        assert_eq!(SandboxCapability::CgroupV2.to_string(), "CgroupV2");
        assert_eq!(SandboxCapability::Setrlimit.to_string(), "Setrlimit");
        assert_eq!(SandboxCapability::None.to_string(), "None");
    }

    #[test]
    fn test_has_setrlimit_on_linux() {
        // On Linux CI this must be true.
        if cfg!(target_os = "linux") {
            assert!(has_setrlimit());
        }
    }

    #[test]
    fn test_has_cgroup_v2_matches_filesystem() {
        let expected = Path::new("/sys/fs/cgroup/cgroup.controllers").exists();
        assert_eq!(has_cgroup_v2(), expected);
    }
}
