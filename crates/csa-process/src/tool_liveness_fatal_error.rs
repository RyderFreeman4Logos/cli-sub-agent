use regex::{Regex, RegexBuilder};
use std::fs::{self, File};
use std::io::{ErrorKind, Read, Seek, SeekFrom};
use std::path::Path;
use std::process::Command;
use std::sync::{
    OnceLock,
    atomic::{AtomicBool, Ordering},
};

use super::ProviderErrorKind;
use super::{FATAL_ERROR_MARKERS_FILE, OUTPUT_LOG_FILE, STDERR_LOG_FILE};

const FATAL_ERROR_TAIL_BYTES: u64 = 4096;

#[derive(Clone)]
struct FatalErrorRegexes {
    permanent: FatalErrorChannelRegexes,
    transient: FatalErrorChannelRegexes,
}

#[derive(Clone)]
struct FatalErrorChannelRegexes {
    all_channel: Option<Regex>,
    stderr_only: Option<Regex>,
}

impl FatalErrorRegexes {
    fn from_markers(markers: &[String]) -> Self {
        let (transient, permanent): (Vec<_>, Vec<_>) = markers
            .iter()
            .cloned()
            .partition(|marker| is_transient_provider_marker(marker));
        Self {
            permanent: FatalErrorChannelRegexes::from_markers(&permanent),
            transient: FatalErrorChannelRegexes::from_markers(&transient),
        }
    }
}

impl FatalErrorChannelRegexes {
    fn from_markers(markers: &[String]) -> Self {
        let (stderr_only, all_channel): (Vec<_>, Vec<_>) = markers
            .iter()
            .cloned()
            .partition(|marker| is_broad_http_marker(marker));
        // Broad HTTP/status markers are matched EXACTLY (same word-boundary logic as
        // all-channel markers); only the CHANNEL differs (stderr-only), not the matching
        // precision. A configured code like "HTTP 404" must never match an unrelated or
        // non-fatal code such as "HTTP 200"/"HTTP 301" (#1652 round-5 false-positive).
        Self {
            all_channel: build_fatal_error_regex(&all_channel),
            stderr_only: build_fatal_error_regex(&stderr_only),
        }
    }
}

/// Classify a marker as a "broad" HTTP/status reference (e.g. "HTTP 404",
/// "status 500", "404 Not Found"). Broad markers are scoped to stderr only; they are
/// still matched EXACTLY (by the caller via `build_fatal_error_regex`), so the specific
/// status code is preserved — this is purely a channel classifier, not a pattern source.
fn is_broad_http_marker(marker: &str) -> bool {
    marker_http_status_code(marker).is_some()
}

fn marker_http_status_code(marker: &str) -> Option<u16> {
    let mut words = marker.split_whitespace();
    let (Some(first), Some(second)) = (words.next(), words.next()) else {
        return None;
    };
    if first.eq_ignore_ascii_case("http") || first.eq_ignore_ascii_case("status") {
        return parse_three_digit_status_code(second);
    }
    if second.chars().next().is_some_and(char::is_alphabetic) {
        return parse_three_digit_status_code(first);
    }
    None
}

fn parse_three_digit_status_code(value: &str) -> Option<u16> {
    if value.len() != 3 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    value.parse().ok()
}

fn is_transient_provider_marker(marker: &str) -> bool {
    let normalized = marker.trim().to_ascii_lowercase();
    matches!(
        normalized.as_str(),
        "rate_limit_exceeded" | "rate limit exceeded" | "overloaded_error"
    ) || marker_http_status_code(marker)
        .is_some_and(|status| matches!(status, 408 | 429 | 500 | 502 | 503 | 504))
}

fn default_tier1_fatal_error_markers() -> Vec<String> {
    const MARKERS: &str = "\
rate_limit_exceeded
insufficient_quota
insufficient quota
quota exceeded
QUOTA_EXHAUSTED
TerminalQuotaError
overloaded_error
invalid_api_key
API key not found
rate limit exceeded";
    MARKERS.lines().map(str::to_string).collect()
}

fn default_tier2_http_fatal_error_markers() -> Vec<String> {
    // Enumerate the fatal HTTP status codes explicitly. Markers are matched EXACTLY, so each
    // code only fast-fails when that specific code appears on stderr — non-fatal codes
    // (1xx/2xx/3xx) and uncatalogued codes never trip the watchdog.
    const STATUSES: &[(&str, &str)] = &[
        ("400", "Bad Request"),
        ("401", "Unauthorized"),
        ("403", "Forbidden"),
        ("404", "Not Found"),
        ("408", "Request Timeout"),
        ("409", "Conflict"),
        ("429", "Too Many Requests"),
        ("500", "Internal Server Error"),
        ("502", "Bad Gateway"),
        ("503", "Service Unavailable"),
        ("504", "Gateway Timeout"),
    ];

    let mut markers = Vec::with_capacity(STATUSES.len() * 3);
    for (code, name) in STATUSES {
        markers.push(format!("HTTP {code}"));
        markers.push(format!("status {code}"));
        markers.push(format!("{code} {name}"));
    }
    markers
}

