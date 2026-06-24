pub(super) fn select_actionable_failure_line(text: &str) -> Option<String> {
    let all_lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    if let Some(detail) = select_todo_persist_failure_detail(&all_lines) {
        return Some(detail);
    }
    if let Some(detail) = select_todo_validation_failure_detail(&all_lines) {
        return Some(detail);
    }
    let lines: Vec<&str> = all_lines
        .into_iter()
        .filter(|line| !is_low_signal_failure_line(line))
        .collect();
    lines
        .iter()
        .rev()
        .find(|line| is_high_signal_failure_line(line))
        .or_else(|| lines.last())
        .map(|line| truncate_failure_detail(line))
}

fn select_todo_persist_failure_detail(lines: &[&str]) -> Option<String> {
    let persist_line = lines
        .iter()
        .rev()
        .copied()
        .find(|line| is_todo_persist_wrapper(line))?;

    let diagnostic = lines
        .iter()
        .rev()
        .copied()
        .find(|line| is_todo_persist_diagnostic(line));

    let mut parts = Vec::new();
    if let Some(diagnostic) = diagnostic {
        parts.push(diagnostic);
    }
    if !parts.contains(&persist_line) {
        parts.push(persist_line);
    }
    for line in prioritized_todo_context_lines(lines) {
        if !parts.contains(&line) {
            parts.push(line);
        }
    }

    Some(truncate_failure_detail(&parts.join(" | ")))
}

fn select_todo_validation_failure_detail(lines: &[&str]) -> Option<String> {
    let diagnostic = lines
        .iter()
        .rev()
        .copied()
        .find(|line| is_todo_validation_diagnostic(line))?;
    let mut parts = vec![diagnostic];
    for line in prioritized_todo_context_lines(lines) {
        if !parts.contains(&line) {
            parts.push(line);
        }
    }
    Some(truncate_failure_detail(&parts.join(" | ")))
}

fn prioritized_todo_context_lines<'a>(lines: &'a [&str]) -> Vec<&'a str> {
    let mut prioritized = Vec::new();
    for predicate in [
        is_spec_artifact_path as fn(&str) -> bool,
        is_raw_spec_artifact_path,
        is_first_marker_kind,
        is_todo_artifact_path,
        is_persist_stderr_artifact,
    ] {
        for line in lines.iter().copied().filter(|line| predicate(line)) {
            if !prioritized.contains(&line) {
                prioritized.push(line);
            }
        }
    }
    prioritized
}

fn is_spec_artifact_path(line: &str) -> bool {
    line.starts_with("Spec artifact path:")
}

fn is_raw_spec_artifact_path(line: &str) -> bool {
    line.starts_with("Raw spec artifact path:")
}

fn is_first_marker_kind(line: &str) -> bool {
    line.contains("first marker kind:")
}

fn is_todo_artifact_path(line: &str) -> bool {
    line.starts_with("TODO artifact path:")
}

fn is_persist_stderr_artifact(line: &str) -> bool {
    line.starts_with("Persist stderr artifact:")
}

fn is_todo_persist_wrapper(line: &str) -> bool {
    line == "csa todo persist failed" || line.starts_with("csa todo persist failed ")
}

fn is_todo_persist_diagnostic(line: &str) -> bool {
    !is_todo_persist_wrapper(line) && is_todo_validation_diagnostic(line)
}

fn is_todo_validation_diagnostic(line: &str) -> bool {
    let normalized = line.to_ascii_lowercase();
    normalized.contains("failed to parse spec file")
        || normalized.contains("toml parse error")
        || normalized.contains("spec artifact-shape error")
        || normalized.contains("bad spec artifact")
        || normalized.contains("spec artifact is empty")
        || normalized.contains("todo artifact is empty")
        || normalized.contains("todo artifact has")
        || normalized.contains("generated todo")
        || normalized.contains("generated spec")
        || normalized.contains("spec plan_ulid")
        || normalized.contains("without a mechanically-verifiable")
        || normalized.contains("invalid criterion")
        || normalized.contains("no non-empty checkbox tasks")
        || normalized.contains("summary lacks han")
        || normalized.contains("todo han chars")
        || normalized.contains("todo cjk chars")
        || normalized.contains("step_8_output is empty")
        || normalized.contains("neither step_12_output")
        || normalized.contains("csa todo create failed")
}

fn is_low_signal_failure_line(line: &str) -> bool {
    line.starts_with("✓ PASS")
        || line.starts_with("- SKIP")
        || line.starts_with("✗ FAIL")
        || line.starts_with("Status:")
        || line.starts_with("Summary:")
        || line.starts_with("Failed step:")
        || line.starts_with("Artifacts:")
        || line.starts_with("Passed:")
        || line.starts_with("Total:")
        || line.starts_with("Duration:")
        || line.starts_with("Exit code ")
        || (line.starts_with("Error: ") && line.contains(" step(s) failed"))
        || line.starts_with("stderr (last ")
        || line.starts_with("stdout (last ")
        || line == "```text"
        || line == "```"
}

fn is_high_signal_failure_line(line: &str) -> bool {
    let normalized = line.to_ascii_lowercase();
    line.starts_with("ERROR:")
        || line.starts_with("Error:")
        || normalized.contains("unexpected eof")
        || normalized.contains("quota")
        || normalized.contains("timeout")
        || normalized.contains("failed")
}

fn truncate_failure_detail(line: &str) -> String {
    const MAX_FAILURE_DETAIL_CHARS: usize = 240;
    let mut chars = line.chars();
    let truncated: String = chars.by_ref().take(MAX_FAILURE_DETAIL_CHARS).collect();
    if chars.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
    }
}
