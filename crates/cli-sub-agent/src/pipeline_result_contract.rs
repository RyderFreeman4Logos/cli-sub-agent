//! Result.toml path contract enforcement for session execution.
//!
//! When a prompt contains the contract marker `csa_result_toml_path_contract=1`,
//! the session output must contain the absolute path to a valid `result.toml`
//! artifact inside the session directory. Accepted paths are the runtime
//! session root (`result.toml`), the canonical output-sidecar
//! (`output/result.toml`), or the legacy sidecar (`output/user-result.toml`).
//! If the contract is violated the execution result is coerced to failure.

use std::path::Path;
use tracing::warn;

use csa_process::ExecutionResult;

pub(crate) const RESULT_TOML_PATH_CONTRACT_MARKER: &str = "csa_result_toml_path_contract=1";

pub(crate) fn enforce_result_toml_path_contract(
    prompt: &str,
    _effective_prompt: &str,
    session_dir: &Path,
    result_file_cleared: bool,
    result: &mut ExecutionResult,
) {
    if result.exit_code != 0 || !prompt_requires_result_toml_path(prompt) {
        return;
    }

    if !result_file_cleared {
        let expected_path = session_dir.join("result.toml");
        let legacy_output_path = csa_session::legacy_user_result_path(session_dir);
        let reason = format!(
            "contract violation: failed to clear pre-existing result artifacts '{}', '{}', and '{}' before execution; refusing to trust stale files",
            expected_path.display(),
            csa_session::contract_result_path(session_dir).display(),
            legacy_output_path.display()
        );
        warn!(
            summary = %result.summary,
            "Session output violated result.toml path contract after pre-clear failure; coercing run to failure"
        );
        if !result.stderr_output.is_empty() && !result.stderr_output.ends_with('\n') {
            result.stderr_output.push('\n');
        }
        result.stderr_output.push_str(&reason);
        result.stderr_output.push('\n');
        result.summary = reason;
        result.exit_code = 1;
        return;
    }

    let path_candidate = contract_result_toml_path_candidate(result);
    let expected_path = session_dir.join("result.toml");
    let expected_contract_output_path = csa_session::contract_result_path(session_dir);
    let expected_user_result_path = csa_session::legacy_user_result_path(session_dir);
    if path_matches_expected_contract_result(path_candidate, &expected_path)
        || path_matches_expected_contract_result(path_candidate, &expected_contract_output_path)
        || path_matches_expected_contract_result(path_candidate, &expected_user_result_path)
    {
        return;
    }

    if sidecar_result_fallback_is_valid(&expected_contract_output_path) {
        let warning = format!(
            "contract warning: output/summary path mismatch; accepted fallback artifact '{}'",
            expected_contract_output_path.display()
        );
        warn!(
            summary = %result.summary,
            fallback = %expected_contract_output_path.display(),
            "Session output path did not match contract; accepting verified output/result.toml fallback"
        );
        if !result.stderr_output.is_empty() && !result.stderr_output.ends_with('\n') {
            result.stderr_output.push('\n');
        }
        result.stderr_output.push_str(&warning);
        result.stderr_output.push('\n');
        return;
    }

    if sidecar_result_fallback_is_valid(&expected_user_result_path) {
        let warning = format!(
            "contract warning: output/summary path mismatch; accepted fallback artifact '{}'",
            expected_user_result_path.display()
        );
        warn!(
            summary = %result.summary,
            fallback = %expected_user_result_path.display(),
            "Session output path did not match contract; accepting verified output/user-result.toml fallback"
        );
        if !result.stderr_output.is_empty() && !result.stderr_output.ends_with('\n') {
            result.stderr_output.push('\n');
        }
        result.stderr_output.push_str(&warning);
        result.stderr_output.push('\n');
        return;
    }

    // Disk-based fallback: the agent wrote result.toml to session_dir but the
    // path was not found in output/summary (e.g. verbose output truncated the
    // path, or ACP message boundaries split it). Accept the file if it exists,
    // passes validation, and contains valid TOML.
    if session_result_fallback_is_valid(&expected_path) {
        let warning = format!(
            "contract warning: output/summary path not found; accepted verified session result '{}'",
            expected_path.display()
        );
        warn!(
            summary = %result.summary,
            fallback = %expected_path.display(),
            "Session output path not in output/summary; accepting verified session-dir result.toml fallback"
        );
        if !result.stderr_output.is_empty() && !result.stderr_output.ends_with('\n') {
            result.stderr_output.push('\n');
        }
        result.stderr_output.push_str(&warning);
        result.stderr_output.push('\n');
        return;
    }

    let reason = if path_candidate.is_empty() {
        format!(
            "contract violation: expected existing absolute result path '{}', '{}', or '{}' in output/summary, but output and summary were empty",
            expected_path.display(),
            expected_contract_output_path.display(),
            expected_user_result_path.display()
        )
    } else {
        format!(
            "contract violation: expected existing absolute result path '{}', '{}', or '{}' in output/summary, got '{path_candidate}'",
            expected_path.display(),
            expected_contract_output_path.display(),
            expected_user_result_path.display()
        )
    };

    warn!(
        summary = %result.summary,
        "Session output violated result.toml path contract; coercing run to failure"
    );
    if !result.stderr_output.is_empty() && !result.stderr_output.ends_with('\n') {
        result.stderr_output.push('\n');
    }
    result.stderr_output.push_str(&reason);
    result.stderr_output.push('\n');
    result.summary = reason;
    result.exit_code = 1;
}

