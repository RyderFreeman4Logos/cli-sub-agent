//! Landlock LSM filesystem sandbox — kernel-level access control fallback.
//!
//! When bubblewrap (`bwrap`) is unavailable (e.g. AppArmor restricts user
//! namespaces), Landlock provides a lightweight, in-process alternative for
//! filesystem isolation.  It requires Linux 5.13+ (ABI v1) and degrades
//! gracefully on older kernels via `BestEffort` compatibility mode.
//!
//! # Usage
//!
//! Call [`apply_landlock_rules`] inside a `pre_exec` closure — it restricts
//! the calling thread (and therefore the about-to-be-exec'd child) to
//! read-only access on the root filesystem, with write access granted only
//! to the directories listed in `writable_paths`.

use std::path::PathBuf;

use landlock::{
    ABI, Access, AccessFs, CompatLevel, Compatible, PathBeneath, PathFd, RestrictionStatus,
    Ruleset, RulesetAttr, RulesetCreatedAttr, RulesetStatus,
};
use tracing::{debug, warn};

/// Target ABI version.  V3 (Linux 6.2) adds `Truncate`; we request it but
/// `BestEffort` silently drops unsupported rights on older kernels.
const TARGET_ABI: ABI = ABI::V3;

/// Apply Landlock filesystem rules to the current thread.
///
/// After this call the thread (and any process it `exec`s) will:
/// - Have **read + execute** access to the entire root filesystem.
/// - Have **full read-write** access only to `writable_paths`.
///
/// Uses `BestEffort` compatibility so the function succeeds even when the
/// running kernel does not support Landlock — in that case it logs a warning
/// and returns `Ok(())`.
///
/// # Errors
///
/// Returns an error only on unexpected system failures (e.g. invalid fd).
/// Kernel-level lack of support is *not* an error in BestEffort mode.
pub fn apply_landlock_rules(writable_paths: &[PathBuf]) -> anyhow::Result<()> {
    let status = build_and_restrict(writable_paths)?;
    report_status(&status);
    Ok(())
}

