//! Shell command parsing helpers for post-run commit policy enforcement.
//!
//! Extracted from `run_cmd.rs` to keep module sizes manageable.

pub(crate) fn detect_no_verify_commit_commands(executed_shell_commands: &[String]) -> Vec<String> {
    let mut matches = Vec::new();
    for command in executed_shell_commands {
        let trimmed = command.trim();
        if trimmed.is_empty() {
            continue;
        }
        if !command_contains_forbidden_no_verify_commit(trimmed) {
            continue;
        }
        if !matches.iter().any(|existing| existing == trimmed) {
            matches.push(trimmed.to_string());
        }
    }
    matches
}

/// Detect commands that set `LEFTHOOK=0` or `LEFTHOOK_SKIP` to bypass
/// pre-commit hooks.  Mirrors `detect_no_verify_commit_commands` for the
/// hook-bypass-prevention policy (AGENTS.md rule 029).
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

pub(crate) fn detect_no_verify_commit_commands_from_tool_output(
    result: &csa_process::ExecutionResult,
    trace_only: bool,
) -> Vec<String> {
    let mut matches = Vec::new();
    collect_no_verify_command_like_lines(&result.output, &mut matches, trace_only);
    collect_no_verify_command_like_lines(&result.summary, &mut matches, trace_only);
    collect_no_verify_command_like_lines(&result.stderr_output, &mut matches, trace_only);
    matches
}

fn collect_no_verify_command_like_lines(source: &str, matches: &mut Vec<String>, trace_only: bool) {
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
        if !command_contains_forbidden_no_verify_commit(normalized_command) {
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

fn looks_like_shell_command_line(line: &str) -> bool {
    let command_line = strip_command_prompt_prefix(line);
    let Some(first_token) = command_line.split_whitespace().next() else {
        return false;
    };
    if is_env_assignment(first_token) {
        return true;
    }
    is_git_token(first_token)
        || is_shell_token(first_token)
        || first_token.rsplit('/').next() == Some("env")
        || first_token.rsplit('/').next() == Some("sudo")
        || first_token.eq_ignore_ascii_case("sudo")
        || first_token.eq_ignore_ascii_case("env")
        || first_token.eq_ignore_ascii_case("command")
        || first_token.eq_ignore_ascii_case("time")
        || first_token.eq_ignore_ascii_case("export")
}

fn has_command_prompt_prefix(line: &str) -> bool {
    line.starts_with("$ ") || line.starts_with("+ ")
}

fn strip_command_prompt_prefix(line: &str) -> &str {
    line.strip_prefix("$ ")
        .or_else(|| line.strip_prefix("+ "))
        .unwrap_or(line)
}

fn command_contains_forbidden_no_verify_commit(command: &str) -> bool {
    split_shell_segments_preserving_quotes(command)
        .into_iter()
        .any(|segment| segment_contains_forbidden_no_verify_commit(&segment))
}

fn split_shell_segments_preserving_quotes(command: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let mut escaped = false;
    let mut chars = command.chars().peekable();

    while let Some(ch) = chars.next() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }

        if in_single_quote {
            if ch == '\'' {
                current.push(ch);
                in_single_quote = false;
            } else {
                current.push(ch);
            }
            continue;
        }

        if in_double_quote {
            match ch {
                '"' => {
                    current.push(ch);
                    in_double_quote = false;
                }
                '\\' => escaped = true,
                _ => current.push(ch),
            }
            continue;
        }

        match ch {
            '\\' => escaped = true,
            '\'' => {
                current.push(ch);
                in_single_quote = true;
            }
            '"' => {
                current.push(ch);
                in_double_quote = true;
            }
            '\n' | ';' => push_shell_segment(&mut segments, &mut current),
            '&' | '|' => {
                if chars.peek().is_some_and(|next| *next == ch) {
                    let _ = chars.next();
                }
                push_shell_segment(&mut segments, &mut current);
            }
            _ => current.push(ch),
        }
    }

    push_shell_segment(&mut segments, &mut current);
    segments
}

fn push_shell_segment(segments: &mut Vec<String>, current: &mut String) {
    let trimmed = current.trim();
    if !trimmed.is_empty() {
        segments.push(trimmed.to_string());
    }
    current.clear();
}