fn prompt_requires_result_toml_path(prompt: &str) -> bool {
    prompt.lines().any(|line| {
        let normalized = strip_marker_line_prefix(line).trim().to_ascii_lowercase();
        normalized == RESULT_TOML_PATH_CONTRACT_MARKER
            || normalized == format!("contract marker: {RESULT_TOML_PATH_CONTRACT_MARKER}")
    })
}

fn contract_result_toml_path_candidate(result: &ExecutionResult) -> &str {
    // 1. Whole-line match (exact path on its own line).
    let output_path_candidate = result
        .output
        .lines()
        .rev()
        .find(|line| line_looks_like_result_toml_path(line))
        .map(normalize_contract_path_candidate);
    if let Some(candidate) = output_path_candidate {
        return candidate;
    }

    // 2. Summary as path.
    let summary_candidate = normalize_contract_path_candidate(&result.summary);
    if !summary_candidate.is_empty() && line_looks_like_result_toml_path(summary_candidate) {
        return summary_candidate;
    }

    // 3. Embedded path extraction: scan output lines for an absolute result.toml
    //    path that appears as a substring within a longer line.
    if let Some(embedded) = extract_embedded_result_toml_path(&result.output) {
        return embedded;
    }

    // 4. Embedded path in summary.
    if let Some(embedded) = extract_embedded_result_toml_path(&result.summary) {
        return embedded;
    }

    // 5. Last non-empty output line (legacy fallback).
    result
        .output
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .map(normalize_contract_path_candidate)
        .unwrap_or("")
}

/// Extract an embedded absolute `result.toml` or `user-result.toml` path from
/// text that may contain the path as a substring within a longer line.
///
/// Scans each line for a `/` character that begins an absolute path ending with
/// `result.toml` or `user-result.toml`, stripping surrounding quotes/backticks.
fn extract_embedded_result_toml_path(text: &str) -> Option<&str> {
    for line in text.lines().rev() {
        if let Some(path) = find_result_toml_path_in_line(line) {
            return Some(path);
        }
    }
    None
}

/// Find an absolute result.toml path embedded anywhere in a single line.
/// Returns the longest matching substring that starts with `/` and ends with
/// `result.toml` or `user-result.toml`.
fn find_result_toml_path_in_line(line: &str) -> Option<&str> {
    const SUFFIXES: &[&str] = &["result.toml", "user-result.toml"];

    for suffix in SUFFIXES {
        // Search from the end to prefer the last occurrence.
        let mut search_from = line.len();
        while let Some(end_pos) = line[..search_from].rfind(suffix) {
            let candidate_end = end_pos + suffix.len();
            // Walk backwards from end_pos to find the start `/`.
            // The path must start with `/` and contain only path-legal characters.
            if let Some(start) = find_absolute_path_start(&line[..end_pos]) {
                let raw = &line[start..candidate_end];
                let cleaned = raw.trim_matches(|c: char| c == '"' || c == '`' || c == '\'');
                let path = Path::new(cleaned);
                if path.is_absolute()
                    && path.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
                        n.eq_ignore_ascii_case("result.toml")
                            || n.eq_ignore_ascii_case("user-result.toml")
                    })
                {
                    return Some(cleaned);
                }
            }
            // Continue searching before this occurrence.
            search_from = end_pos;
            if search_from == 0 {
                break;
            }
        }
    }
    None
}

