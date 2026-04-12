use super::{BugClassCandidate, CaseStudy};

const SKILL_SHORT_FIELD_MAX_CHARS: usize = 200;
const SKILL_TEXT_MAX_CHARS: usize = 500;
const SKILL_CODE_MAX_CHARS: usize = 2000;
pub(crate) const SANITIZED_CONTENT_PLACEHOLDER: &str = "Content removed due to sanitization.";

pub(crate) fn sanitize_candidate_for_skill(candidate: &BugClassCandidate) -> BugClassCandidate {
    BugClassCandidate {
        language: candidate.language.clone(),
        domain: sanitize_optional_text(candidate.domain.as_deref(), SKILL_SHORT_FIELD_MAX_CHARS),
        rule_id: sanitize_optional_text(candidate.rule_id.as_deref(), SKILL_SHORT_FIELD_MAX_CHARS),
        anti_pattern_category: sanitize_text_for_skill(
            &candidate.anti_pattern_category,
            SKILL_SHORT_FIELD_MAX_CHARS,
        )
        .if_empty_then("uncategorized"),
        preferred_pattern: sanitize_text_for_skill(
            &candidate.preferred_pattern,
            SKILL_TEXT_MAX_CHARS,
        )
        .if_empty_then(SANITIZED_CONTENT_PLACEHOLDER),
        case_studies: candidate
            .case_studies
            .iter()
            .map(sanitize_case_study_for_skill)
            .collect(),
        recurrence_count: candidate.recurrence_count,
    }
}

fn sanitize_case_study_for_skill(case_study: &CaseStudy) -> CaseStudy {
    CaseStudy {
        session_id: case_study.session_id.clone(),
        file_path: sanitize_text_for_skill(&case_study.file_path, SKILL_SHORT_FIELD_MAX_CHARS)
            .if_empty_then("unknown"),
        line_range: case_study.line_range,
        code_snippet: case_study
            .code_snippet
            .as_deref()
            .map(sanitize_code_for_skill)
            .filter(|snippet| !snippet.is_empty()),
        fix_description: sanitize_text_for_skill(&case_study.fix_description, SKILL_TEXT_MAX_CHARS)
            .if_empty_then(SANITIZED_CONTENT_PLACEHOLDER),
    }
}

fn sanitize_optional_text(value: Option<&str>, max_chars: usize) -> Option<String> {
    value
        .map(|value| sanitize_text_for_skill(value, max_chars))
        .filter(|value| !value.is_empty())
}

pub(crate) fn sanitize_text_for_skill(value: &str, max_chars: usize) -> String {
    let sanitized = escape_template_syntax(strip_instruction_like_content(value).trim());
    truncate_chars(&sanitized, max_chars)
}

pub(crate) fn sanitize_code_for_skill(value: &str) -> String {
    let sanitized =
        escape_template_syntax(strip_instruction_like_content(value).trim_matches('\n'));
    truncate_chars(&sanitized, SKILL_CODE_MAX_CHARS)
}

fn strip_instruction_like_content(value: &str) -> String {
    let mut retained = Vec::new();
    let mut markdown_prompt_block = false;
    let mut xml_prompt_block_end = None;

    for line in value.lines() {
        let trimmed = line.trim();
        let lower = trimmed.to_ascii_lowercase();

        if let Some(closing_tag) = xml_prompt_block_end {
            if lower.contains(closing_tag) {
                xml_prompt_block_end = None;
            }
            continue;
        }

        if markdown_prompt_block {
            if trimmed.starts_with('#') {
                markdown_prompt_block = false;
            } else {
                continue;
            }
        }

        if let Some(closing_tag) = prompt_block_closing_tag(&lower) {
            if !lower.contains(closing_tag) {
                xml_prompt_block_end = Some(closing_tag);
            }
            continue;
        }

        if is_prompt_heading(trimmed, &lower) {
            markdown_prompt_block = true;
            continue;
        }

        if is_instruction_like_line(&lower) {
            continue;
        }

        retained.push(line);
    }

    retained.join("\n")
}

fn prompt_block_closing_tag(lower: &str) -> Option<&'static str> {
    if lower.starts_with("<system") {
        Some("</system>")
    } else if lower.starts_with("<developer") {
        Some("</developer>")
    } else if lower.starts_with("<assistant") {
        Some("</assistant>")
    } else if lower.starts_with("<instructions") {
        Some("</instructions>")
    } else if lower.starts_with("<instruction") {
        Some("</instruction>")
    } else {
        None
    }
}

fn is_prompt_heading(trimmed: &str, lower: &str) -> bool {
    if !trimmed.starts_with('#') {
        return false;
    }

    let heading = lower.trim_start_matches('#').trim_start();
    heading.starts_with("system")
        || heading.starts_with("developer")
        || heading.starts_with("assistant")
        || heading.starts_with("instruction")
        || heading.starts_with("instructions")
}

fn is_instruction_like_line(lower: &str) -> bool {
    [
        "system:",
        "developer:",
        "assistant:",
        "instruction:",
        "instructions:",
    ]
    .iter()
    .any(|prefix| lower.starts_with(prefix))
}

fn escape_template_syntax(value: &str) -> String {
    value
        .replace("{{{", "{ { {")
        .replace("}}}", "} } }")
        .replace("{{", "{ {")
        .replace("}}", "} }")
        .replace("${", "$ {")
        .replace("#{", "# {")
        .replace("{%", "{ %")
        .replace("%}", "% }")
        .replace("<%", "< %")
        .replace("%>", "% >")
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }

    value
        .chars()
        .take(max_chars.saturating_sub(3))
        .chain("...".chars())
        .collect()
}

trait EmptyStringFallback {
    fn if_empty_then(self, fallback: &str) -> String;
}

impl EmptyStringFallback for String {
    fn if_empty_then(self, fallback: &str) -> String {
        if self.is_empty() {
            fallback.to_string()
        } else {
            self
        }
    }
}
