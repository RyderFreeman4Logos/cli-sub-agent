use super::*;

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

pub(crate) fn detect_hook_bypass_env_usage(
    executed_shell_commands: &[String],
    execution_env: Option<&std::collections::HashMap<String, String>>,
) -> Vec<String> {
    let Some(execution_env) = execution_env else {
        return Vec::new();
    };
    let Some(command) = executed_shell_commands
        .iter()
        .map(|command| command.trim())
        .find(|command| command_contains_hook_sensitive_execution(command))
    else {
        return Vec::new();
    };

    let mut matches = Vec::new();
    for (key, value) in execution_env {
        if !env_value_disables_hooks(key, value) {
            continue;
        }
        let matched = format!("{key}={value} applied while executing: {command}");
        if !matches.iter().any(|existing| existing == &matched) {
            matches.push(matched);
        }
    }
    matches
}

pub(crate) fn command_contains_forbidden_lefthook_bypass(command: &str) -> bool {
    split_shell_segments_preserving_quotes(command)
        .into_iter()
        .any(|segment| segment_contains_forbidden_lefthook_bypass(&segment))
}

/// Check whether a single shell segment sets a hook-bypass env var.
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
            if is_hook_bypass_env_assignment(token) {
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
        if is_hook_bypass_env_assignment(next) {
            return (true, idx);
        }
        idx += 1;
    }

    (false, idx)
}

fn is_hook_bypass_env_assignment(token: &str) -> bool {
    let Some(eq_pos) = token.find('=') else {
        return false;
    };
    if eq_pos == 0 || token.starts_with('-') {
        return false;
    }
    env_value_disables_hooks(&token[..eq_pos], &token[eq_pos + 1..])
}

fn env_value_disables_hooks(key: &str, value: &str) -> bool {
    match key.to_ascii_uppercase().as_str() {
        "LEFTHOOK" => value == "0",
        "LEFTHOOK_DISABLED" => value == "1",
        "LEFTHOOK_SKIP" => !value.is_empty(),
        "HUSKY" => value == "0",
        "HUSKY_DISABLE" => value == "1",
        "SKIP_HOOKS" | "SKIP_GIT_HOOKS" | "PRE_COMMIT_ALLOW_NO_CONFIG" => value == "1",
        _ => false,
    }
}

fn command_contains_hook_sensitive_execution(command: &str) -> bool {
    split_shell_segments_preserving_quotes(command)
        .into_iter()
        .any(|segment| segment_contains_hook_sensitive_execution(&segment))
}

fn segment_contains_hook_sensitive_execution(segment: &str) -> bool {
    let tokens = tokenize_shell_tokens(segment);
    if tokens.is_empty() {
        return false;
    }

    if let Some(shell_script_tokens) = extract_shell_c_payload_tokens(&tokens)
        && shell_script_contains_hook_sensitive_execution(shell_script_tokens)
    {
        return true;
    }

    command_tokens_contain_hook_sensitive_execution(&tokens)
}

fn shell_script_contains_hook_sensitive_execution(tokens: &[String]) -> bool {
    let script_tokens = expand_shell_script_tokens(tokens);
    command_tokens_contain_hook_sensitive_execution(&script_tokens)
}

fn command_tokens_contain_hook_sensitive_execution(tokens: &[String]) -> bool {
    let mut command_start = 0usize;

    while command_start < tokens.len() {
        while command_start < tokens.len()
            && is_command_separator_token(tokens[command_start].as_str())
        {
            command_start += 1;
        }
        if command_start >= tokens.len() {
            break;
        }

        let command_end = tokens[command_start..]
            .iter()
            .position(|token| is_command_separator_token(token.as_str()))
            .map_or(tokens.len(), |idx| command_start + idx);
        let command_tokens = &tokens[command_start..command_end];
        let command_idx = skip_command_prefix_tokens(command_tokens, 0);

        if command_idx < command_tokens.len() {
            let command = command_tokens[command_idx].as_str();
            if command_name_is(command, "lefthook") || command_name_is(command, "pre-commit") {
                return true;
            }
            if is_git_token(command)
                && find_git_hook_relevant_subcommand(command_tokens, command_idx + 1).is_some()
            {
                return true;
            }
        }

        command_start = command_end.saturating_add(1);
    }

    false
}
