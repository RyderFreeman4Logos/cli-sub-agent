//! Default gating for the #1652 fatal-error-marker silent-hang scan (#1847).
//!
//! The scan classifies a session as a silent hang when its tool output contains
//! a configured fatal-error marker. Codex multiplexes provider-error text into
//! the tool-command output stream (#1738), so a codex-fallback step inside a
//! weave pattern can trip the scan and SELF-KILL — with `on_fail = "abort"` that
//! aborts the whole `csa plan run` (#1830/#1847).
//!
//! This module owns ONLY the DEFAULT gating decision (whether the scan is on for
//! a session); it does not touch the marker-detection logic itself. The
//! precedence is: explicit CLI flag > `CSA_PATTERN_INTERNAL` marker default >
//! config `[resources].error_marker_scan` default.

/// Collapse the paired `--error-marker-scan` / `--no-error-marker-scan` CLI
/// flags into a tri-state explicit override.
///
/// The two flags are mutually exclusive at the clap layer (`conflicts_with`);
/// should both ever arrive, the enable flag wins (fail-open toward keeping the
/// safety scan on). `None` means the caller passed neither flag, deferring to
/// the marker/config default.
pub(crate) fn override_from_flags(
    error_marker_scan: bool,
    no_error_marker_scan: bool,
) -> Option<bool> {
    match (error_marker_scan, no_error_marker_scan) {
        (true, _) => Some(true),
        (_, true) => Some(false),
        (false, false) => None,
    }
}

/// Resolve whether the #1652 fatal-error-marker scan runs for this session.
///
/// Precedence (highest first):
///   1. `override_enabled` — an explicit `--error-marker-scan` (`Some(true)`) or
///      `--no-error-marker-scan` (`Some(false)`) CLI flag ALWAYS wins;
///   2. `pattern_internal` — when the `CSA_PATTERN_INTERNAL` marker is set, the
///      scan DEFAULTS OFF so a codex-fallback pattern-internal step cannot
///      self-kill the pipeline (#1847);
///   3. `config_error_marker_scan` — config `[resources].error_marker_scan`
///      (absent ⇒ `true`) when neither an explicit flag nor the marker applies.
///
/// Only the marker-based fatal classification is gated; each step's
/// `--idle-timeout` and the wall-clock `--timeout` still apply, so a genuine
/// hang is still caught (it just exits via timeout instead of the early scan).
pub(crate) fn resolve_error_marker_scan_enabled(
    override_enabled: Option<bool>,
    pattern_internal: bool,
    config_error_marker_scan: Option<bool>,
) -> bool {
    match override_enabled {
        Some(enabled) => enabled,
        None if pattern_internal => false,
        None => config_error_marker_scan.unwrap_or(true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn override_from_flags_maps_each_combination() {
        assert_eq!(override_from_flags(false, false), None);
        assert_eq!(override_from_flags(true, false), Some(true));
        assert_eq!(override_from_flags(false, true), Some(false));
        // Defensive: clap `conflicts_with` prevents both, but enable wins.
        assert_eq!(override_from_flags(true, true), Some(true));
    }

    #[test]
    fn marker_absent_keeps_config_default_enabled() {
        // (iii) Interactive caller, no marker, no flag: scan stays ON by default.
        assert!(resolve_error_marker_scan_enabled(None, false, None));
        assert!(resolve_error_marker_scan_enabled(None, false, Some(true)));
        // Config explicitly off is still honored when no marker/flag.
        assert!(!resolve_error_marker_scan_enabled(None, false, Some(false)));
    }

    #[test]
    fn marker_present_defaults_scan_off() {
        // (ii) Pattern-internal session with no explicit flag: scan defaults OFF,
        // even when config default is on.
        assert!(!resolve_error_marker_scan_enabled(None, true, None));
        assert!(!resolve_error_marker_scan_enabled(None, true, Some(true)));
    }

    #[test]
    fn explicit_flag_overrides_marker_both_ways() {
        // (iv) Explicit disable forces OFF regardless of marker/config.
        assert!(!resolve_error_marker_scan_enabled(
            Some(false),
            false,
            Some(true)
        ));
        assert!(!resolve_error_marker_scan_enabled(
            Some(false),
            true,
            Some(true)
        ));
        // Explicit enable forces ON even when the marker would default it OFF.
        assert!(resolve_error_marker_scan_enabled(
            Some(true),
            true,
            Some(false)
        ));
        assert!(resolve_error_marker_scan_enabled(
            Some(true),
            false,
            Some(false)
        ));
    }
}