fn segment_contains_forbidden_no_verify_commit(segment: &str) -> bool {
    let tokens = tokenize_shell_tokens(segment);
    if tokens.is_empty() {
        return false;
    }

    if let Some(shell_script_tokens) = extract_shell_c_payload_tokens(&tokens)
        && shell_script_contains_forbidden_no_verify_commit(shell_script_tokens)
    {
        return true;
    }

    let Some((_, git_commit_subcommand_idx)) = locate_git_commit_command(&tokens) else {
        return false;
    };
    commit_args_include_no_verify(&tokens[git_commit_subcommand_idx + 1..])
}

fn tokenize_shell_tokens(segment: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut chars = segment.chars().peekable();
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let mut escaped = false;

    while let Some(ch) = chars.next() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }

        if ch == '\\' {
            escaped = true;
            continue;
        }

        if in_single_quote {
            if ch == '\'' {
                in_single_quote = false;
            } else {
                current.push(ch);
            }
            continue;
        }

        if in_double_quote {
            if ch == '"' {
                in_double_quote = false;
            } else {
                current.push(ch);
            }
            continue;
        }

        match ch {
            '\'' => in_single_quote = true,
            '"' => in_double_quote = true,
            ch if ch.is_whitespace() => push_shell_token(&mut tokens, &mut current),
            ';' => {
                push_shell_token(&mut tokens, &mut current);
                tokens.push(";".to_string());
            }
            '&' => {
                push_shell_token(&mut tokens, &mut current);
                if chars.peek().is_some_and(|next| *next == '&') {
                    let _ = chars.next();
                    tokens.push("&&".to_string());
                } else {
                    tokens.push("&".to_string());
                }
            }
            '|' => {
                push_shell_token(&mut tokens, &mut current);
                if chars.peek().is_some_and(|next| *next == '|') {
                    let _ = chars.next();
                    tokens.push("||".to_string());
                } else {
                    tokens.push("|".to_string());
                }
            }
            _ => current.push(ch),
        }
    }

    if escaped {
        current.push('\\');
    }
    push_shell_token(&mut tokens, &mut current);
    tokens
}

fn push_shell_token(tokens: &mut Vec<String>, current: &mut String) {
    let trimmed = current.trim();
    if !trimmed.is_empty() {
        tokens.push(trimmed.to_string());
    }
    current.clear();
}

fn extract_shell_c_payload_tokens(tokens: &[String]) -> Option<&[String]> {
    let idx = skip_command_prefix_tokens(tokens, 0);
    if idx + 2 >= tokens.len() {
        return None;
    }
    if !is_shell_token(tokens[idx].as_str()) {
        return None;
    }
    let shell_flag = tokens[idx + 1].as_str();
    if !shell_flag.starts_with('-') || !shell_flag.contains('c') {
        return None;
    }
    Some(&tokens[idx + 2..])
}

fn locate_git_commit_command(tokens: &[String]) -> Option<(usize, usize)> {
    let idx = skip_command_prefix_tokens(tokens, 0);
    if idx >= tokens.len() {
        return None;
    }
    if !is_git_token(tokens[idx].as_str()) {
        return None;
    }
    let subcommand_idx = find_git_commit_subcommand(tokens, idx + 1)?;
    Some((idx, subcommand_idx))
}

fn shell_script_contains_forbidden_no_verify_commit(tokens: &[String]) -> bool {
    let script_tokens = expand_shell_script_tokens(tokens);
    for git_idx in 0..script_tokens.len() {
        if !is_git_token(script_tokens[git_idx].as_str())
            || !is_shell_command_boundary(&script_tokens, git_idx)
        {
            continue;
        }
        let Some(commit_idx) = find_git_commit_subcommand(&script_tokens, git_idx + 1) else {
            continue;
        };
        if commit_args_include_no_verify(&script_tokens[commit_idx + 1..]) {
            return true;
        }
    }
    false
}

fn expand_shell_script_tokens(tokens: &[String]) -> Vec<String> {
    let mut expanded = Vec::new();
    for token in tokens {
        let nested_tokens = tokenize_shell_tokens(token);
        if nested_tokens.is_empty() {
            continue;
        }
        expanded.extend(nested_tokens);
    }
    expanded
}

fn is_shell_command_boundary(tokens: &[String], idx: usize) -> bool {
    idx == 0 || is_command_separator_token(tokens[idx - 1].as_str())
}