/// Find the start index of an absolute path that ends at `before_suffix`.
/// Walks backwards from the end to find a `/` that begins the path,
/// skipping only characters valid in Unix paths.
fn find_absolute_path_start(before_suffix: &str) -> Option<usize> {
    // Walk backwards to find the leading `/` of the absolute path.
    // Path characters: anything except whitespace and certain delimiters.
    let bytes = before_suffix.as_bytes();
    let mut i = bytes.len();
    while i > 0 {
        i -= 1;
        let c = bytes[i];
        // Stop at whitespace or common non-path delimiters, but allow the
        // path to start with `/` preceded by whitespace.
        if c == b'/' {
            // Check if this is the root `/` (preceded by start-of-string,
            // whitespace, quote, or backtick).
            if i == 0
                || matches!(
                    bytes[i - 1],
                    b' ' | b'\t' | b'"' | b'\'' | b'`' | b'(' | b'[' | b'{'
                )
            {
                return Some(i);
            }
            // Otherwise it's a path separator within the path, keep going.
            continue;
        }
        if c.is_ascii_whitespace() || c == b'(' || c == b'[' || c == b'{' {
            // The path starts after this delimiter.
            return None;
        }
    }
    None
}

fn path_matches_expected_contract_result(path_candidate: &str, expected_path: &Path) -> bool {
    let path = Path::new(path_candidate);
    path == expected_path && expected_contract_file_is_valid(expected_path)
}

fn expected_contract_file_is_valid(path: &Path) -> bool {
    let has_expected_file_name =
        path.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| {
                name.eq_ignore_ascii_case("result.toml")
                    || name.eq_ignore_ascii_case("user-result.toml")
            });

    if !path.is_absolute() || !has_expected_file_name || !path.is_file() {
        return false;
    }

    let meta = std::fs::symlink_metadata(path)
        .ok()
        .filter(|meta| !meta.file_type().is_symlink());
    let Some(meta) = meta else {
        return false;
    };

    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.nlink() != 1 {
            return false;
        }
    }

    true
}

fn sidecar_result_fallback_is_valid(path: &Path) -> bool {
    if !expected_contract_file_is_valid(path) {
        return false;
    }

    let Ok(contents) = std::fs::read_to_string(path) else {
        return false;
    };

    matches!(
        toml::from_str::<toml::Value>(&contents),
        Ok(toml::Value::Table(table)) if !table.is_empty()
    )
}

/// Validates session-dir result.toml as a disk-based fallback when the path
/// could not be extracted from output/summary. Applies the same validation as
/// user-result fallback: file must exist, not be a symlink, have nlink==1,
/// and contain valid non-empty TOML.
fn session_result_fallback_is_valid(path: &Path) -> bool {
    if !expected_contract_file_is_valid(path) {
        return false;
    }

    let Ok(contents) = std::fs::read_to_string(path) else {
        return false;
    };

    matches!(
        toml::from_str::<toml::Value>(&contents),
        Ok(toml::Value::Table(table)) if !table.is_empty()
    )
}

fn line_looks_like_result_toml_path(line: &str) -> bool {
    let candidate = normalize_contract_path_candidate(line);
    let path = Path::new(candidate);
    path.is_absolute()
        && path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| {
                name.eq_ignore_ascii_case("result.toml")
                    || name.eq_ignore_ascii_case("user-result.toml")
            })
}

pub(super) fn strip_marker_line_prefix(line: &str) -> &str {
    let trimmed = line.trim_start();
    if let Some(stripped) = trimmed
        .strip_prefix("- ")
        .or_else(|| trimmed.strip_prefix("* "))
        .or_else(|| trimmed.strip_prefix("+ "))
    {
        return stripped.trim_start();
    }

    let digit_count = trimmed.chars().take_while(|c| c.is_ascii_digit()).count();
    if digit_count > 0 {
        let suffix = &trimmed[digit_count..];
        if let Some(stripped) = suffix
            .strip_prefix(". ")
            .or_else(|| suffix.strip_prefix(") "))
        {
            return stripped.trim_start();
        }
    }

    trimmed
}

pub(super) fn clear_expected_result_toml(path: &Path) -> bool {
    match std::fs::remove_file(path) {
        Ok(()) => true,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => true,
        Err(err) => {
            warn!(
                path = %path.display(),
                error = %err,
                "Failed to remove pre-existing result.toml before execution"
            );
            false
        }
    }
}

pub(super) fn clear_expected_result_tomls(session_dir: &Path) -> bool {
    let session_result_path = session_dir.join("result.toml");
    let contract_output_path = csa_session::contract_result_path(session_dir);
    let legacy_output_path = csa_session::legacy_user_result_path(session_dir);
    let session_cleared = clear_expected_result_toml(&session_result_path);
    let contract_output_cleared = clear_expected_result_toml(&contract_output_path);
    let legacy_output_cleared = clear_expected_result_toml(&legacy_output_path);
    session_cleared && contract_output_cleared && legacy_output_cleared
}

fn normalize_contract_path_candidate(path: &str) -> &str {
    path.trim().trim_matches('"').trim_matches('`').trim()
}
