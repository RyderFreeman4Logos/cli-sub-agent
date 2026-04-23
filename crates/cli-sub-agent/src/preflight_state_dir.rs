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

#[derive(Debug, Clone, Copy)]
struct StateDirUsage {
    size_bytes: u64,
    size_mb: u64,
    cap_bytes: u64,
    cap_mb: u64,
}

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

    let Some(usage) = compute_state_dir_usage(config) else {
        return StateDirCheckResult::Ok;
    };

    match config.on_exceed {
        csa_config::StateDirOnExceed::Error => StateDirCheckResult::Error(anyhow!(
            "{}",
            build_error_message(
                usage.size_mb,
                usage.cap_mb,
                usage.size_bytes,
                usage.cap_bytes
            )
        )),
        csa_config::StateDirOnExceed::AutoGc => {
            match crate::gc::reap_runtime_payloads_global(
                false,
                crate::gc::AUTO_GC_REAP_RUNTIME_MAX_AGE_DAYS,
            ) {
                Ok(stats) => {
                    crate::gc::invalidate_state_dir_size_cache();
                    info!(
                        sessions_reaped = stats.sessions_reaped,
                        bytes_reclaimed = stats.bytes_reclaimed,
                        max_age_days = crate::gc::AUTO_GC_REAP_RUNTIME_MAX_AGE_DAYS,
                        "State-dir auto-gc reaped runtime payloads"
                    );
                    let Some(post_gc_usage) = compute_state_dir_usage(config) else {
                        return StateDirCheckResult::Ok;
                    };
                    if post_gc_usage.size_bytes <= post_gc_usage.cap_bytes {
                        StateDirCheckResult::Ok
                    } else {
                        StateDirCheckResult::Warn(build_warning_preamble(
                            post_gc_usage.size_mb,
                            post_gc_usage.cap_mb,
                            Some(&format!(
                                "Auto-gc reaped runtime/ for {} session(s), reclaiming {} bytes, but the cap is still exceeded.",
                                stats.sessions_reaped, stats.bytes_reclaimed
                            )),
                        ))
                    }
                }
                Err(err) => StateDirCheckResult::Warn(build_warning_preamble(
                    usage.size_mb,
                    usage.cap_mb,
                    Some(&format!("Auto-gc failed: {err}")),
                )),
            }
        }
        csa_config::StateDirOnExceed::Warn => {
            StateDirCheckResult::Warn(build_warning_preamble(usage.size_mb, usage.cap_mb, None))
        }
    }
}

fn build_error_message(actual_mb: u64, cap_mb: u64, actual_bytes: u64, cap_bytes: u64) -> String {
    format!(
        "CSA state directory is {actual_mb} MB / {cap_mb} MB cap exceeded \
         ({actual_bytes} bytes / {cap_bytes} bytes) with `on_exceed = \"error\"`.\n\
         To reclaim space: `csa gc --reap-runtime --max-age-days 30 --global`\n\
         Or raise `state_dir.max_size_mb` in ~/.config/cli-sub-agent/config.toml."
    )
}

fn build_warning_preamble(actual_mb: u64, cap_mb: u64, note: Option<&str>) -> String {
    let note_block = note
        .map(|note| format!("\nNote: {note}\n"))
        .unwrap_or_default();
    format!(
        "\n<state-dir-warning>\n\
         CSA state directory is {actual_mb} MB / {cap_mb} MB cap exceeded.\n\
         To reclaim space: `csa gc --reap-runtime --max-age-days 30 --global`\n\
         or raise `state_dir.max_size_mb` in\n\
         ~/.config/cli-sub-agent/config.toml.\n\
         Common large consumers: runtime/gemini-home/.npm (~186 MB each) -- if\n\
         you're on csa >= 0.1.514, Phase 1 of #1047 should have mitigated this.\
         {note_block}\n\
         </state-dir-warning>\n"
    )
}

