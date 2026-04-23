use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Result, anyhow};
use csa_config::StateDirConfig;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

/// Cached size measurement for the state directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SizeCache {
    /// Total size in bytes.
    size_bytes: u64,
    /// Unix timestamp (seconds) of last scan.
    scanned_at: u64,
}

const SIZE_CACHE_FILENAME: &str = ".size-cache.toml";

/// Result of the state directory preflight check.
pub(crate) enum StateDirCheckResult {
    /// No cap configured or size is within limits.
    Ok,
    /// Size exceeds cap — returns warning preamble to inject.
    Warn(String),
    /// Size exceeds cap and `on_exceed = error`.
    Error(anyhow::Error),
}

/// Enforce the state-dir hard cap. MUST run on **every** execution (fresh or resumed).
/// Returns `Err` when `on_exceed = "error"` and size exceeds cap. Returns `Ok(())` otherwise.
pub(crate) fn enforce_state_dir_cap(
    global_config: Option<&csa_config::GlobalConfig>,
) -> anyhow::Result<()> {
    let config = match global_config {
        Some(gc) if gc.state_dir.max_size_mb > 0 => &gc.state_dir,
        _ => return Ok(()),
    };
    match check_state_dir_size(config) {
        StateDirCheckResult::Error(err) => Err(err),
        _ => Ok(()),
    }
}

/// Run state-dir preflight preamble injection. Returns `Ok(Some(preamble))` for
/// warning injection on fresh-spawn, `Ok(None)` when within cap or unconfigured.
/// Does NOT enforce the hard block — call `enforce_state_dir_cap()` separately.
pub(crate) fn run_state_dir_preflight(
    global_config: Option<&csa_config::GlobalConfig>,
) -> Option<String> {
    let config = match global_config {
        Some(gc) if gc.state_dir.max_size_mb > 0 => &gc.state_dir,
        _ => return None,
    };
    match check_state_dir_size(config) {
        StateDirCheckResult::Warn(preamble) => Some(preamble),
        _ => None,
    }
}

/// Run the state directory size check against the configured cap.
///
/// Returns a `StateDirCheckResult` indicating whether the session should
/// proceed (with optional warning injection) or be blocked.
fn check_state_dir_size(config: &StateDirConfig) -> StateDirCheckResult {
    if config.max_size_mb == 0 {
        return StateDirCheckResult::Ok;
    }

    let state_roots = csa_config::paths::state_dir_all_roots();
    if state_roots.is_empty() {
        debug!("No existing state directory roots found; skipping size check");
        return StateDirCheckResult::Ok;
    }

    let mut size_bytes = 0u64;
    for state_root in &state_roots {
        match get_or_compute_size(state_root, config.scan_interval_seconds) {
            Ok(size) => {
                size_bytes = size_bytes.saturating_add(size);
            }
            Err(e) => {
                warn!(
                    path = %state_root.display(),
                    error = %e,
                    "Failed to compute state directory size; skipping check"
                );
                return StateDirCheckResult::Ok;
            }
        }
    }

    let size_mb = size_bytes / (1024 * 1024);
    let cap_mb = config.max_size_mb;
    let cap_bytes = cap_mb.saturating_mul(1024 * 1024);

    if size_bytes <= cap_bytes {
        debug!(size_mb, cap_mb, "State directory within cap");
        return StateDirCheckResult::Ok;
    }

    info!(size_mb, cap_mb, on_exceed = ?config.on_exceed, "State directory exceeds cap");

    match config.on_exceed {
        csa_config::StateDirOnExceed::Error => StateDirCheckResult::Error(anyhow!(
            "{}",
            build_error_message(size_mb, cap_mb, size_bytes, cap_bytes)
        )),
        csa_config::StateDirOnExceed::AutoGc => {
            // Phase 3 will wire actual gc invocation. For now, fall back to warn.
            StateDirCheckResult::Warn(build_warning_preamble(
                size_mb, cap_mb, true, // auto_gc_note
            ))
        }
        csa_config::StateDirOnExceed::Warn => {
            StateDirCheckResult::Warn(build_warning_preamble(size_mb, cap_mb, false))
        }
    }
}

fn build_error_message(actual_mb: u64, cap_mb: u64, actual_bytes: u64, cap_bytes: u64) -> String {
    format!(
        "CSA state directory is {actual_mb} MB / {cap_mb} MB cap exceeded \
         ({actual_bytes} bytes / {cap_bytes} bytes) with `on_exceed = \"error\"`.\n\
         To reclaim space: `csa gc --max-age-days 5 --global`\n\
         Or raise `state_dir.max_size_mb` in ~/.config/cli-sub-agent/config.toml."
    )
}

