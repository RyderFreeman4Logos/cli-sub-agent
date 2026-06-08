//! Shell command parsing helpers for post-run commit policy enforcement, extracted from `run_cmd.rs`.

#[path = "run_cmd_shell_lefthook.rs"]
mod lefthook;

#[cfg(test)]
pub(crate) use lefthook::{
    command_contains_forbidden_lefthook_bypass, segment_contains_forbidden_lefthook_bypass,
};
pub(crate) use lefthook::{detect_hook_bypass_env_usage, detect_lefthook_bypass_commands};

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

pub(crate) fn detect_git_commit_commands(executed_shell_commands: &[String]) -> Vec<String> {
    let mut matches = Vec::new();
    for command in executed_shell_commands {
        let trimmed = command.trim();
        if trimmed.is_empty() {
            continue;
        }
        if !command_contains_git_commit(trimmed) {
            continue;
        }
        if !matches.iter().any(|existing| existing == trimmed) {
            matches.push(trimmed.to_string());
        }
    }
    matches
}

pub(crate) fn command_contains_forbidden_no_verify_commit(command: &str) -> bool {
    split_shell_segments_preserving_quotes(command)
        .into_iter()
        .any(|segment| segment_contains_forbidden_git_bypass(&segment))
}

pub(crate) fn command_contains_git_commit(command: &str) -> bool {
    split_shell_segments_preserving_quotes(command)
        .into_iter()
        .any(|segment| segment_contains_git_commit(&segment))
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
            if ch != '\n' {
                current.push(ch);
            }
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

fn segment_contains_forbidden_git_bypass(segment: &str) -> bool {
    let tokens = tokenize_shell_tokens(segment);
    if tokens.is_empty() {
        return false;
    }

    if let Some(shell_script_tokens) = extract_shell_c_payload_tokens(&tokens)
        && shell_script_contains_forbidden_git_bypass(shell_script_tokens)
    {
        return true;
    }

    let Some((_, git_subcommand_idx)) = locate_git_hook_relevant_command(&tokens) else {
        return false;
    };
    git_args_include_forbidden_bypass(
        tokens[git_subcommand_idx].as_str(),
        &tokens[git_subcommand_idx + 1..],
    )
}

fn segment_contains_git_commit(segment: &str) -> bool {
    let tokens = tokenize_shell_tokens(segment);
    if tokens.is_empty() {
        return false;
    }

    if let Some(shell_script_tokens) = extract_shell_c_payload_tokens(&tokens)
        && shell_script_contains_git_commit(shell_script_tokens)
    {
        return true;
    }

    locate_git_commit_command(&tokens).is_some()
}

pub(crate) fn tokenize_shell_tokens(segment: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut chars = segment.chars().peekable();
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let mut escaped = false;

    while let Some(ch) = chars.next() {
        if escaped {
            if ch != '\n' {
                current.push(ch);
            }
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
            '\n' => {
                push_shell_token(&mut tokens, &mut current);
                tokens.push(";".to_string());
            }
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
    let mut idx = skip_command_prefix_tokens(tokens, 0);
    if idx >= tokens.len() {
        return None;
    }
    if !is_shell_token(tokens[idx].as_str()) {
        return None;
    }
    idx += 1;

    while idx < tokens.len() {
        let shell_flag = tokens[idx].as_str();
        if shell_flag == "--" || !is_shell_option_token(shell_flag) {
            return None;
        }
        if shell_option_enables_c_payload(shell_flag) {
            return (idx + 1 < tokens.len()).then_some(&tokens[idx + 1..idx + 2]);
        }

        idx += 1;
        if shell_option_consumes_value(shell_flag) && idx < tokens.len() {
            idx += 1;
        }
    }

    None
}

fn is_shell_option_token(token: &str) -> bool {
    token.len() > 1 && (token.starts_with('-') || token.starts_with('+'))
}

fn shell_option_enables_c_payload(token: &str) -> bool {
    if token == "-c" || token == "--command" {
        return true;
    }
    if !token.starts_with('-') || token.starts_with("--") {
        return false;
    }

    for flag in token[1..].chars() {
        if flag == 'c' {
            return true;
        }
        if shell_short_option_consumes_value(flag) {
            return false;
        }
    }

    false
}

fn shell_option_consumes_value(token: &str) -> bool {
    if token.starts_with("--") {
        return shell_long_option_consumes_value(token) && !token.contains('=');
    }

    let mut chars = token[1..].chars().peekable();
    while let Some(flag) = chars.next() {
        if shell_short_option_consumes_value(flag) {
            return chars.peek().is_none();
        }
    }

    false
}

fn shell_short_option_consumes_value(flag: char) -> bool {
    matches!(flag, 'o' | 'O')
}

fn shell_long_option_consumes_value(token: &str) -> bool {
    matches!(token, "--init-file" | "--rcfile" | "--emulate")
}

fn locate_git_hook_relevant_command(tokens: &[String]) -> Option<(usize, usize)> {
    let idx = skip_command_prefix_tokens(tokens, 0);
    if idx >= tokens.len() {
        return None;
    }
    if !is_git_token(tokens[idx].as_str()) {
        return None;
    }
    let subcommand_idx = find_git_hook_relevant_subcommand(tokens, idx + 1)?;
    Some((idx, subcommand_idx))
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

fn shell_script_contains_forbidden_git_bypass(tokens: &[String]) -> bool {
    let script_tokens = expand_shell_script_tokens(tokens);
    let mut command_start = 0usize;

    while command_start < script_tokens.len() {
        while command_start < script_tokens.len()
            && is_command_separator_token(script_tokens[command_start].as_str())
        {
            command_start += 1;
        }
        if command_start >= script_tokens.len() {
            break;
        }

        let command_end = script_tokens[command_start..]
            .iter()
            .position(|token| is_command_separator_token(token.as_str()))
            .map_or(script_tokens.len(), |idx| command_start + idx);
        let command_tokens = &script_tokens[command_start..command_end];
        let git_idx = skip_command_prefix_tokens(command_tokens, 0);

        if git_idx < command_tokens.len()
            && is_git_token(command_tokens[git_idx].as_str())
            && let Some(subcommand_idx) =
                find_git_hook_relevant_subcommand(command_tokens, git_idx + 1)
            && git_args_include_forbidden_bypass(
                command_tokens[subcommand_idx].as_str(),
                &command_tokens[subcommand_idx + 1..],
            )
        {
            return true;
        }

        command_start = command_end.saturating_add(1);
    }

    false
}

fn shell_script_contains_git_commit(tokens: &[String]) -> bool {
    let script_tokens = expand_shell_script_tokens(tokens);
    let mut command_start = 0usize;

    while command_start < script_tokens.len() {
        while command_start < script_tokens.len()
            && is_command_separator_token(script_tokens[command_start].as_str())
        {
            command_start += 1;
        }
        if command_start >= script_tokens.len() {
            break;
        }

        let command_end = script_tokens[command_start..]
            .iter()
            .position(|token| is_command_separator_token(token.as_str()))
            .map_or(script_tokens.len(), |idx| command_start + idx);
        let command_tokens = &script_tokens[command_start..command_end];

        if locate_git_commit_command(command_tokens).is_some() {
            return true;
        }

        command_start = command_end.saturating_add(1);
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
        if command_name_is(token, "sudo") {
            idx += 1;
            idx = skip_prefixed_command_options(tokens, idx, sudo_option_consumes_value);
            continue;
        }
        if mode == PrefixSkipMode::FullCommandPrefix && command_name_is(token, "env") {
            idx += 1;
            idx = skip_prefixed_command_options(tokens, idx, env_option_consumes_value);
            while idx < tokens.len() && is_env_assignment(tokens[idx].as_str()) {
                idx += 1;
            }
            continue;
        }
        if command_name_is(token, "nice") {
            idx += 1;
            idx = skip_prefixed_command_options(tokens, idx, nice_option_consumes_value);
            continue;
        }
        if command_name_is(token, "ionice") {
            idx += 1;
            idx = skip_prefixed_command_options(tokens, idx, ionice_option_consumes_value);
            continue;
        }
        if command_name_is(token, "strace") || command_name_is(token, "ltrace") {
            idx += 1;
            idx = skip_prefixed_command_options(tokens, idx, trace_option_consumes_value);
            continue;
        }
        if command_name_is(token, "command") {
            idx += 1;
            idx = skip_prefixed_command_options(tokens, idx, command_option_consumes_value);
            continue;
        }
        if command_name_is(token, "time") {
            idx += 1;
            idx = skip_prefixed_command_options(tokens, idx, time_option_consumes_value);
            continue;
        }
        if command_name_is(token, "exec") || token == "--" {
            idx += 1;
            continue;
        }
        break;
    }
    idx
}

fn find_git_hook_relevant_subcommand(tokens: &[String], mut idx: usize) -> Option<usize> {
    while idx < tokens.len() {
        let token = tokens[idx].as_str();
        if is_hook_relevant_git_subcommand(token) {
            return Some(idx);
        }
        if token == "--" {
            if idx + 1 < tokens.len() && is_hook_relevant_git_subcommand(tokens[idx + 1].as_str()) {
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

fn is_hook_relevant_git_subcommand(token: &str) -> bool {
    token.eq_ignore_ascii_case("commit") || token.eq_ignore_ascii_case("push")
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

fn nice_option_consumes_value(token: &str) -> bool {
    matches!(token, "-n" | "--adjustment")
}

fn ionice_option_consumes_value(token: &str) -> bool {
    matches!(
        token,
        "-c" | "--class" | "-n" | "--classdata" | "-t" | "--ignore" | "-p" | "--pid"
    )
}

fn trace_option_consumes_value(token: &str) -> bool {
    matches!(
        token,
        "-e" | "--trace" | "-o" | "--output" | "-p" | "--attach" | "-u" | "--user"
    )
}

fn command_option_consumes_value(_token: &str) -> bool {
    false
}

fn time_option_consumes_value(_token: &str) -> bool {
    false
}

fn command_name_is(token: &str, name: &str) -> bool {
    token.eq_ignore_ascii_case(name) || token.rsplit('/').next() == Some(name)
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

fn git_args_include_forbidden_bypass(subcommand: &str, args: &[String]) -> bool {
    let mut idx = 0usize;
    while idx < args.len() {
        let token = args[idx].as_str();
        if token == "--" || is_command_separator_token(token) {
            break;
        }
        if token.eq_ignore_ascii_case("--no-verify") || token.eq_ignore_ascii_case("--no-gpg-sign")
        {
            return true;
        }

        if token.starts_with("--") {
            idx += 1;
            if long_option_consumes_value(token) && !token.contains('=') {
                idx = consume_option_value(args, idx);
            }
            continue;
        }

        if token.starts_with('-') && token.len() > 1 {
            let mut chars = token[1..].chars().peekable();
            let mut consumes_value = false;
            while let Some(flag) = chars.next() {
                if subcommand.eq_ignore_ascii_case("commit") && flag == 'n' {
                    return true;
                }
                if short_option_consumes_value(flag) {
                    consumes_value = chars.peek().is_none();
                    break;
                }
            }
            idx += 1;
            if consumes_value {
                idx = consume_option_value(args, idx);
            }
            continue;
        }

        idx += 1;
    }
    false
}

fn consume_option_value(args: &[String], mut idx: usize) -> usize {
    if idx < args.len() {
        idx += 1;
    }
    idx
}

fn short_option_consumes_value(flag: char) -> bool {
    matches!(flag, 'm' | 'F' | 'c' | 'C' | 't')
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

#[cfg(test)]
#[path = "run_cmd_shell_tests.rs"]
mod tests;