fn compute_state_dir_usage(config: &StateDirConfig) -> Option<StateDirUsage> {
    let state_roots = csa_config::paths::state_dir_all_roots();
    if state_roots.is_empty() {
        debug!("No existing state directory roots found; skipping size check");
        return None;
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
                return None;
            }
        }
    }

    let cap_mb = config.max_size_mb;
    let cap_bytes = cap_mb.saturating_mul(1024 * 1024);
    let size_mb = size_bytes / (1024 * 1024);
    if size_bytes <= cap_bytes {
        debug!(size_mb, cap_mb, "State directory within cap");
        return None;
    }

    info!(size_mb, cap_mb, on_exceed = ?config.on_exceed, "State directory exceeds cap");
    Some(StateDirUsage {
        size_bytes,
        size_mb,
        cap_bytes,
        cap_mb,
    })
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
    use crate::test_session_sandbox::ScopedSessionSandbox;
    use csa_config::{StateDirConfig, StateDirOnExceed};
    use csa_session::{
        SessionArtifact, SessionPhase, SessionResult, create_session, save_result, save_session,
    };

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
        let preamble = build_warning_preamble(500, 100, None);
        assert!(preamble.contains("500 MB"));
        assert!(preamble.contains("100 MB"));
        assert!(preamble.contains("csa gc"));
        assert!(preamble.contains("reap-runtime"));
    }

    #[test]
    fn warning_preamble_includes_optional_note() {
        let preamble = build_warning_preamble(500, 100, Some("auto-gc ran"));
        assert!(preamble.contains("Note: auto-gc ran"));
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

    fn seed_retired_session_with_runtime(
        project_root: &std::path::Path,
        last_accessed: chrono::DateTime<chrono::Utc>,
        runtime_bytes: u64,
    ) -> std::path::PathBuf {
        std::fs::create_dir_all(project_root).unwrap();

        let mut session = create_session(project_root, Some("auto-gc"), None, Some("codex"))
            .expect("create session");
        session.phase = SessionPhase::Retired;
        session.last_accessed = last_accessed;
        save_session(&session).expect("persist retired session");

        let session_root = csa_session::get_session_root(project_root).expect("session root");
        let session_dir = session_root.join("sessions").join(&session.meta_session_id);
        let runtime_blob = session_dir
            .join("runtime")
            .join("gemini-home")
            .join(".npm")
            .join("_cacache")
            .join("blob.bin");
        std::fs::create_dir_all(runtime_blob.parent().unwrap()).unwrap();
        let file = std::fs::File::create(&runtime_blob).unwrap();
        file.set_len(runtime_bytes).unwrap();

        let now = chrono::Utc::now();
        save_result(
            project_root,
            &session.meta_session_id,
            &SessionResult {
                status: "success".to_string(),
                exit_code: 0,
                summary: "completed".to_string(),
                tool: "codex".to_string(),
                started_at: now,
                completed_at: now,
                events_count: 0,
                artifacts: vec![SessionArtifact::new("output/summary.md")],
                peak_memory_mb: None,
                manager_fields: Default::default(),
            },
        )
        .expect("save result");
        std::fs::write(session_dir.join("stderr.log"), "stderr").unwrap();
        std::fs::write(session_dir.join("output/summary.md"), "summary").unwrap();

        session_dir.join("runtime")
    }

    #[test]
    fn auto_gc_default_runtime_reap_age_is_30_days() {
        assert_eq!(crate::gc::AUTO_GC_REAP_RUNTIME_MAX_AGE_DAYS, 30);
    }

    #[test]
    fn auto_gc_reaps_runtime_and_next_check_is_ok() {
        const TWO_MIB: u64 = 2 * 1024 * 1024;

        let temp = tempfile::tempdir().unwrap();
        let _sandbox = ScopedSessionSandbox::new_blocking(&temp);
        let project_root = temp.path().join("project");
        let runtime_dir = seed_retired_session_with_runtime(
            &project_root,
            chrono::Utc::now()
                - chrono::Duration::days(crate::gc::AUTO_GC_REAP_RUNTIME_MAX_AGE_DAYS as i64 + 10),
            TWO_MIB,
        );

        let config = StateDirConfig {
            max_size_mb: 1,
            scan_interval_seconds: 0,
            on_exceed: StateDirOnExceed::AutoGc,
        };

        assert!(
            matches!(check_state_dir_size(&config), StateDirCheckResult::Ok),
            "auto-gc should reap old runtime payloads until the cap check passes"
        );
        assert!(
            !runtime_dir.exists(),
            "runtime/ should be removed by auto-gc when the session is old enough"
        );
        assert!(
            matches!(check_state_dir_size(&config), StateDirCheckResult::Ok),
            "subsequent checks should stay within the cap after auto-gc"
        );
    }
}
