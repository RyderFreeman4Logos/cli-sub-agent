//! `git commit --no-verify` / `git commit -n` heuristic detector.
//!
//! Used to flag ACP execute-tool-call titles that look like a hook-bypassing
//! commit, so they are never silently evicted from the bounded ring buffer.
//! Intentionally simpler than the authoritative shell parser in
//! `run_cmd_shell.rs`; the canonical decision still happens in
//! `apply_no_verify_commit_policy`.

/// Quick heuristic: does a tool-call title look like `git commit --no-verify`
/// or `git commit -n`?
pub(super) fn command_looks_like_no_verify_commit(cmd: &str) -> bool {
    let tokens = tokenize_shell_tokens(cmd);
    if let Some(shell_script_tokens) = extract_shell_c_payload_tokens(&tokens)
        && shell_script_contains_no_verify_commit(shell_script_tokens)
    {
        return true;
    }
    tokens_contain_no_verify_commit(&tokens, |tokens| skip_command_prefix_tokens(tokens, 0))
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
            c if c.is_whitespace() => push_shell_token(&mut tokens, &mut current),
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
    if idx + 2 >= tokens.len() || !is_shell_token(tokens[idx].as_str()) {
        return None;
    }
    let shell_flag = tokens[idx + 1].as_str();
    if !shell_flag.starts_with('-') || !shell_flag.contains('c') {
        return None;
    }
    Some(&tokens[idx + 2..])
}

fn shell_script_contains_no_verify_commit(tokens: &[String]) -> bool {
    let mut script_tokens = Vec::new();
    for token in tokens {
        script_tokens.extend(tokenize_shell_tokens(token));
    }

    tokens_contain_no_verify_commit(&script_tokens, |tokens| {
        skip_command_prefix_tokens(tokens, 0)
    })
}

fn tokens_contain_no_verify_commit<F>(tokens: &[String], skip_prefix: F) -> bool
where
    F: Fn(&[String]) -> usize,
{
    let mut command_start = 0usize;

    while command_start < tokens.len() {
        let command_end = tokens[command_start..]
            .iter()
            .position(|token| is_command_separator_token(token.as_str()))
            .map_or(tokens.len(), |idx| command_start + idx);

        if command_segment_contains_no_verify_commit(
            &tokens[command_start..command_end],
            &skip_prefix,
        ) {
            return true;
        }

        command_start = command_end.saturating_add(1);
    }

    false
}

fn command_segment_contains_no_verify_commit<F>(tokens: &[String], skip_prefix: &F) -> bool
where
    F: Fn(&[String]) -> usize,
{
    if let Some(shell_script_tokens) = extract_shell_c_payload_tokens(tokens)
        && shell_script_contains_no_verify_commit(shell_script_tokens)
    {
        return true;
    }

    let idx = skip_prefix(tokens);
    if idx >= tokens.len() || !is_git_token(tokens[idx].as_str()) {
        return false;
    }
    let Some(commit_idx) = find_git_commit_subcommand(tokens, idx + 1) else {
        return false;
    };
    commit_args_include_no_verify(&tokens[commit_idx + 1..])
}

fn find_git_commit_subcommand(tokens: &[String], mut idx: usize) -> Option<usize> {
    while idx < tokens.len() {
        let current = tokens[idx].as_str();
        if current == "commit" {
            return Some(idx);
        }
        if current == "--" {
            break;
        }
        if current.starts_with('-') {
            idx += 1;
            if git_global_option_consumes_value(current) && !current.contains('=') {
                idx = consume_option_value(tokens, idx);
            }
            continue;
        }
        break;
    }

    None
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
            if commit_long_option_consumes_value(token) && !token.contains('=') {
                idx = consume_option_value(args, idx);
            }
            continue;
        }
        if token.starts_with('-') && token.len() > 1 {
            let mut chars = token[1..].chars().peekable();
            let mut consumes_value = false;
            while let Some(flag) = chars.next() {
                if flag == 'n' {
                    return true;
                }
                if commit_short_option_consumes_value(flag) {
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

fn is_command_separator_token(token: &str) -> bool {
    matches!(token, ";" | "&&" | "||" | "|" | "&")
        || token.ends_with(';')
        || token.ends_with("&&")
        || token.ends_with("||")
        || token.ends_with('|')
        || token.ends_with('&')
}

fn consume_option_value(args: &[String], mut idx: usize) -> usize {
    if idx < args.len() {
        idx += 1;
    }
    idx
}

fn commit_short_option_consumes_value(flag: char) -> bool {
    matches!(flag, 'm' | 'F' | 'c' | 'C' | 't')
}

fn commit_long_option_consumes_value(token: &str) -> bool {
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

fn is_git_token(token: &str) -> bool {
    token.eq_ignore_ascii_case("git") || token.ends_with("/git")
}

fn is_shell_token(token: &str) -> bool {
    matches!(
        token.rsplit('/').next(),
        Some("bash" | "sh" | "zsh" | "fish")
    )
}

fn skip_command_prefix_tokens(tokens: &[String], mut idx: usize) -> usize {
    while idx < tokens.len() {
        let token = tokens[idx].as_str();
        if is_env_assignment(token) {
            idx += 1;
            continue;
        }
        if command_name_is(token, "sudo") {
            idx += 1;
            idx = skip_prefixed_command_options(tokens, idx, sudo_option_consumes_value);
            continue;
        }
        if command_name_is(token, "env") {
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

fn is_env_assignment(token: &str) -> bool {
    token
        .find('=')
        .is_some_and(|eq_pos| eq_pos > 0 && !token.starts_with('-'))
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

fn git_global_option_consumes_value(token: &str) -> bool {
    matches!(
        token,
        "-c" | "-C" | "--exec-path" | "--git-dir" | "--work-tree" | "--namespace" | "--config-env"
    )
}
