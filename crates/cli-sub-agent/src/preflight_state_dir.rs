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

/// Run state-dir preflight from global config. Returns `Ok(Some(preamble))` for
/// warning injection, `Ok(None)` when within cap or unconfigured, `Err` on block.
pub(crate) fn run_state_dir_preflight(
    global_config: Option<&csa_config::GlobalConfig>,
) -> anyhow::Result<Option<String>> {
    let config = match global_config {
        Some(gc) if gc.state_dir.max_size_mb > 0 => &gc.state_dir,
        _ => return Ok(None),
    };
    match check_state_dir_size(config) {
        StateDirCheckResult::Ok => Ok(None),
        StateDirCheckResult::Warn(preamble) => Ok(Some(preamble)),
        StateDirCheckResult::Error(err) => Err(err),
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

    let state_dir = match csa_config::paths::state_dir() {
        Some(d) => d,
        None => {
            debug!("State directory not resolvable; skipping size check");
            return StateDirCheckResult::Ok;
        }
    };

    if !state_dir.exists() {
        return StateDirCheckResult::Ok;
    }

    let size_bytes = match get_or_compute_size(&state_dir, config.scan_interval_seconds) {
        Ok(size) => size,
        Err(e) => {
            warn!(error = %e, "Failed to compute state directory size; skipping check");
            return StateDirCheckResult::Ok;
        }
    };

    let size_mb = size_bytes / (1024 * 1024);
    let cap_mb = config.max_size_mb;

    if size_mb <= cap_mb {
        debug!(size_mb, cap_mb, "State directory within cap");
        return StateDirCheckResult::Ok;
    }

    info!(size_mb, cap_mb, on_exceed = ?config.on_exceed, "State directory exceeds cap");

    match config.on_exceed {
        csa_config::StateDirOnExceed::Error => StateDirCheckResult::Error(anyhow!(
            "State directory is {} MB / {} MB cap exceeded.\n\
                 To reclaim space: `csa gc --max-age-days 5 --global`\n\
                 Or raise `state_dir.max_size_mb` in ~/.config/cli-sub-agent/config.toml.",
            size_mb,
            cap_mb,
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
    walk_dir_size(state_dir, &mut total)?;
    Ok(total)
}

fn walk_dir_size(dir: &Path, total: &mut u64) -> Result<()> {
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
        let metadata = match entry.metadata() {
            Ok(m) => m,
            Err(e) => {
                debug!(path = %entry.path().display(), error = %e, "Skipping entry during size walk");
                continue;
            }
        };

        if metadata.is_file() {
            *total = total.saturating_add(metadata.len());
        } else if metadata.is_dir() {
            walk_dir_size(&entry.path(), total)?;
        }
        // Skip symlinks to avoid double-counting
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
}