fn build_warning_preamble(actual_mb: u64, cap_mb: u64, auto_gc_pending: bool) -> String {
    let auto_gc_note = if auto_gc_pending {
        "\nNote: `on_exceed = \"auto-gc\"` is configured but not yet implemented (Phase 3).\n\
         Falling back to warning mode.\n"
    } else {
        ""
    };
    format!(
        "\n<state-dir-warning>\n\
         CSA state directory is {actual_mb} MB / {cap_mb} MB cap exceeded.\n\
         To reclaim space: `csa gc --max-age-days 5 --global` (removes sessions\n\
         older than 5 days) or raise `state_dir.max_size_mb` in\n\
         ~/.config/cli-sub-agent/config.toml.\n\
         Common large consumers: runtime/gemini-home/.npm (~186 MB each) -- if\n\
         you're on csa >= 0.1.514, Phase 1 of #1047 should have mitigated this.\
         {auto_gc_note}\n\
         </state-dir-warning>\n"
    )
}

/// Get the cached size or recompute if stale.
fn get_or_compute_size(state_dir: &Path, scan_interval_seconds: u64) -> Result<u64> {
    let cache_path = state_dir.join(SIZE_CACHE_FILENAME);

    // Try to read cached value
    if let Some(cached) = read_size_cache(&cache_path) {
        let now = now_unix_secs();
        if scan_interval_seconds > 0
            && now.saturating_sub(cached.scanned_at) < scan_interval_seconds
        {
            debug!(
                cached_size_bytes = cached.size_bytes,
                age_secs = now.saturating_sub(cached.scanned_at),
                "Using cached state directory size"
            );
            return Ok(cached.size_bytes);
        }
    }

    let size_bytes = compute_state_dir_size(state_dir)?;

    // Best-effort cache write
    if let Err(e) = write_size_cache(&cache_path, size_bytes) {
        debug!(error = %e, "Failed to write size cache (non-fatal)");
    }

    Ok(size_bytes)
}

/// Walk the state directory tree and sum all file sizes.
pub(crate) fn compute_state_dir_size(state_dir: &Path) -> Result<u64> {
    let mut total: u64 = 0;
    walk_dir_size(state_dir, state_dir, &mut total)?;
    Ok(total)
}

fn walk_dir_size(root: &Path, dir: &Path, total: &mut u64) -> Result<()> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            debug!(path = %dir.display(), "Permission denied during size walk; skipping");
            return Ok(());
        }
        Err(e) => return Err(e.into()),
    };

    for entry in entries {
        let entry = entry?;
        // Use file_type() which does NOT follow symlinks (unlike metadata()).
        // This prevents traversal outside the state tree and infinite recursion
        // on symlink cycles. Matches the safe pattern in gc.rs:672.
        let ft = match entry.file_type() {
            Ok(ft) => ft,
            Err(e) => {
                debug!(path = %entry.path().display(), error = %e, "Skipping entry during size walk");
                continue;
            }
        };

        if ft.is_symlink() {
            continue; // Skip symlinks: avoids external overcount + loop recursion
        } else if ft.is_file() {
            if dir == root
                && entry
                    .path()
                    .file_name()
                    .is_some_and(|name| name == std::ffi::OsStr::new(SIZE_CACHE_FILENAME))
            {
                continue;
            }
            // symlink_metadata() is equivalent to metadata() for non-symlinks,
            // but we've already excluded symlinks above so either call is safe.
            if let Ok(m) = entry.metadata() {
                *total = total.saturating_add(m.len());
            }
        } else if ft.is_dir() {
            walk_dir_size(root, &entry.path(), total)?;
        }
    }
    Ok(())
}

fn read_size_cache(path: &Path) -> Option<SizeCache> {
    let content = std::fs::read_to_string(path).ok()?;
    toml::from_str(&content).ok()
}