fn is_command_separator_token(token: &str) -> bool {
    matches!(token, ";" | "&&" | "||" | "|" | "&")
        || token.ends_with(';')
        || token.ends_with("&&")
        || token.ends_with("||")
        || token.ends_with('|')
        || token.ends_with('&')
}

fn skip_command_prefix_tokens(tokens: &[String], idx: usize) -> usize {
    skip_command_prefix_tokens_with_mode(tokens, idx, PrefixSkipMode::FullCommandPrefix)
}

fn skip_command_wrapper_tokens(tokens: &[String], idx: usize) -> usize {
    skip_command_prefix_tokens_with_mode(tokens, idx, PrefixSkipMode::WrapperOnly)
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum PrefixSkipMode {
    FullCommandPrefix,
    WrapperOnly,
}

fn skip_command_prefix_tokens_with_mode(
    tokens: &[String],
    mut idx: usize,
    mode: PrefixSkipMode,
) -> usize {
    while idx < tokens.len() {
        let token = tokens[idx].as_str();
        if mode == PrefixSkipMode::FullCommandPrefix && is_env_assignment(token) {
            idx += 1;
            continue;
        }
        if token.eq_ignore_ascii_case("sudo") || token.rsplit('/').next() == Some("sudo") {
            idx += 1;
            idx = skip_prefixed_command_options(tokens, idx, sudo_option_consumes_value);
            continue;
        }
        if mode == PrefixSkipMode::FullCommandPrefix
            && (token.eq_ignore_ascii_case("env") || token.ends_with("/env"))
        {
            idx += 1;
            idx = skip_prefixed_command_options(tokens, idx, env_option_consumes_value);
            while idx < tokens.len() && is_env_assignment(tokens[idx].as_str()) {
                idx += 1;
            }
            continue;
        }
        if token.eq_ignore_ascii_case("command") || token == "--" {
            idx += 1;
            continue;
        }
        if token.eq_ignore_ascii_case("time") {
            idx += 1;
            idx = skip_prefixed_command_options(tokens, idx, |_token| false);
            continue;
        }
        break;
    }
    idx
}

fn find_git_commit_subcommand(tokens: &[String], mut idx: usize) -> Option<usize> {
    while idx < tokens.len() {
        let token = tokens[idx].as_str();
        if token.eq_ignore_ascii_case("commit") {
            return Some(idx);
        }
        if token == "--" {
            if idx + 1 < tokens.len() && tokens[idx + 1].eq_ignore_ascii_case("commit") {
                return Some(idx + 1);
            }
            return None;
        }
        if !token.starts_with('-') {
            return None;
        }
        if git_global_option_consumes_value(token) && idx + 1 < tokens.len() {
            idx += 2;
            continue;
        }
        idx += 1;
    }
    None
}

fn skip_prefixed_command_options<F>(tokens: &[String], mut idx: usize, consumes_value: F) -> usize
where
    F: Fn(&str) -> bool,
{
    while idx < tokens.len() {
        let token = tokens[idx].as_str();
        if token == "--" {
            idx += 1;
            break;
        }
        if !token.starts_with('-') {
            break;
        }
        let takes_value = consumes_value(token) && !token.contains('=');
        idx += 1;
        if takes_value && idx < tokens.len() {
            idx += 1;
        }
    }
    idx
}

fn git_global_option_consumes_value(token: &str) -> bool {
    matches!(
        token,
        "-c" | "-C" | "--exec-path" | "--git-dir" | "--work-tree" | "--namespace" | "--config-env"
    )
}

fn env_option_consumes_value(token: &str) -> bool {
    matches!(
        token,
        "-u" | "--unset" | "-C" | "--chdir" | "-S" | "--split-string"
    )
}

fn sudo_option_consumes_value(token: &str) -> bool {
    matches!(
        token,
        "-u" | "--user"
            | "-g"
            | "--group"
            | "-h"
            | "--host"
            | "-p"
            | "--prompt"
            | "-r"
            | "--role"
            | "-t"
            | "--type"
            | "-C"
            | "--chdir"
    )
}

fn is_env_assignment(token: &str) -> bool {
    token
        .find('=')
        .is_some_and(|eq_pos| eq_pos > 0 && !token.starts_with('-'))
}

fn is_shell_token(token: &str) -> bool {
    matches!(
        token.rsplit('/').next(),
        Some("bash" | "sh" | "zsh" | "fish")
    )
}

fn is_git_token(token: &str) -> bool {
    token.eq_ignore_ascii_case("git") || token.ends_with("/git")
}

fn commit_args_include_no_verify(args: &[String]) -> bool {
    let mut idx = 0usize;
    while idx < args.len() {
        let token = args[idx].as_str();
        if token == "--" || is_command_separator_token(token) {
            break;
        }
        if token.eq_ignore_ascii_case("--no-verify") {
            return true;
        }

        if token.starts_with("--") {
            idx += 1;
            if long_option_consumes_value(token) && !token.contains('=') {
                idx = consume_option_value(args, idx, long_option_is_message_like(token));
            }
            continue;
        }

        if token.starts_with('-') && token.len() > 1 {
            let mut chars = token[1..].chars().peekable();
            let mut consumes_value = false;
            let mut message_like = false;
            while let Some(flag) = chars.next() {
                if flag == 'n' {
                    return true;
                }
                if short_option_consumes_value(flag) {
                    consumes_value = chars.peek().is_none();
                    message_like = short_option_is_message_like(flag);
                    break;
                }
            }
            idx += 1;
            if consumes_value {
                idx = consume_option_value(args, idx, message_like);
            }
            continue;
        }

        idx += 1;
    }
    false
}

fn consume_option_value(args: &[String], mut idx: usize, message_like: bool) -> usize {
    if !message_like {
        if idx < args.len() {
            idx += 1;
        }
        return idx;
    }

    if idx < args.len() {
        idx += 1;
    }

    while idx < args.len() {
        let token = args[idx].as_str();
        if is_command_separator_token(token) || token.starts_with('-') {
            break;
        }
        idx += 1;
    }
    idx
}

fn short_option_consumes_value(flag: char) -> bool {
    matches!(flag, 'm' | 'F' | 'c' | 'C' | 't')
}

fn short_option_is_message_like(flag: char) -> bool {
    matches!(flag, 'm')
}

fn long_option_consumes_value(token: &str) -> bool {
    matches!(
        token,
        "--message"
            | "--file"
            | "--template"
            | "--reuse-message"
            | "--reedit-message"
            | "--fixup"
            | "--squash"
            | "--author"
            | "--date"
            | "--trailer"
            | "--pathspec-from-file"
            | "--cleanup"
    )
}

fn long_option_is_message_like(token: &str) -> bool {
    token == "--message"
}

// ── LEFTHOOK bypass detection ──────────────────────────────────────

/// Forbidden LEFTHOOK env var names that disable pre-commit hooks.
const FORBIDDEN_LEFTHOOK_ENV_VARS: &[&str] = &["LEFTHOOK", "LEFTHOOK_SKIP"];

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

    // Check for `sh -c "export LEFTHOOK=0; ..."` or similar shell wrappers.
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

/// Scan a flat token list for any LEFTHOOK bypass env assignment.
///
/// Handles:
///   - Inline env prefix: `LEFTHOOK=0 git commit`
///   - `export LEFTHOOK=0`
///   - `env LEFTHOOK=0 git ...`
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
            let mut saw_separator = false;
            while idx < tokens.len() {
                let next = tokens[idx].as_str();
                if is_command_separator_token(next) {
                    idx += 1;
                    idx = skip_command_wrapper_tokens(tokens, idx);
                    saw_separator = true;
                    break;
                }
                if !is_env_assignment(next) {
                    idx = skip_to_next_command_boundary(tokens, idx + 1);
                    break;
                }
                if is_lefthook_env_assignment(next) {
                    return true;
                }
                idx += 1;
            }
            if saw_separator {
                continue;
            }
            return false;
        }

        if token.eq_ignore_ascii_case("export") {
            idx += 1;
            let mut saw_separator = false;
            while idx < tokens.len() {
                let next = tokens[idx].as_str();
                if is_command_separator_token(next) {
                    idx += 1;
                    idx = skip_command_wrapper_tokens(tokens, idx);
                    saw_separator = true;
                    break;
                }
                if !is_env_assignment(next) {
                    idx = skip_to_next_command_boundary(tokens, idx + 1);
                    break;
                }
                if is_lefthook_env_assignment(next) {
                    return true;
                }
                idx += 1;
            }
            if saw_separator {
                continue;
            }
            return false;
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

/// Return true if `token` is `LEFTHOOK=<val>` or `LEFTHOOK_SKIP=<val>`.
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