fn read_fatal_error_marker_file(marker_path: &Path) -> Vec<String> {
    fs::read_to_string(marker_path)
        .ok()
        .map(|content| {
            content
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

pub(super) fn provider_error_signal(session_dir: &Path) -> Option<ProviderErrorKind> {
    let tmux_pane = capture_tmux_pane(session_dir);
    provider_error_signal_in_channels(session_dir, tmux_pane.as_deref())
}

#[cfg(test)]
pub(super) fn has_fatal_error_signal_in_channels(
    session_dir: &Path,
    tmux_pane: Option<&str>,
) -> bool {
    provider_error_signal_in_channels(session_dir, tmux_pane).is_some()
}

fn provider_error_signal_in_channels(
    session_dir: &Path,
    tmux_pane: Option<&str>,
) -> Option<ProviderErrorKind> {
    let regexes = fatal_error_regexes_for_session(session_dir);
    let stderr_tail = read_file_tail(&session_dir.join(STDERR_LOG_FILE)).ok();
    let output_tail = read_file_tail(&session_dir.join(OUTPUT_LOG_FILE)).ok();

    if matches_provider_error(
        &regexes.permanent,
        stderr_tail.as_deref(),
        output_tail.as_deref(),
        tmux_pane,
    ) {
        return Some(ProviderErrorKind::Permanent);
    }
    matches_provider_error(
        &regexes.transient,
        stderr_tail.as_deref(),
        output_tail.as_deref(),
        tmux_pane,
    )
    .then_some(ProviderErrorKind::Transient)
}

fn matches_provider_error(
    regexes: &FatalErrorChannelRegexes,
    stderr_tail: Option<&str>,
    output_tail: Option<&str>,
    tmux_pane: Option<&str>,
) -> bool {
    stderr_tail.is_some_and(|tail| {
        matches_fatal_error(&regexes.all_channel, tail)
            || matches_fatal_error(&regexes.stderr_only, tail)
    }) || output_tail.is_some_and(|tail| matches_fatal_error(&regexes.all_channel, tail))
        || tmux_pane.is_some_and(|pane| matches_fatal_error(&regexes.all_channel, pane))
}

fn matches_fatal_error(regex: &Option<Regex>, text: &str) -> bool {
    regex.as_ref().is_some_and(|regex| regex.is_match(text))
}

fn fatal_error_regexes_for_session(session_dir: &Path) -> FatalErrorRegexes {
    let marker_path = session_dir.join(FATAL_ERROR_MARKERS_FILE);
    if marker_path.exists() {
        // Sidecar markers and built-in defaults use identical content-based
        // channel routing so config defaults cannot bypass stderr scoping.
        return FatalErrorRegexes::from_markers(&read_fatal_error_marker_file(&marker_path));
    }
    default_fatal_error_regexes()
}

fn default_fatal_error_regexes() -> FatalErrorRegexes {
    static DEFAULT_REGEXES: OnceLock<FatalErrorRegexes> = OnceLock::new();
    DEFAULT_REGEXES
        .get_or_init(|| {
            let mut markers = default_tier1_fatal_error_markers();
            markers.extend(default_tier2_http_fatal_error_markers());
            FatalErrorRegexes::from_markers(&markers)
        })
        .clone()
}

pub(super) fn build_fatal_error_regex(markers: &[String]) -> Option<Regex> {
    let alternatives = markers
        .iter()
        .map(|marker| marker.trim())
        .filter(|marker| !marker.is_empty())
        .map(|marker| {
            let boundary = |ch: Option<char>| {
                if ch.is_some_and(|ch| ch == '_' || ch.is_alphanumeric()) {
                    r"\b"
                } else {
                    ""
                }
            };
            format!(
                "{}{}{}",
                boundary(marker.chars().next()),
                regex::escape(marker),
                boundary(marker.chars().next_back())
            )
        })
        .collect::<Vec<_>>();
    if alternatives.is_empty() {
        return None;
    }
    let pattern = format!("(?:{})", alternatives.join("|"));
    RegexBuilder::new(&pattern)
        .case_insensitive(true)
        .build()
        .ok()
}

fn read_file_tail(path: &Path) -> std::io::Result<String> {
    let mut file = File::open(path)?;
    let file_len = file.metadata()?.len();
    let tail_len = file_len.min(FATAL_ERROR_TAIL_BYTES);
    file.seek(SeekFrom::Start(file_len.saturating_sub(tail_len)))?;

    let mut buf = Vec::with_capacity(tail_len as usize);
    file.take(tail_len).read_to_end(&mut buf)?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

fn capture_tmux_pane(session_dir: &Path) -> Option<String> {
    static TMUX_AVAILABLE: AtomicBool = AtomicBool::new(true);
    if !TMUX_AVAILABLE.load(Ordering::Relaxed) {
        return None;
    }

    let session_id = session_dir.file_name()?.to_str()?;
    let session_name = format!("csa-{session_id}");

    if let Ok(handle) = tokio::runtime::Handle::try_current()
        && matches!(
            handle.runtime_flavor(),
            tokio::runtime::RuntimeFlavor::MultiThread
        )
    {
        return tokio::task::block_in_place(|| {
            capture_tmux_pane_blocking(&session_name, &TMUX_AVAILABLE)
        });
    }

    capture_tmux_pane_blocking(&session_name, &TMUX_AVAILABLE)
}

fn capture_tmux_pane_blocking(session_name: &str, tmux_available: &AtomicBool) -> Option<String> {
    let output = match Command::new("tmux")
        .args(["capture-pane", "-t", session_name, "-p", "-S", "-200"])
        .output()
    {
        Ok(output) => output,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            tmux_available.store(false, Ordering::Relaxed);
            return None;
        }
        Err(_) => return None,
    };
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}