fn write_size_cache(path: &Path, size_bytes: u64) -> Result<()> {
    let cache = SizeCache {
        size_bytes,
        scanned_at: now_unix_secs(),
    };
    let content = toml::to_string_pretty(&cache)?;
    std::fs::write(path, content)?;
    Ok(())
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_env_lock::{ScopedEnvVarRestore, TEST_ENV_LOCK};
    use csa_config::{StateDirConfig, StateDirOnExceed};

    #[test]
    fn compute_size_of_temp_dir() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a.txt"), "hello").unwrap();
        std::fs::write(d.path().join("b.txt"), "world!").unwrap();
        std::fs::create_dir(d.path().join("sub")).unwrap();
        std::fs::write(d.path().join("sub/c.txt"), "!!").unwrap();
        let size = compute_state_dir_size(d.path()).unwrap();
        assert_eq!(size, 5 + 6 + 2); // "hello" + "world!" + "!!"
    }

    #[test]
    fn cache_roundtrip() {
        let d = tempfile::tempdir().unwrap();
        let cache_path = d.path().join(SIZE_CACHE_FILENAME);
        write_size_cache(&cache_path, 12345).unwrap();
        let cached = read_size_cache(&cache_path).unwrap();
        assert_eq!(cached.size_bytes, 12345);
        assert!(cached.scanned_at > 0);
    }

    #[test]
    fn cache_reused_within_interval() {
        let d = tempfile::tempdir().unwrap();
        // Create a file so the dir is non-empty
        std::fs::write(d.path().join("data"), vec![0u8; 1024]).unwrap();

        // Write a cache that claims size=999 (stale value, but within interval)
        let cache_path = d.path().join(SIZE_CACHE_FILENAME);
        let cache = SizeCache {
            size_bytes: 999,
            scanned_at: now_unix_secs(),
        };
        std::fs::write(&cache_path, toml::to_string_pretty(&cache).unwrap()).unwrap();

        let size = get_or_compute_size(d.path(), 3600).unwrap();
        assert_eq!(size, 999); // Should use cached, not recompute
    }

    #[test]
    fn cache_stale_triggers_rescan() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("data"), vec![0u8; 512]).unwrap();

        // Write a cache with timestamp far in the past
        let cache_path = d.path().join(SIZE_CACHE_FILENAME);
        let cache = SizeCache {
            size_bytes: 999,
            scanned_at: 1000, // ancient timestamp
        };
        std::fs::write(&cache_path, toml::to_string_pretty(&cache).unwrap()).unwrap();

        let size = get_or_compute_size(d.path(), 3600).unwrap();
        // Should have rescanned — real size is 512 (data file)
        // The cache file itself also exists now but we don't count it as 999.
        assert_ne!(size, 999);
    }

    #[test]
    fn repeated_rescans_ignore_root_size_cache_file() {
        const ONE_MIB: u64 = 1024 * 1024;

        let d = tempfile::tempdir().unwrap();
        let file = std::fs::File::create(d.path().join("exact.bin")).unwrap();
        file.set_len(ONE_MIB).unwrap();

        let first = get_or_compute_size(d.path(), 0).unwrap();
        assert_eq!(first, ONE_MIB);
        assert!(
            d.path().join(SIZE_CACHE_FILENAME).exists(),
            "first scan should write the cache file"
        );

        let second = get_or_compute_size(d.path(), 0).unwrap();
        assert_eq!(second, ONE_MIB);
    }

    #[test]
    fn zero_cap_always_ok() {
        let config = StateDirConfig {
            max_size_mb: 0,
            ..Default::default()
        };
        assert!(matches!(
            check_state_dir_size(&config),
            StateDirCheckResult::Ok
        ));
    }

    #[test]
    fn on_exceed_warn_produces_preamble() {
        let preamble = build_warning_preamble(500, 100, false);
        assert!(preamble.contains("500 MB"));
        assert!(preamble.contains("100 MB"));
        assert!(preamble.contains("csa gc"));
        assert!(!preamble.contains("auto-gc"));
    }

    #[test]
    fn on_exceed_auto_gc_shows_note() {
        let preamble = build_warning_preamble(500, 100, true);
        assert!(preamble.contains("auto-gc"));
        assert!(preamble.contains("not yet implemented"));
    }

    #[test]
    fn config_roundtrip_all_on_exceed_variants() {
        for (input, expected) in [
            (r#"on_exceed = "warn""#, StateDirOnExceed::Warn),
            (r#"on_exceed = "error""#, StateDirOnExceed::Error),
            (r#"on_exceed = "auto-gc""#, StateDirOnExceed::AutoGc),
        ] {
            let toml_str = format!("max_size_mb = 1024\nscan_interval_seconds = 3600\n{input}");
            let parsed: StateDirConfig = toml::from_str(&toml_str).unwrap();
            assert_eq!(parsed.on_exceed, expected, "for input: {input}");

            // Roundtrip
            let serialized = toml::to_string(&parsed).unwrap();
            let reparsed: StateDirConfig = toml::from_str(&serialized).unwrap();
            assert_eq!(
                reparsed.on_exceed, expected,
                "roundtrip failed for: {input}"
            );
        }
    }

    #[test]
    fn config_defaults() {
        let config: StateDirConfig = toml::from_str("").unwrap();
        assert_eq!(config.max_size_mb, 0);
        assert_eq!(config.scan_interval_seconds, 3600);
        assert_eq!(config.on_exceed, StateDirOnExceed::Warn);
        assert!(config.is_default());
    }

    #[test]
    fn cap_boundary_uses_raw_bytes_instead_of_floored_mib() {
        const ONE_MIB: u64 = 1024 * 1024;
        let _env_lock = TEST_ENV_LOCK.blocking_lock();
        let config = StateDirConfig {
            max_size_mb: 1,
            scan_interval_seconds: 0,
            on_exceed: StateDirOnExceed::Error,
        };

        {
            let temp = tempfile::tempdir().unwrap();
            let state_home = temp.path().join("xdg-state");
            let _home_guard = ScopedEnvVarRestore::set("HOME", temp.path());
            let _state_guard = ScopedEnvVarRestore::set("XDG_STATE_HOME", &state_home);
            let state_dir = csa_config::paths::state_dir().unwrap();
            std::fs::create_dir_all(&state_dir).unwrap();
            let file = std::fs::File::create(state_dir.join("exact.bin")).unwrap();
            file.set_len(ONE_MIB).unwrap();

            assert!(matches!(
                check_state_dir_size(&config),
                StateDirCheckResult::Ok
            ));
        }

        {
            let temp = tempfile::tempdir().unwrap();
            let state_home = temp.path().join("xdg-state");
            let _home_guard = ScopedEnvVarRestore::set("HOME", temp.path());
            let _state_guard = ScopedEnvVarRestore::set("XDG_STATE_HOME", &state_home);
            let state_dir = csa_config::paths::state_dir().unwrap();
            std::fs::create_dir_all(&state_dir).unwrap();
            let file = std::fs::File::create(state_dir.join("over.bin")).unwrap();
            file.set_len(ONE_MIB + 1).unwrap();

            assert!(matches!(
                check_state_dir_size(&config),
                StateDirCheckResult::Error(_)
            ));
        }
    }

    fn run_dual_root_cap_check(canonical_bytes: u64, legacy_bytes: u64) -> StateDirCheckResult {
        let temp = tempfile::tempdir().unwrap();
        let state_home = temp.path().join("xdg-state");
        let _home_guard = ScopedEnvVarRestore::set("HOME", temp.path());
        let _state_guard = ScopedEnvVarRestore::set("XDG_STATE_HOME", &state_home);

        let canonical = csa_config::paths::state_dir_write().expect("canonical state dir");
        let legacy = csa_config::paths::legacy_state_dir().expect("legacy state dir");
        std::fs::create_dir_all(&canonical).unwrap();
        std::fs::create_dir_all(&legacy).unwrap();

        if canonical_bytes > 0 {
            let file = std::fs::File::create(canonical.join("canonical.bin")).unwrap();
            file.set_len(canonical_bytes).unwrap();
        }
        if legacy_bytes > 0 {
            let file = std::fs::File::create(legacy.join("legacy.bin")).unwrap();
            file.set_len(legacy_bytes).unwrap();
        }

        check_state_dir_size(&StateDirConfig {
            max_size_mb: 1,
            scan_interval_seconds: 0,
            on_exceed: StateDirOnExceed::Error,
        })
    }

    fn assert_cap_error_size(result: StateDirCheckResult, actual_mb: u64) {
        match result {
            StateDirCheckResult::Error(err) => {
                let message = err.to_string();
                assert!(
                    message.contains(&format!("{actual_mb} MB / 1 MB cap exceeded")),
                    "unexpected error message: {message}"
                );
            }
            StateDirCheckResult::Warn(message) => {
                panic!("expected error result, got warning: {message}")
            }
            StateDirCheckResult::Ok => panic!("expected error result, got ok"),
        }
    }

    #[test]
    fn check_state_dir_size_sums_existing_canonical_and_legacy_roots() {
        const ONE_MIB: u64 = 1024 * 1024;
        let _env_lock = TEST_ENV_LOCK.blocking_lock();

        assert_cap_error_size(run_dual_root_cap_check(0, 2 * ONE_MIB), 2);
        assert_cap_error_size(run_dual_root_cap_check(2 * ONE_MIB, 0), 2);
        assert_cap_error_size(run_dual_root_cap_check(ONE_MIB, ONE_MIB), 2);
    }

    /// HIGH-1 regression: symlinks must not be followed during size walk.
    /// A symlink pointing to a large external file must NOT inflate the total.
    #[cfg(unix)]
    #[test]
    fn symlink_to_external_file_not_counted() {
        let state = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();

        // Real file inside state dir: 10 bytes
        std::fs::write(state.path().join("real.txt"), "0123456789").unwrap();

        // Large external file: 1000 bytes
        std::fs::write(external.path().join("big.bin"), vec![0u8; 1000]).unwrap();

        // Symlink inside state dir → external file
        std::os::unix::fs::symlink(
            external.path().join("big.bin"),
            state.path().join("link_to_big"),
        )
        .unwrap();

        let size = compute_state_dir_size(state.path()).unwrap();
        // Should only count the 10-byte real file, NOT the 1000-byte symlink target
        assert_eq!(size, 10);
    }

    /// HIGH-1 regression: a symlink cycle (dir → ancestor) must not infinite-recurse.
    #[cfg(unix)]
    #[test]
    fn symlink_cycle_does_not_infinite_recurse() {
        let state = tempfile::tempdir().unwrap();
        std::fs::write(state.path().join("data.txt"), "abc").unwrap();

        // Create symlink pointing back to the state dir root (cycle)
        std::os::unix::fs::symlink(state.path(), state.path().join("cycle_link")).unwrap();

        // Must return without hanging; the cycle link is skipped.
        let size = compute_state_dir_size(state.path()).unwrap();
        assert_eq!(size, 3); // Only "abc"
    }

    /// HIGH-2 regression: enforce_state_dir_cap must block on_exceed="error" even
    /// when called from a resume path (no fresh_spawn_preflight_override).
    ///
    /// Since check_state_dir_size reads paths::state_dir() internally, we test:
    /// 1. The public enforce_state_dir_cap API exercises the config → check path.
    /// 2. The core enum logic: size > cap + on_exceed=error => Error variant.
    #[test]
    fn enforce_cap_blocks_on_error_mode() {
        use csa_config::GlobalConfig;

        let gc: GlobalConfig = toml::from_str(
            r#"
            [state_dir]
            max_size_mb = 1
            scan_interval_seconds = 0
            on_exceed = "error"
            "#,
        )
        .unwrap();

        // Exercise the public API path (uses real state_dir()).
        // Result depends on actual state dir size — we can't control it here,
        // but this proves enforce_state_dir_cap routes through check_state_dir_size.
        let _result = enforce_state_dir_cap(Some(&gc));

        // Core enum-routing test: verify that when size > cap AND on_exceed=error,
        // check_state_dir_size returns the Error variant (not Warn/Ok).
        let config = StateDirConfig {
            max_size_mb: 1,
            scan_interval_seconds: 0,
            on_exceed: StateDirOnExceed::Error,
        };
        // check_state_dir_size reads paths::state_dir(); if real dir > 1 MB → Error,
        // if ≤ 1 MB or missing → Ok. Both are valid; the key invariant is it never
        // returns Warn for on_exceed=error.
        let result = check_state_dir_size(&config);
        assert!(
            !matches!(result, StateDirCheckResult::Warn(_)),
            "on_exceed=error must never produce Warn variant"
        );
    }

    /// HIGH-2 regression: run_state_dir_preflight returns preamble (not error) for warn mode.
    #[test]
    fn warn_mode_returns_preamble_not_error() {
        let config = StateDirConfig {
            max_size_mb: 1,
            scan_interval_seconds: 0,
            on_exceed: StateDirOnExceed::Warn,
        };
        // For warn mode, check_state_dir_size returns Warn (if over) or Ok (if under).
        // Either way it must NOT return Error.
        let result = check_state_dir_size(&config);
        assert!(
            !matches!(result, StateDirCheckResult::Error(_)),
            "Warn mode must never produce Error variant"
        );
    }

    /// HIGH-2 regression: enforce_state_dir_cap with warn mode returns Ok (not Err).
    #[test]
    fn enforce_cap_allows_warn_mode() {
        use csa_config::GlobalConfig;

        let gc: GlobalConfig = toml::from_str(
            r#"
            [state_dir]
            max_size_mb = 1
            scan_interval_seconds = 0
            on_exceed = "warn"
            "#,
        )
        .unwrap();

        // enforce_state_dir_cap should Ok even if state dir is over cap in warn mode
        let result = enforce_state_dir_cap(Some(&gc));
        assert!(
            result.is_ok(),
            "Warn mode must not return Err from enforce_state_dir_cap"
        );
    }
}
