//! Transcript evidence for signal exits caused by bounded child commands.

use std::{
    fs::File,
    io::{BufRead, BufReader},
    ops::Range,
    path::Path,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChildTimeoutKind {
    BoundedCommand,
    HookEnabledGitCommit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ChildTimeoutProvenance {
    pub(crate) command: String,
    pub(crate) timeout_seconds: Option<u64>,
    pub(crate) kind: ChildTimeoutKind,
    pub(crate) command_status: Option<String>,
    pub(crate) transcript_exit_143: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ObservedChildCommand {
    command: String,
    status: Option<String>,
    exit_code: Option<i64>,
    transcript_exit_143: bool,
}

pub(crate) fn detect_child_timeout_provenance(
    session_dir: &Path,
    exit_code: i32,
) -> Option<ChildTimeoutProvenance> {
    if exit_code != 143 {
        return None;
    }

    let output_log = session_dir.join("output.log");
    let file = File::open(output_log).ok()?;
    let reader = BufReader::new(file);
    let mut last_command: Option<ObservedChildCommand> = None;

    for line_result in reader.lines() {
        let Ok(line) = line_result else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
            mark_exit_143_after_last_command(&line, &mut last_command);
            continue;
        };
        if let Some(command) = observed_child_command(&value) {
            last_command = Some(command);
        }
        mark_exit_143_after_last_command(&line, &mut last_command);
    }

    let observed = last_command?;
    if !observed.has_active_timeout_evidence() {
        return None;
    }
    let timeout_seconds = timeout_duration_seconds(&observed.command)?;
    let kind = if invokes_hook_enabled_git_commit(&observed.command) {
        ChildTimeoutKind::HookEnabledGitCommit
    } else {
        ChildTimeoutKind::BoundedCommand
    };

    let redacted_command = redact_command_text(&observed.command);
    Some(ChildTimeoutProvenance {
        command: truncate_one_line(&redacted_command, 300),
        timeout_seconds: Some(timeout_seconds),
        kind,
        command_status: observed.status,
        transcript_exit_143: observed.transcript_exit_143 || observed.exit_code == Some(143),
    })
}

pub(crate) fn redact_command_text(command: &str) -> String {
    let redacted = redact_shell_credential_options(command);
    csa_session::redact_text_content(&redacted)
}

#[derive(Debug, Clone)]
struct CommandToken {
    text: String,
    start: usize,
    end: usize,
    boundary_before: bool,
    quote_id: Option<usize>,
    quote_started_before: bool,
}

#[derive(Debug, Clone)]
struct CommandRedaction {
    range: Range<usize>,
    next_index: usize,
}

#[derive(Debug, Clone, Copy)]
enum RedactionValueKind {
    Header,
    Credential,
}

fn redact_shell_credential_options(command: &str) -> String {
    let tokens = command_tokens(command);
    let mut redactions = Vec::new();
    let mut index = 0;

    while index < tokens.len() {
        if let Some(redaction) = header_option_redaction(&tokens, index) {
            index = redaction.next_index;
            redactions.push(redaction.range);
            continue;
        }
        if let Some(redaction) = user_option_redaction(&tokens, index) {
            index = redaction.next_index;
            redactions.push(redaction.range);
            continue;
        }
        if let Some(redaction) = sensitive_option_redaction(&tokens, index) {
            index = redaction.next_index;
            redactions.push(redaction.range);
            continue;
        }
        index += 1;
    }

    apply_redactions(command, redactions)
}

fn header_option_redaction(tokens: &[CommandToken], index: usize) -> Option<CommandRedaction> {
    let token = &tokens[index];
    let text = token.text.as_str();

    if text == "-H" || text.eq_ignore_ascii_case("--header") {
        let value_index = following_value_index(tokens, index)?;
        return Some(value_redaction(
            tokens,
            value_index,
            tokens[value_index].start,
            tokens[value_index].text.as_str(),
            RedactionValueKind::Header,
        ));
    }

    if let Some(offset) = short_header_value_offset(text) {
        return inline_value_redaction(tokens, index, offset, RedactionValueKind::Header);
    }

    if let Some(offset) = long_inline_value_offset(text, "--header") {
        return inline_value_redaction(tokens, index, offset, RedactionValueKind::Header);
    }

    None
}

fn user_option_redaction(tokens: &[CommandToken], index: usize) -> Option<CommandRedaction> {
    let token = &tokens[index];
    let text = token.text.as_str();

    if text.eq_ignore_ascii_case("--user") || text.eq_ignore_ascii_case("--proxy-user") {
        let value_index = following_value_index(tokens, index)?;
        return Some(value_redaction(
            tokens,
            value_index,
            tokens[value_index].start,
            tokens[value_index].text.as_str(),
            RedactionValueKind::Credential,
        ));
    }

    for option_name in ["--user", "--proxy-user"] {
        if let Some(offset) = long_inline_value_offset(text, option_name) {
            return inline_value_redaction(tokens, index, offset, RedactionValueKind::Credential);
        }
    }

    if text == "-u" {
        let value_index = following_value_index(tokens, index)?;
        return Some(value_redaction(
            tokens,
            value_index,
            tokens[value_index].start,
            tokens[value_index].text.as_str(),
            RedactionValueKind::Credential,
        ));
    }

    if let Some(offset) = short_user_value_offset(text) {
        return inline_value_redaction(tokens, index, offset, RedactionValueKind::Credential);
    }

    None
}

fn sensitive_option_redaction(tokens: &[CommandToken], index: usize) -> Option<CommandRedaction> {
    let token = &tokens[index];
    let text = token.text.as_str();
    if !text.starts_with('-') || text == "-" {
        return None;
    }

    if let Some(separator_index) = text.find(['=', ':']) {
        let option_name = &text[..separator_index];
        if is_sensitive_option_name(option_name) {
            return inline_value_redaction(
                tokens,
                index,
                separator_index + 1,
                RedactionValueKind::Credential,
            );
        }
        return None;
    }

    if !is_sensitive_option_name(text) {
        return None;
    }
    let value_index = following_value_index(tokens, index)?;
    Some(value_redaction(
        tokens,
        value_index,
        tokens[value_index].start,
        tokens[value_index].text.as_str(),
        RedactionValueKind::Credential,
    ))
}

fn inline_value_redaction(
    tokens: &[CommandToken],
    index: usize,
    offset: usize,
    kind: RedactionValueKind,
) -> Option<CommandRedaction> {
    let token = &tokens[index];
    if offset < token.text.len() {
        return Some(value_redaction(
            tokens,
            index,
            token.start + offset,
            &token.text[offset..],
            kind,
        ));
    }

    let value_index = following_value_index(tokens, index)?;
    Some(value_redaction(
        tokens,
        value_index,
        tokens[value_index].start,
        tokens[value_index].text.as_str(),
        kind,
    ))
}

fn value_redaction(
    tokens: &[CommandToken],
    value_index: usize,
    value_start: usize,
    value_text: &str,
    kind: RedactionValueKind,
) -> CommandRedaction {
    if !value_text.is_empty()
        && value_text.chars().all(|ch| ch == '\\')
        && let Some(last_value_index) = escaped_quoted_value_last_token(tokens, value_index)
    {
        return CommandRedaction {
            range: value_start..tokens[last_value_index].end,
            next_index: last_value_index + 1,
        };
    }

    let starts_at_token_start = value_start == tokens[value_index].start;
    let last_value_index = match kind {
        RedactionValueKind::Header => {
            header_value_last_token(tokens, value_index, value_text, starts_at_token_start)
        }
        RedactionValueKind::Credential => {
            quoted_value_last_token(tokens, value_index, starts_at_token_start)
                .unwrap_or(value_index)
        }
    };

    CommandRedaction {
        range: value_start..tokens[last_value_index].end,
        next_index: last_value_index + 1,
    }
}

fn header_value_last_token(
    tokens: &[CommandToken],
    value_index: usize,
    value_text: &str,
    starts_at_token_start: bool,
) -> usize {
    if let Some(last_index) = quoted_value_last_token(tokens, value_index, starts_at_token_start) {
        return last_index;
    }

    let mut last_index = value_index;
    let Some((name, after_colon)) = value_text.trim().split_once(':') else {
        return last_index;
    };
    let after_colon = after_colon.trim_start();

    if header_name_is_authorization(name) {
        if after_colon.is_empty() {
            if let Some(next_index) = next_header_value_token(tokens, last_index, true) {
                last_index = next_index;
                if is_auth_scheme(tokens[last_index].text.as_str())
                    && let Some(secret_index) = next_header_value_token(tokens, last_index, true)
                {
                    last_index = secret_index;
                }
            }
        } else if is_auth_scheme(after_colon)
            && let Some(secret_index) = next_header_value_token(tokens, last_index, true)
        {
            last_index = secret_index;
        }
    } else if after_colon.is_empty() {
        let allow_option_like_token = header_name_is_credential_bearing(name);
        while let Some(next_index) =
            next_header_value_token(tokens, last_index, allow_option_like_token)
        {
            last_index = next_index;
        }
    }

    last_index
}

fn escaped_quoted_value_last_token(tokens: &[CommandToken], value_index: usize) -> Option<usize> {
    let quoted_value_index = value_index + 1;
    if quoted_value_index >= tokens.len() || tokens[quoted_value_index].boundary_before {
        return None;
    }
    quoted_value_last_token(tokens, quoted_value_index, true)
}

fn quoted_value_last_token(
    tokens: &[CommandToken],
    value_index: usize,
    starts_at_token_start: bool,
) -> Option<usize> {
    if !starts_at_token_start || !tokens[value_index].quote_started_before {
        return None;
    }
    let quote_id = tokens[value_index].quote_id?;
    let mut last_index = value_index;
    while last_index + 1 < tokens.len() && tokens[last_index + 1].quote_id == Some(quote_id) {
        last_index += 1;
    }
    Some(last_index)
}

fn following_value_index(tokens: &[CommandToken], option_index: usize) -> Option<usize> {
    let mut value_index = option_index + 1;
    if value_index >= tokens.len() || tokens[value_index].boundary_before {
        return None;
    }
    if matches!(tokens[value_index].text.as_str(), "=" | ":") {
        value_index += 1;
        if value_index >= tokens.len() || tokens[value_index].boundary_before {
            return None;
        }
    }
    Some(value_index)
}

fn next_header_value_token(
    tokens: &[CommandToken],
    current_index: usize,
    allow_option_like_token: bool,
) -> Option<usize> {
    let next_index = current_index + 1;
    if next_index >= tokens.len() || tokens[next_index].boundary_before {
        return None;
    }
    let next_text = tokens[next_index].text.as_str();
    if looks_like_url(next_text) || (!allow_option_like_token && looks_like_option(next_text)) {
        return None;
    }
    Some(next_index)
}

fn short_header_value_offset(text: &str) -> Option<usize> {
    if text == "-H" {
        None
    } else if text.starts_with("-H=") || text.starts_with("-H:") {
        Some(3)
    } else if text.starts_with("-H") {
        Some(2)
    } else {
        None
    }
}

fn short_user_value_offset(text: &str) -> Option<usize> {
    if text == "-u" {
        None
    } else if text.starts_with("-u=") || text.starts_with("-u:") {
        Some(3)
    } else if text.starts_with("-u") {
        Some(2)
    } else {
        None
    }
}

fn long_inline_value_offset(text: &str, option_name: &str) -> Option<usize> {
    if text.len() <= option_name.len() {
        return None;
    }
    let (candidate, rest) = text.split_at(option_name.len());
    if candidate.eq_ignore_ascii_case(option_name)
        && (rest.starts_with('=') || rest.starts_with(':'))
    {
        Some(option_name.len() + 1)
    } else {
        None
    }
}

fn is_sensitive_option_name(name: &str) -> bool {
    let normalized = normalize_credential_name(name.trim_start_matches('-'));
    normalized.contains("apikey")
        || normalized.contains("auth")
        || normalized.contains("bearer")
        || normalized.contains("password")
        || normalized.contains("passwd")
        || normalized.contains("pwd")
        || normalized.contains("secret")
        || normalized.contains("token")
}

fn header_name_is_authorization(name: &str) -> bool {
    normalize_credential_name(name).contains("authorization")
}

fn header_name_is_credential_bearing(name: &str) -> bool {
    let normalized = normalize_credential_name(name);
    normalized.contains("authorization")
        || normalized.contains("apikey")
        || normalized.contains("auth")
        || normalized.contains("password")
        || normalized.contains("secret")
        || normalized.contains("token")
}

fn normalize_credential_name(name: &str) -> String {
    name.chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn is_auth_scheme(value: &str) -> bool {
    let normalized = value.trim_matches(|ch: char| !ch.is_ascii_alphanumeric());
    normalized.eq_ignore_ascii_case("basic")
        || normalized.eq_ignore_ascii_case("bearer")
        || normalized.eq_ignore_ascii_case("digest")
        || normalized.eq_ignore_ascii_case("negotiate")
        || normalized.eq_ignore_ascii_case("token")
}

fn looks_like_option(value: &str) -> bool {
    value.starts_with('-') && value != "-"
}

fn looks_like_url(value: &str) -> bool {
    value.contains("://")
}

fn command_tokens(command: &str) -> Vec<CommandToken> {
    let mut tokens = Vec::new();
    let mut token_start = None;
    let mut token_text = String::new();
    let mut token_boundary_before = false;
    let mut token_quote_id = None;
    let mut token_quote_started_before = false;
    let mut pending_boundary = false;
    let mut pending_quote_start = None;
    let mut quote_stack = Vec::new();
    let mut next_quote_id = 0;

    for (index, ch) in command.char_indices() {
        if matches!(ch, '\'' | '"') {
            push_command_token(
                &mut tokens,
                &mut token_start,
                &mut token_text,
                token_boundary_before,
                token_quote_id,
                token_quote_started_before,
                index,
            );
            if quote_stack.last().is_some_and(|(quote, _)| *quote == ch) {
                let (_, quote_id) = quote_stack.pop().expect("quote stack should have top");
                if pending_quote_start == Some(quote_id) {
                    pending_quote_start = None;
                }
            } else {
                let quote_id = next_quote_id;
                next_quote_id += 1;
                quote_stack.push((ch, quote_id));
                pending_quote_start = Some(quote_id);
            }
            continue;
        }

        if ch.is_whitespace() {
            push_command_token(
                &mut tokens,
                &mut token_start,
                &mut token_text,
                token_boundary_before,
                token_quote_id,
                token_quote_started_before,
                index,
            );
            continue;
        }

        if is_shell_control(ch) {
            push_command_token(
                &mut tokens,
                &mut token_start,
                &mut token_text,
                token_boundary_before,
                token_quote_id,
                token_quote_started_before,
                index,
            );
            pending_boundary = true;
            continue;
        }

        if token_start.is_none() {
            token_start = Some(index);
            token_boundary_before = pending_boundary;
            pending_boundary = false;
            token_quote_id = quote_stack.last().map(|(_, quote_id)| *quote_id);
            token_quote_started_before =
                token_quote_id.is_some() && pending_quote_start == token_quote_id;
            if token_quote_started_before {
                pending_quote_start = None;
            }
        }
        token_text.push(ch);
    }

    push_command_token(
        &mut tokens,
        &mut token_start,
        &mut token_text,
        token_boundary_before,
        token_quote_id,
        token_quote_started_before,
        command.len(),
    );
    tokens
}

fn push_command_token(
    tokens: &mut Vec<CommandToken>,
    token_start: &mut Option<usize>,
    token_text: &mut String,
    boundary_before: bool,
    quote_id: Option<usize>,
    quote_started_before: bool,
    end: usize,
) {
    let Some(start) = token_start.take() else {
        return;
    };
    tokens.push(CommandToken {
        text: std::mem::take(token_text),
        start,
        end,
        boundary_before,
        quote_id,
        quote_started_before,
    });
}

fn is_shell_control(ch: char) -> bool {
    matches!(ch, ';' | '(' | ')' | '&' | '|')
}

fn apply_redactions(command: &str, mut redactions: Vec<Range<usize>>) -> String {
    redactions.retain(|range| range.start < range.end && range.end <= command.len());
    if redactions.is_empty() {
        return command.to_string();
    }

    redactions.sort_by_key(|range| (range.start, range.end));
    let mut merged: Vec<Range<usize>> = Vec::new();
    for range in redactions {
        if let Some(last) = merged.last_mut()
            && range.start <= last.end
        {
            last.end = last.end.max(range.end);
            continue;
        }
        merged.push(range);
    }

    let mut redacted = String::with_capacity(command.len());
    let mut cursor = 0;
    for range in merged {
        redacted.push_str(&command[cursor..range.start]);
        redacted.push_str("[REDACTED]");
        cursor = range.end;
    }
    redacted.push_str(&command[cursor..]);
    redacted
}

fn mark_exit_143_after_last_command(line: &str, last_command: &mut Option<ObservedChildCommand>) {
    if mentions_exit_143(line)
        && let Some(command) = last_command
    {
        command.transcript_exit_143 = true;
    }
}

fn observed_child_command(value: &serde_json::Value) -> Option<ObservedChildCommand> {
    let item = value.get("item")?;
    if item.get("type").and_then(serde_json::Value::as_str) != Some("command_execution") {
        return None;
    }
    let command = item
        .get("command")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|command| !command.is_empty())?;
    let status = item
        .get("status")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|status| !status.is_empty())
        .map(ToOwned::to_owned);
    let exit_code = item.get("exit_code").and_then(serde_json::Value::as_i64);
    Some(ObservedChildCommand {
        command: command.to_string(),
        status,
        exit_code,
        transcript_exit_143: exit_code == Some(143),
    })
}

impl ObservedChildCommand {
    fn has_active_timeout_evidence(&self) -> bool {
        self.transcript_exit_143
            || self.exit_code == Some(143)
            || self.status.as_deref() == Some("in_progress")
    }
}

fn mentions_exit_143(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    lower.contains("exit code 143")
        || lower.contains("exited with code 143")
        || lower.contains("terminated with code 143")
}

fn timeout_duration_seconds(command: &str) -> Option<u64> {
    let tokens = shellish_tokens(command);
    for (index, token) in tokens.iter().enumerate() {
        if !basename_is(token, "timeout") {
            continue;
        }
        let mut cursor = index + 1;
        while cursor < tokens.len() {
            let token = tokens[cursor].as_str();
            if timeout_option_consumes_next(token) {
                cursor += 2;
                continue;
            }
            if timeout_option_without_value(token) {
                cursor += 1;
                continue;
            }
            return parse_duration_seconds(token);
        }
    }
    None
}

fn invokes_hook_enabled_git_commit(command: &str) -> bool {
    if shellish_tokens(command)
        .iter()
        .any(|token| token == "--no-verify")
    {
        return false;
    }
    shellish_tokens(command)
        .windows(2)
        .any(|window| basename_is(&window[0], "git") && window[1] == "commit")
}

fn shellish_tokens(command: &str) -> Vec<String> {
    command
        .split(|ch: char| {
            ch.is_whitespace() || matches!(ch, '\'' | '"' | ';' | '(' | ')' | '&' | '|')
        })
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn basename_is(token: &str, expected: &str) -> bool {
    token.rsplit('/').next() == Some(expected)
}

fn timeout_option_consumes_next(token: &str) -> bool {
    matches!(token, "-k" | "--kill-after" | "-s" | "--signal")
}

fn timeout_option_without_value(token: &str) -> bool {
    token == "--foreground"
        || token == "--preserve-status"
        || token == "--verbose"
        || token.starts_with("--kill-after=")
        || token.starts_with("--signal=")
        || token.starts_with("-k")
        || token.starts_with("-s")
}

fn parse_duration_seconds(raw: &str) -> Option<u64> {
    let digit_len = raw
        .as_bytes()
        .iter()
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    if digit_len == 0 {
        return None;
    }
    let value = raw[..digit_len].parse::<u64>().ok()?;
    match &raw[digit_len..] {
        "" | "s" => Some(value),
        "m" => value.checked_mul(60),
        "h" => value.checked_mul(60 * 60),
        "d" => value.checked_mul(24 * 60 * 60),
        _ => None,
    }
}

pub(crate) fn truncate_one_line(value: &str, max_chars: usize) -> String {
    let one_line = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() <= max_chars {
        return one_line;
    }
    let mut truncated = one_line
        .chars()
        .take(max_chars.saturating_sub(3))
        .collect::<String>();
    truncated.push_str("...");
    truncated
}
