//! Filesystem sandbox capability detection.
//!
//! Probes the host environment to determine which filesystem isolation
//! mechanism is available: bubblewrap (`bwrap`), Landlock LSM, or nothing.
//! The result is cached for the lifetime of the process via `OnceLock`.

use std::path::Path;
use std::process::Command;
use std::sync::OnceLock;

/// Filesystem isolation mechanism available on this host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilesystemCapability {
    /// Bubblewrap (`bwrap`) with functional user namespaces (best isolation).
    Bwrap,
    /// Linux Landlock LSM — kernel-level filesystem access control.
    Landlock,
    /// No usable filesystem isolation mechanism detected.
    None,
}

impl std::fmt::Display for FilesystemCapability {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bwrap => write!(f, "Bwrap"),
            Self::Landlock => write!(f, "Landlock"),
            Self::None => write!(f, "None"),
        }
    }
}

/// Process-wide cached probe result.
static CAPABILITY: OnceLock<FilesystemCapability> = OnceLock::new();

/// Return the detected filesystem sandbox capability, probing only once per process.
pub fn detect_filesystem_capability() -> FilesystemCapability {
    *CAPABILITY.get_or_init(probe_capability)
}

/// Perform the actual detection (called at most once).
fn probe_capability() -> FilesystemCapability {
    if has_bwrap() && has_usable_user_namespaces() {
        return FilesystemCapability::Bwrap;
    }

    if has_landlock() {
        return FilesystemCapability::Landlock;
    }

    FilesystemCapability::None
}

/// Check whether the `bwrap` binary is on `PATH`.
fn has_bwrap() -> bool {
    Command::new("which")
        .arg("bwrap")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Check whether unprivileged user namespaces are functional.
///
/// Two checks are performed:
/// 1. AppArmor restriction: if `/proc/sys/kernel/apparmor_restrict_unprivileged_userns`
///    exists and contains "1", user namespaces are restricted.
/// 2. Practical test: `unshare -U true` must succeed.
fn has_usable_user_namespaces() -> bool {
    if is_apparmor_userns_restricted() {
        return false;
    }

    Command::new("unshare")
        .args(["-U", "true"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Check whether AppArmor restricts unprivileged user namespaces.
fn is_apparmor_userns_restricted() -> bool {
    let path = Path::new("/proc/sys/kernel/apparmor_restrict_unprivileged_userns");
    std::fs::read_to_string(path)
        .map(|content| content.trim() == "1")
        .unwrap_or(false)
}

/// Check whether Landlock LSM is available on this kernel.
///
/// Looks for the Landlock sysfs entry, which is present when the kernel
/// was compiled with `CONFIG_SECURITY_LANDLOCK=y` and the LSM is active.
fn has_landlock() -> bool {
    Path::new("/sys/kernel/security/landlock").exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_bwrap_available() {
        // Integration-style: verify detection logic is consistent with
        // the individual probe functions on this host.
        let bwrap_ok = has_bwrap() && has_usable_user_namespaces();
        let result = probe_capability();
        if bwrap_ok {
            assert_eq!(
                result,
                FilesystemCapability::Bwrap,
                "expected Bwrap when bwrap is available and user namespaces work"
            );
        } else {
            assert_ne!(
                result,
                FilesystemCapability::Bwrap,
                "expected non-Bwrap when bwrap or user namespaces unavailable"
            );
        }
    }

    #[test]
    fn test_detect_landlock_fallback() {
        // When bwrap is not usable, Landlock should be the fallback if
        // the kernel supports it.
        let bwrap_ok = has_bwrap() && has_usable_user_namespaces();
        let landlock_ok = has_landlock();
        let result = probe_capability();

        if !bwrap_ok && landlock_ok {
            assert_eq!(
                result,
                FilesystemCapability::Landlock,
                "expected Landlock fallback when bwrap unavailable but Landlock present"
            );
        } else if !bwrap_ok && !landlock_ok {
            assert_eq!(
                result,
                FilesystemCapability::None,
                "expected None when neither bwrap nor Landlock available"
            );
        }
    }

    #[test]
    fn test_capability_caching() {
        let first = detect_filesystem_capability();
        let second = detect_filesystem_capability();
        assert_eq!(first, second, "cached result must be stable across calls");
    }

    #[test]
    fn test_display_variants() {
        assert_eq!(FilesystemCapability::Bwrap.to_string(), "Bwrap");
        assert_eq!(FilesystemCapability::Landlock.to_string(), "Landlock");
        assert_eq!(FilesystemCapability::None.to_string(), "None");
    }

    #[test]
    fn test_apparmor_check_missing_file() {
        // On hosts without the AppArmor sysctl, should return false
        // (not restricted). This test documents the expected fallback.
        let path_exists =
            Path::new("/proc/sys/kernel/apparmor_restrict_unprivileged_userns").exists();
        if !path_exists {
            assert!(
                !is_apparmor_userns_restricted(),
                "missing sysctl should not be treated as restricted"
            );
        }
    }
}
