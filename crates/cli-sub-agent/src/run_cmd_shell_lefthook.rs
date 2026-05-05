use super::*;

/// Forbidden LEFTHOOK env var names that disable pre-commit hooks.
const FORBIDDEN_LEFTHOOK_ENV_VARS: &[&str] = &["LEFTHOOK", "LEFTHOOK_SKIP"];

pub(crate) fn detect_lefthook_bypass_commands(executed_shell_commands: &[String]) -> Vec<String> {
    let mut matches = Vec::new();
    for command in executed_shell_commands {
        let trimmed = command.trim();
        if trimmed.is_empty() {
            continue;
        }
        if !command_contains_forbidden_lefthook_bypass(trimmed) {
            continue;
        }
        if !matches.iter().any(|existing| existing == trimmed) {
            matches.push(trimmed.to_string());
        }
    }
    matches
}

pub(crate) fn detect_lefthook_bypass_commands_from_tool_output(
    result: &csa_process::ExecutionResult,
    trace_only: bool,
) -> Vec<String> {
    let mut matches = Vec::new();
    collect_lefthook_bypass_command_like_lines(&result.output, &mut matches, trace_only);
    collect_lefthook_bypass_command_like_lines(&result.summary, &mut matches, trace_only);
    collect_lefthook_bypass_command_like_lines(&result.stderr_output, &mut matches, trace_only);
    matches
}

fn collect_lefthook_bypass_command_like_lines(
    source: &str,
    matches: &mut Vec<String>,
    trace_only: bool,
) {
    let mut inside_code_fence = false;
    for line in source.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            inside_code_fence = !inside_code_fence;
            continue;
        }
        if inside_code_fence || trimmed.is_empty() {
            continue;
        }
        if trace_only && !has_command_prompt_prefix(trimmed) {
            continue;
        }
        if !looks_like_shell_command_line(trimmed) {
            continue;
        }
        let normalized_command = strip_command_prompt_prefix(trimmed);
        if !command_contains_forbidden_lefthook_bypass(normalized_command) {
            continue;
        }
        if !matches
            .iter()
            .any(|existing| existing == normalized_command)
        {
            matches.push(normalized_command.to_string());
        }
    }
}

pub(crate) fn command_contains_forbidden_lefthook_bypass(command: &str) -> bool {
    split_shell_segments_preserving_quotes(command)
        .into_iter()
        .any(|segment| segment_contains_forbidden_lefthook_bypass(&segment))
}

/// Check whether a single shell segment sets a forbidden LEFTHOOK env var.
///
/// Detects patterns:
/// - `LEFTHOOK=0 git commit ...`  (inline env assignment before command)
/// - `export LEFTHOOK=0`
/// - `env LEFTHOOK=0 git commit ...`
/// - `LEFTHOOK_SKIP=... git push ...`
pub(crate) fn segment_contains_forbidden_lefthook_bypass(segment: &str) -> bool {
    let tokens = tokenize_shell_tokens(segment);
    if tokens.is_empty() {
        return false;
    }

    if let Some(shell_script_tokens) = extract_shell_c_payload_tokens(&tokens)
        && shell_script_contains_forbidden_lefthook_bypass(shell_script_tokens)
    {
        return true;
    }

    tokens_contain_lefthook_bypass(&tokens)
}

fn shell_script_contains_forbidden_lefthook_bypass(tokens: &[String]) -> bool {
    let script_tokens = expand_shell_script_tokens(tokens);
    tokens_contain_lefthook_bypass(&script_tokens)
}

fn skip_to_next_command_boundary(tokens: &[String], mut idx: usize) -> usize {
    while idx < tokens.len() && !is_command_separator_token(tokens[idx].as_str()) {
        idx += 1;
    }
    idx
}

fn tokens_contain_lefthook_bypass(tokens: &[String]) -> bool {
    let mut idx = skip_command_wrapper_tokens(tokens, 0);

    while idx < tokens.len() {
        let token = tokens[idx].as_str();

        if is_command_separator_token(token) {
            idx += 1;
            idx = skip_command_wrapper_tokens(tokens, idx);
            continue;
        }

        if token.eq_ignore_ascii_case("env") || token.ends_with("/env") {
            idx += 1;
            idx = skip_prefixed_command_options(tokens, idx, env_option_consumes_value);
            let (contains_bypass, next_idx) = scan_env_assignments_for_lefthook_bypass(tokens, idx);
            if contains_bypass {
                return true;
            }
            idx = next_idx;
            continue;
        }

        if token.eq_ignore_ascii_case("export") {
            idx += 1;
            let (contains_bypass, next_idx) = scan_env_assignments_for_lefthook_bypass(tokens, idx);
            if contains_bypass {
                return true;
            }
            idx = next_idx;
            continue;
        }

        if is_env_assignment(token) {
            if is_lefthook_env_assignment(token) {
                return true;
            }
            idx += 1;
            continue;
        }

        idx = skip_to_next_command_boundary(tokens, idx + 1);
    }

    false
}

fn scan_env_assignments_for_lefthook_bypass(tokens: &[String], mut idx: usize) -> (bool, usize) {
    while idx < tokens.len() {
        let next = tokens[idx].as_str();
        if is_command_separator_token(next) {
            idx += 1;
            idx = skip_command_wrapper_tokens(tokens, idx);
            break;
        }
        if !is_env_assignment(next) {
            idx = skip_to_next_command_boundary(tokens, idx + 1);
            break;
        }
        if is_lefthook_env_assignment(next) {
            return (true, idx);
        }
        idx += 1;
    }

    (false, idx)
}

fn is_lefthook_env_assignment(token: &str) -> bool {
    let Some(eq_pos) = token.find('=') else {
        return false;
    };
    if eq_pos == 0 || token.starts_with('-') {
        return false;
    }
    let var_name = &token[..eq_pos];
    FORBIDDEN_LEFTHOOK_ENV_VARS
        .iter()
        .any(|forbidden| var_name.eq_ignore_ascii_case(forbidden))
}
