//! Timeout, interruption, and resume helpers for `csa run`.
//!
//! Extracted from `run_cmd.rs` to keep module sizes manageable.

use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::Result;
use csa_core::types::{OutputFormat, ToolName};

pub(super) const DEFAULT_PR_BOT_TIMEOUT_SECS: u64 = 2400;
const RUN_TIMEOUT_EXIT_CODE: i32 = 124;

pub(crate) fn resolve_run_timeout_seconds(
    cli_timeout: Option<u64>,
    skill: Option<&str>,
) -> Option<u64> {
    if cli_timeout.is_some() {
        return cli_timeout;
    }
    if matches!(skill, Some("pr-bot")) {
        return Some(DEFAULT_PR_BOT_TIMEOUT_SECS);
    }
    None
}

pub(crate) fn resolve_remaining_run_timeout(
    run_timeout_seconds: Option<u64>,
    run_started_at: Instant,
) -> Option<Duration> {
    run_timeout_seconds
        .map(|seconds| Duration::from_secs(seconds).saturating_sub(run_started_at.elapsed()))
}

pub(crate) fn emit_run_timeout(
    output_format: OutputFormat,
    timeout_seconds: u64,
    tool: ToolName,
    skill: Option<&str>,
    session_id: Option<&str>,
) -> Result<i32> {
    let message =
        format!("csa run exceeded wall-clock timeout ({timeout_seconds}s); execution terminated");
    match output_format {
        OutputFormat::Text => {
            if let Some(sid) = session_id {
                let resume_hint = build_resume_hint_command(sid, tool, skill);
                eprintln!("{message}. Resume with:\n  {resume_hint}");
            } else {
                eprintln!("{message}.");
            }
        }
        OutputFormat::Json => {
            let resume_hint = session_id.map(|sid| build_resume_hint_command(sid, tool, skill));
            let payload = serde_json::json!({
                "error": "timeout",
                "exit_code": RUN_TIMEOUT_EXIT_CODE,
                "timeout_seconds": timeout_seconds,
                "session_id": session_id,
                "resume_hint": resume_hint,
                "message": message,
            });
            println!("{}", serde_json::to_string_pretty(&payload)?);
        }
    }
    Ok(RUN_TIMEOUT_EXIT_CODE)
}

pub(crate) fn detect_effective_repo(project_root: &Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["config", "--get", "remote.origin.url"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let raw = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if raw.is_empty() {
        return None;
    }

    let sanitized = if let Some(pos) = raw.find("://") {
        let (scheme, rest) = raw.split_at(pos + 3);
        if let Some(at_pos) = rest.find('@') {
            format!("{}{}", scheme, &rest[at_pos + 1..])
        } else {
            raw
        }
    } else {
        raw
    };

    let trimmed = sanitized.trim_end_matches(".git");
    if let Some(rest) = trimmed.strip_prefix("git@github.com:") {
        return Some(rest.to_string());
    }
    if let Some(rest) = trimmed.strip_prefix("https://github.com/") {
        return Some(rest.to_string());
    }
    if let Some(rest) = trimmed.strip_prefix("ssh://git@github.com/") {
        return Some(rest.to_string());
    }
    Some(trimmed.to_string())
}

pub(crate) fn signal_interruption_exit_code(error: &anyhow::Error) -> Option<i32> {
    for cause in error.chain() {
        let message = cause.to_string().to_ascii_lowercase();
        if message.contains("sigterm") {
            return Some(143);
        }
        if message.contains("sigint") {
            return Some(130);
        }
    }
    None
}

pub(crate) fn wall_timeout_seconds_from_error(error: &anyhow::Error) -> Option<u64> {
    const MARKER: &str = "WALL_TIMEOUT timeout_secs=";
    for cause in error.chain() {
        let message = cause.to_string();
        let Some(idx) = message.find(MARKER) else {
            continue;
        };
        let suffix = &message[idx + MARKER.len()..];
        let digits: String = suffix
            .chars()
            .take_while(|ch| ch.is_ascii_digit())
            .collect();
        if let Ok(value) = digits.parse::<u64>()
            && value > 0
        {
            return Some(value);
        }
    }
    None
}

pub(crate) fn run_error_timeout_seconds(
    error: &anyhow::Error,
    _configured_run_timeout_seconds: Option<u64>,
) -> Option<u64> {
    // A configured wall-clock budget constrains execution, but it must not
    // reclassify unrelated pre-exec or transport errors as timeout.
    wall_timeout_seconds_from_error(error)
}

pub(crate) fn signal_name_from_exit_code(exit_code: i32) -> &'static str {
    match exit_code {
        143 => "SIGTERM",
        130 => "SIGINT",
        _ => "signal",
    }
}

pub(crate) fn extract_meta_session_id_from_error(error: &anyhow::Error) -> Option<String> {
    const MARKER: &str = "meta_session_id=";
    for cause in error.chain() {
        let message = cause.to_string();
        let Some(idx) = message.find(MARKER) else {
            continue;
        };
        let suffix = &message[idx + MARKER.len()..];
        let session_id: String = suffix
            .chars()
            .take_while(|ch| ch.is_ascii_alphanumeric())
            .collect();
        if !session_id.is_empty() {
            return Some(session_id);
        }
    }
    None
}

pub(crate) fn build_resume_hint_command(
    session_id: &str,
    tool: ToolName,
    skill: Option<&str>,
) -> String {
    match skill {
        Some(skill_name) => format!(
            "csa run --session {} --tool {} --skill {}",
            session_id,
            tool.as_str(),
            skill_name
        ),
        None => format!(
            "csa run --session {} --tool {} <same prompt>",
            session_id,
            tool.as_str()
        ),
    }
}

pub(crate) fn skill_session_description(skill_name: &str) -> String {
    format!("skill:{skill_name}")
}

pub(crate) fn session_matches_interrupted_skill(
    session: &csa_session::MetaSessionState,
    skill_name: &str,
) -> bool {
    let expected = skill_session_description(skill_name);
    let description_matches = session.description.as_deref() == Some(expected.as_str());
    let terminated_by_signal = matches!(
        session.termination_reason.as_deref(),
        Some("sigterm" | "sigint")
    );
    description_matches && terminated_by_signal
}

pub(crate) fn find_recent_interrupted_skill_session(
    project_root: &Path,
    skill_name: &str,
    tool: &ToolName,
) -> Option<String> {
    let sessions = csa_session::find_sessions(
        project_root,
        None,
        Some("run"),
        None,
        Some(&[tool.as_str()]),
    )
    .ok()?;

    for session in sessions {
        if !session_matches_interrupted_skill(&session, skill_name) {
            continue;
        }

        match csa_session::load_result(project_root, &session.meta_session_id) {
            Ok(Some(result))
                if result.status == "interrupted"
                    || result.exit_code == 130
                    || result.exit_code == 143 =>
            {
                return Some(session.meta_session_id.clone());
            }
            Ok(None) => return Some(session.meta_session_id.clone()),
            Ok(Some(_)) => continue,
            Err(_) => return Some(session.meta_session_id.clone()),
        }
    }

    None
}
