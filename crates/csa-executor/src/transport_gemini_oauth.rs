use csa_process::ExecutionResult;

pub(crate) const GEMINI_OAUTH_PROMPT_SUMMARY: &str =
    "gemini-cli auth failure: OAuth browser prompt detected; no tool output produced";

pub(crate) fn is_gemini_oauth_prompt_result(execution: &ExecutionResult) -> bool {
    let stdout_has_auth_text =
        execution.output.contains("authentication") || execution.output.contains("Authentication");
    let stderr_has_auth_text = execution.stderr_output.contains("authentication")
        || execution.stderr_output.contains("Authentication");
    if !stdout_has_auth_text && !stderr_has_auth_text {
        return false;
    }

    let normalized_stdout = if stdout_has_auth_text {
        normalize_gemini_prompt_text(&execution.output)
    } else {
        String::new()
    };
    let normalized_stderr = if stderr_has_auth_text {
        normalize_gemini_prompt_text(&execution.stderr_output)
    } else {
        String::new()
    };
    let combined = if normalized_stderr.is_empty() {
        normalized_stdout.clone()
    } else if normalized_stdout.is_empty() {
        normalized_stderr.clone()
    } else {
        format!("{normalized_stdout}\n{normalized_stderr}")
    };

    if !contains_gemini_oauth_prompt(&combined) {
        return false;
    }

    !combined.lines().any(|line| {
        line.contains("\"type\":\"turn.completed\"")
            || line.contains("\"type\": \"turn.completed\"")
            || line.trim() == "turn.completed"
    })
}

pub(crate) fn classify_gemini_oauth_prompt_result(execution: &mut ExecutionResult) {
    execution.exit_code = 1;
    execution.summary = GEMINI_OAUTH_PROMPT_SUMMARY.to_string();
    if execution.stderr_output.is_empty() {
        execution.stderr_output = GEMINI_OAUTH_PROMPT_SUMMARY.to_string();
    } else if !execution
        .stderr_output
        .contains(GEMINI_OAUTH_PROMPT_SUMMARY)
    {
        if !execution.stderr_output.ends_with('\n') {
            execution.stderr_output.push('\n');
        }
        execution
            .stderr_output
            .push_str(GEMINI_OAUTH_PROMPT_SUMMARY);
    }
}

pub fn contains_gemini_oauth_prompt(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("opening authentication page in your browser")
        || (lower.contains("opening authentication page")
            && lower.contains("do you want to continue"))
        || (lower.contains("authentication page in your browser")
            && lower.contains("do you want to continue"))
}

pub fn normalize_gemini_prompt_text(text: &str) -> String {
    let mut cleaned = String::new();
    let mut in_guard = false;
    for raw_line in strip_ansi_escape_sequences(text).lines() {
        let line = raw_line.trim_end_matches('\r');
        let trimmed = line.trim();
        if trimmed.starts_with("<csa-caller-sa-guard") {
            in_guard = true;
            continue;
        }
        if trimmed.starts_with("</csa-caller-sa-guard>") {
            in_guard = false;
            continue;
        }
        if trimmed.starts_with("<csa-caller-prompt-injection") {
            in_guard = true;
            continue;
        }
        if trimmed.starts_with("</csa-caller-prompt-injection>") {
            in_guard = false;
            continue;
        }
        if in_guard
            || trimmed.is_empty()
            || trimmed.starts_with("[csa-hook]")
            || trimmed.starts_with("WARNING: weave.lock")
            || trimmed.starts_with("csa run context:")
            || trimmed.starts_with("Running scope as unit:")
        {
            continue;
        }
        let stripped = trimmed.strip_prefix("[stdout] ").unwrap_or(trimmed);
        cleaned.push_str(stripped);
        cleaned.push('\n');
    }
    cleaned
}

pub fn strip_ansi_escape_sequences(text: &str) -> String {
    let mut stripped = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\u{1b}' {
            stripped.push(ch);
            continue;
        }
        if !matches!(chars.peek(), Some('[')) {
            continue;
        }
        let _ = chars.next();
        for next in chars.by_ref() {
            if ('@'..='~').contains(&next) {
                break;
            }
        }
    }
    stripped
}