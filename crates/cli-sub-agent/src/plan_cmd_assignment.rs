use std::collections::HashSet;

use weave::compiler::PlanStep;

use super::validate_variable_name;

const OUTPUT_ASSIGNMENT_MARKER_PREFIX: &str = "CSA_VAR:";

/// Remove `CSA_VAR:` lines from step output so `STEP_<id>_OUTPUT` keeps only logical output.
pub(crate) fn strip_assignment_marker_lines(output: &str) -> String {
    output
        .lines()
        .filter(|line| !line.trim().starts_with(OUTPUT_ASSIGNMENT_MARKER_PREFIX))
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn extract_output_assignment_markers(
    output: &str,
    allowlist: &HashSet<String>,
) -> Vec<(String, String)> {
    let mut markers = Vec::new();
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let marker_payload = match trimmed.strip_prefix(OUTPUT_ASSIGNMENT_MARKER_PREFIX) {
            Some(payload) => payload.trim(),
            None => continue,
        };
        if let Some((raw_key, raw_value)) = marker_payload.split_once('=') {
            let key = raw_key.trim();
            if is_assignment_marker_key(key) && allowlist.contains(key) {
                markers.push((key.to_string(), raw_value.trim().to_string()));
            }
        }
    }
    markers
}

pub(crate) fn should_inject_assignment_markers(step: &PlanStep) -> bool {
    step.tool
        .as_deref()
        .is_some_and(|tool| tool.eq_ignore_ascii_case("bash"))
}

pub(crate) fn is_assignment_marker_key(key: &str) -> bool {
    validate_variable_name(key).is_ok()
}
