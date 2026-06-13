pub(super) fn select_actionable_failure_line(text: &str) -> Option<String> {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !is_low_signal_failure_line(line))
        .collect();
    lines
        .iter()
        .rev()
        .find(|line| is_high_signal_failure_line(line))
        .or_else(|| lines.last())
        .map(|line| truncate_failure_detail(line))
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