/// Detect the highest Landlock ABI version supported by the running kernel.
///
/// Returns [`ABI::Unsupported`] when the kernel has no Landlock support.
pub fn detect_abi() -> ABI {
    // Creating a default Ruleset probes the kernel; we inspect the
    // resulting status after a minimal restrict_self() to determine
    // support.  However, restrict_self() is destructive (irreversible),
    // so we instead check the sysfs entry and try the syscall-based
    // detection that the crate performs internally via Ruleset::default().
    //
    // The landlock crate does not expose a standalone ABI query, but
    // Ruleset::default() internally calls landlock_create_ruleset with
    // LANDLOCK_CREATE_RULESET_VERSION.  We can observe the result
    // through the RestrictionStatus after a BestEffort restrict.
    //
    // For a non-destructive check we rely on the sysfs heuristic plus
    // a version probe via the crate's own constants.
    probe_abi_version()
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Build the ruleset and restrict the current thread.
fn build_and_restrict(writable_paths: &[PathBuf]) -> anyhow::Result<RestrictionStatus> {
    let access_all = AccessFs::from_all(TARGET_ABI);
    let access_read = AccessFs::from_read(TARGET_ABI);

    let mut ruleset = Ruleset::default()
        .set_compatibility(CompatLevel::BestEffort)
        .handle_access(access_all)?
        .create()?
        .set_compatibility(CompatLevel::BestEffort);

    // Grant read+execute to the whole filesystem tree.
    let root_fd = PathFd::new("/")?;
    ruleset = ruleset.add_rule(PathBeneath::new(root_fd, access_read))?;

    // Grant full access to each writable directory.
    for path in writable_paths {
        match PathFd::new(path) {
            Ok(fd) => {
                ruleset = ruleset.add_rule(PathBeneath::new(fd, access_all))?;
            }
            Err(e) => {
                // Path might not exist yet (e.g. session dir created later).
                // BestEffort: skip and warn rather than failing.
                warn!(
                    path = %path.display(),
                    error = %e,
                    "landlock: skipping writable path (cannot open)"
                );
            }
        }
    }

    Ok(ruleset.restrict_self()?)
}

/// Log the restriction outcome for diagnostics.
fn report_status(status: &RestrictionStatus) {
    match status.ruleset {
        RulesetStatus::FullyEnforced => {
            debug!("landlock: fully enforced");
        }
        RulesetStatus::PartiallyEnforced => {
            debug!("landlock: partially enforced (kernel supports subset of requested ABI)");
        }
        RulesetStatus::NotEnforced => {
            warn!("landlock: not enforced (kernel lacks Landlock support; BestEffort passthrough)");
        }
    }
}

/// Non-destructive ABI version probe.
///
/// Checks the sysfs entry first, then uses `landlock_create_ruleset`
/// version probing through the crate's `Ruleset` builder.
fn probe_abi_version() -> ABI {
    use std::path::Path;

    // Quick sysfs check — if the directory is missing, Landlock is not
    // compiled into the kernel at all.
    if !Path::new("/sys/kernel/security/landlock").exists() {
        return ABI::Unsupported;
    }

    // The crate doesn't expose a direct ABI query, but we can observe
    // which access flags are handled.  As a pragmatic approach, try
    // creating rulesets at descending ABI levels and see what the kernel
    // accepts.  Since we never call restrict_self() this is safe and
    // non-destructive.
    for &abi in &[ABI::V6, ABI::V5, ABI::V4, ABI::V3, ABI::V2, ABI::V1] {
        let access = AccessFs::from_all(abi);
        if Ruleset::default()
            .set_compatibility(CompatLevel::HardRequirement)
            .handle_access(access)
            .and_then(|r| r.create())
            .is_ok()
        {
            return abi;
        }
    }

    ABI::Unsupported
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_landlock_rules_basic() {
        // Verify the rule builder does not panic, even when the kernel
        // lacks Landlock support.  We build the ruleset in BestEffort
        // mode without calling restrict_self() (which is irreversible).
        let access_all = AccessFs::from_all(TARGET_ABI);
        let access_read = AccessFs::from_read(TARGET_ABI);

        let result = Ruleset::default()
            .set_compatibility(CompatLevel::BestEffort)
            .handle_access(access_all)
            .and_then(|r| r.create())
            .map(|r| r.set_compatibility(CompatLevel::BestEffort))
            .and_then(|r| r.add_rule(PathBeneath::new(PathFd::new("/").unwrap(), access_read)));

        // BestEffort should never return an error for rule construction.
        assert!(result.is_ok(), "BestEffort rule build failed: {result:?}");
    }

    #[test]
    fn test_landlock_abi_detection() {
        let abi = detect_abi();

        // On any Linux host this should return a concrete value (possibly
        // Unsupported on old kernels / non-Linux CI).
        if std::path::Path::new("/sys/kernel/security/landlock").exists() {
            assert!(
                abi != ABI::Unsupported,
                "sysfs entry exists but ABI reported as Unsupported"
            );
            debug!("detected Landlock ABI: {abi:?}");
        } else {
            assert_eq!(
                abi,
                ABI::Unsupported,
                "no sysfs entry but ABI reported as supported"
            );
        }
    }

    #[test]
    fn test_landlock_best_effort_fallback() {
        // On kernels without Landlock, BestEffort must succeed silently.
        // On kernels with Landlock, it must also succeed.  Either way
        // the function returns Ok.
        //
        // NOTE: We cannot call apply_landlock_rules in tests because
        // restrict_self() is irreversible and would affect the test
        // process.  Instead we verify the build+restrict_self path
        // returns a valid RestrictionStatus.
        let writable = vec![PathBuf::from("/tmp")];
        let result = build_and_restrict(&writable);

        match result {
            Ok(status) => {
                // Any status is acceptable — the point is no error.
                debug!("restriction status: {:?}", status.ruleset);
            }
            Err(e) => {
                // The only acceptable failure is a non-Landlock system error,
                // not a compatibility error (BestEffort should mask those).
                panic!("BestEffort build_and_restrict failed unexpectedly: {e}");
            }
        }
    }

    #[test]
    fn test_landlock_missing_writable_path_skipped() {
        // A non-existent writable path should be silently skipped in
        // BestEffort mode, not cause an error.
        let writable = vec![
            PathBuf::from("/tmp"),
            PathBuf::from("/nonexistent_csa_test_path_12345"),
        ];
        let result = build_and_restrict(&writable);
        assert!(
            result.is_ok(),
            "missing writable path should be skipped: {result:?}"
        );
    }
}
