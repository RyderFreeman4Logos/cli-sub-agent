use anyhow::Result;
use std::fs;
use std::path::Path;

const POST_EXEC_GATE_SECTION_ID: &str = "post-exec-gate";
const POST_EXEC_GATE_SECTION_TITLE: &str = "Post-Exec Gate Failure";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::session_cmds_result) struct RenderedStructuredSection {
    id: String,
    title: String,
    content: String,
    tokens: usize,
    post_exec_gate: Option<csa_session::PostExecGateReport>,
}

pub(in crate::session_cmds_result) fn load_structured_post_exec_gate_report(
    session_dir: &Path,
) -> Option<csa_session::PostExecGateReport> {
    let result_path = session_dir.join(csa_session::result::RESULT_FILE_NAME);
    let content = fs::read_to_string(&result_path).ok()?;
    let result: csa_session::SessionResult = toml::from_str(&content).ok()?;
    result.post_exec_gate
}

pub(in crate::session_cmds_result) fn build_gate_aware_summary_content(
    report: &csa_session::PostExecGateReport,
    employee_section: Option<(&str, &str)>,
) -> String {
    let gate_summary = csa_session::post_exec_gate_failure_summary(report);
    let Some((section_id, content)) = employee_section else {
        return gate_summary;
    };
    let content = content.trim();
    if content.is_empty() || content_leads_with_post_exec_gate(content) {
        return gate_summary;
    }
    format!("{gate_summary}\n\n---\n\nSuperseded employee self-report ({section_id}):\n\n{content}")
}

pub(in crate::session_cmds_result) fn gate_summary_employee_section<'a>(
    section_id: &'a str,
    content: &'a str,
) -> Option<(&'a str, &'a str)> {
    (section_id != "full").then_some((section_id, content))
}

pub(in crate::session_cmds_result) fn build_summary_section_json_payload(
    employee_section: Option<(&str, &str)>,
    unavailable_reason: Option<String>,
    post_exec_gate: Option<&csa_session::PostExecGateReport>,
) -> Result<serde_json::Value> {
    let Some(report) = post_exec_gate else {
        let (section_id, content) = employee_section.unwrap_or(("summary", ""));
        return Ok(serde_json::json!({
            "section": section_id,
            "content": content,
            "tokens": csa_session::estimate_tokens(content),
            "unavailable_reason": unavailable_reason,
        }));
    };

    let gate_summary = csa_session::post_exec_gate_failure_summary(report);
    let mut payload = serde_json::json!({
        "section": POST_EXEC_GATE_SECTION_ID,
        "content": gate_summary,
        "tokens": csa_session::estimate_tokens(&gate_summary),
        "post_exec_gate": report,
        "unavailable_reason": unavailable_reason,
    });
    if let Some((section_id, content)) = employee_section
        && !content.trim().is_empty()
        && !content_leads_with_post_exec_gate(content)
    {
        payload["superseded_employee_self_report"] = serde_json::json!({
            "section": section_id,
            "content": content,
            "tokens": csa_session::estimate_tokens(content),
        });
    }
    Ok(payload)
}

pub(in crate::session_cmds_result) fn structured_sections_with_gate_first(
    sections: &[(csa_session::OutputSection, String)],
    post_exec_gate: Option<&csa_session::PostExecGateReport>,
) -> Vec<RenderedStructuredSection> {
    let mut rendered = Vec::with_capacity(sections.len() + usize::from(post_exec_gate.is_some()));
    if let Some(report) = post_exec_gate {
        rendered.push(gate_failure_rendered_section(report));
    }
    rendered.extend(
        sections
            .iter()
            .map(|(section, content)| RenderedStructuredSection {
                id: section.id.clone(),
                title: section.title.clone(),
                content: content.clone(),
                tokens: section.token_estimate,
                post_exec_gate: None,
            }),
    );
    rendered
}

pub(in crate::session_cmds_result) fn build_all_sections_json_payload(
    sections: &[RenderedStructuredSection],
) -> Result<serde_json::Value> {
    let json_sections: Vec<serde_json::Value> = sections
        .iter()
        .map(|section| {
            let mut value = serde_json::json!({
                "section": section.id,
                "title": section.title,
                "content": section.content,
                "tokens": section.tokens,
            });
            if let Some(report) = section.post_exec_gate.as_ref() {
                value["post_exec_gate"] = serde_json::to_value(report)?;
            }
            Ok(value)
        })
        .collect::<Result<Vec<_>>>()?;
    let mut payload = serde_json::json!({ "sections": json_sections });
    if let Some(gate_section) = sections
        .iter()
        .find(|section| section.id == POST_EXEC_GATE_SECTION_ID)
    {
        payload["post_exec_gate_summary"] = serde_json::Value::String(gate_section.content.clone());
        if let Some(report) = gate_section.post_exec_gate.as_ref() {
            payload["post_exec_gate"] = serde_json::to_value(report)?;
        }
    }
    Ok(payload)
}

pub(super) fn rendered_fallback_sections(
    content: &str,
    post_exec_gate: Option<&csa_session::PostExecGateReport>,
) -> Vec<RenderedStructuredSection> {
    let fallback = RenderedStructuredSection {
        id: "full".to_string(),
        title: "Full Output".to_string(),
        content: content.to_string(),
        tokens: csa_session::estimate_tokens(content),
        post_exec_gate: None,
    };
    let mut sections = Vec::with_capacity(1 + usize::from(post_exec_gate.is_some()));
    if let Some(report) = post_exec_gate {
        sections.push(gate_failure_rendered_section(report));
    }
    sections.push(fallback);
    sections
}

pub(super) fn gate_failure_rendered_section(
    report: &csa_session::PostExecGateReport,
) -> RenderedStructuredSection {
    let content = csa_session::post_exec_gate_failure_summary(report);
    RenderedStructuredSection {
        id: POST_EXEC_GATE_SECTION_ID.to_string(),
        title: POST_EXEC_GATE_SECTION_TITLE.to_string(),
        tokens: csa_session::estimate_tokens(&content),
        content,
        post_exec_gate: Some(report.clone()),
    }
}

pub(super) fn print_rendered_sections(sections: &[RenderedStructuredSection]) {
    for (i, section) in sections.iter().enumerate() {
        if i > 0 {
            println!();
        }
        println!("=== {} ({}) ===", section.title, section.id);
        println!("{}", section.content);
    }
}

fn content_leads_with_post_exec_gate(content: &str) -> bool {
    let trimmed = content.trim_start();
    trimmed.starts_with(csa_session::GATE_SUMMARY_LEAD)
        || (trimmed.starts_with('>') && trimmed.contains(csa_session::GATE_SUMMARY_LEAD))
}
