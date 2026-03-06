//! Result.toml path contract enforcement for session execution.
//!
//! When a prompt contains the contract marker `csa_result_toml_path_contract=1`,
//! the session output must contain the absolute path to a valid `result.toml` or
//! `user-result.toml` file inside the session directory.  If the contract is
//! violated the execution result is coerced to failure.

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
        let reason = format!(
            "contract violation: failed to clear pre-existing result.toml '{}' before execution; refusing to trust stale file",
            expected_path.display()
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
    let expected_user_result_path = session_dir.join("output").join("user-result.toml");
    if path_matches_expected_contract_result(path_candidate, &expected_path)
        || path_matches_expected_contract_result(path_candidate, &expected_user_result_path)
    {
        return;
    }

    if user_result_fallback_is_valid(&expected_user_result_path) {
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

    let reason = if path_candidate.is_empty() {
        format!(
            "contract violation: expected existing absolute result path '{}' or '{}' in output/summary, but output and summary were empty",
            expected_path.display(),
            expected_user_result_path.display()
        )
    } else {
        format!(
            "contract violation: expected existing absolute result path '{}' or '{}' in output/summary, got '{path_candidate}'",
            expected_path.display(),
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
    let output_path_candidate = result
        .output
        .lines()
        .rev()
        .find(|line| line_looks_like_result_toml_path(line))
        .map(normalize_contract_path_candidate);
    if let Some(candidate) = output_path_candidate {
        return candidate;
    }

    let summary_candidate = normalize_contract_path_candidate(&result.summary);
    if !summary_candidate.is_empty() {
        return summary_candidate;
    }

    result
        .output
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .map(normalize_contract_path_candidate)
        .unwrap_or("")
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

fn user_result_fallback_is_valid(path: &Path) -> bool {
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

fn normalize_contract_path_candidate(path: &str) -> &str {
    path.trim().trim_matches('"').trim_matches('`').trim()
}
