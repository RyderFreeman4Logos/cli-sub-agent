use csa_session::FindingsFile;

use super::{
    extract_review_findings_from_prose_with_default, findings_section_bodies,
    review_finding_payload_eq,
};

const GENERATED_PROSE_FINDING_ID_PREFIX: &str = "prose-generated-";

pub(in crate::review_cmd) fn generated_prose_finding_id(index: usize) -> String {
    format!("{GENERATED_PROSE_FINDING_ID_PREFIX}{index:03}")
}

pub(in crate::review_cmd) fn is_generated_prose_finding_id(id: &str) -> bool {
    let Some(index) = id.strip_prefix(GENERATED_PROSE_FINDING_ID_PREFIX) else {
        return false;
    };
    !index.is_empty() && index.bytes().all(|byte| byte.is_ascii_digit())
}

pub(in crate::review_cmd) fn findings_file_from_explicit_findings_sections(
    text: &str,
) -> Option<FindingsFile> {
    let mut findings = Vec::new();
    for body in findings_section_bodies(text) {
        let parser_input = format!("Findings\n{}", body.as_str());
        for mut finding in extract_review_findings_from_prose_with_default(&parser_input, None) {
            if findings
                .iter()
                .any(|existing| review_finding_payload_eq(existing, &finding))
            {
                continue;
            }
            if is_generated_prose_finding_id(&finding.id) {
                finding.id = generated_prose_finding_id(findings.len() + 1);
            }
            findings.push(finding);
        }
    }
    (!findings.is_empty()).then_some(FindingsFile { findings })
}
