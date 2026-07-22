//! Shell-aware credential redaction for diagnostic command text.
//!
//! UTF-8 safe: multi-byte just parser markers such as `▶` must never panic
//! byte-index option matching or redaction slicing.

use std::ops::Range;

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
    // Offset is expected to land on a UTF-8 boundary (ASCII option prefixes /
    // separator indices). Refuse mid-scalar offsets so multi-byte diagnostic
    // markers never panic the sanitizer.
    if offset < token.text.len() && token.text.is_char_boundary(offset) {
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
    // option_name is ASCII (e.g. "--header"), but the token may be a multi-byte
    // just parser marker such as "——▶▶▶". split_at(option_name.len()) panics
    // when that byte index lands inside a scalar; refuse non-boundaries.
    if !text.is_char_boundary(option_name.len()) {
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
    // Snap ranges onto UTF-8 scalar boundaries so a buggy offset can never panic
    // valid multi-byte diagnostic text (e.g. just parser "▶" markers).
    redactions.retain_mut(|range| {
        if range.end > command.len() {
            range.end = command.len();
        }
        range.start = floor_char_boundary(command, range.start);
        range.end = ceil_char_boundary(command, range.end);
        range.start < range.end
    });
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

/// Last valid UTF-8 char boundary at or before `index` (stable stand-in for
/// `str::floor_char_boundary`).
fn floor_char_boundary(s: &str, mut index: usize) -> usize {
    if index >= s.len() {
        return s.len();
    }
    while index > 0 && !s.is_char_boundary(index) {
        index -= 1;
    }
    index
}

/// First valid UTF-8 char boundary at or after `index` (stable stand-in for
/// `str::ceil_char_boundary`).
fn ceil_char_boundary(s: &str, mut index: usize) -> usize {
    if index >= s.len() {
        return s.len();
    }
    while index < s.len() && !s.is_char_boundary(index) {
        index += 1;
    }
    index
}
