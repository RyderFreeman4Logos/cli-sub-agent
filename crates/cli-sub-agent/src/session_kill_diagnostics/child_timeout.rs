//! Transcript evidence for signal exits caused by bounded child commands.

use std::{
    fs::File,
    io::{BufRead, BufReader},
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

pub(crate) use super::command_redactor::redact_command_text;

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
