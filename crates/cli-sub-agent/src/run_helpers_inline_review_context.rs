use anyhow::{Context, Result};
use std::fs;
use std::path::Path;
use tracing::warn;

pub(crate) fn prepend_review_context_to_prompt(
    project_root: &Path,
    prompt: String,
    review_session_id: Option<&str>,
) -> Result<String> {
    let Some(session_id) = review_session_id else {
        return Ok(prompt);
    };

    csa_session::validate_session_id(session_id).with_context(|| {
        format!("--inline-context-from-review-session: invalid session ID '{session_id}'")
    })?;

    let session_dir = csa_session::get_session_dir(project_root, session_id)?;
    if !session_dir.exists() {
        anyhow::bail!(
            "--inline-context-from-review-session: session {} not found",
            session_id
        );
    }

    let output_dir = session_dir.join("output");
    let summary = read_optional_review_context_file(&output_dir.join("summary.md"))?;
    let details = read_optional_review_context_file(&output_dir.join("details.md"))?;
    let findings = read_optional_review_context_file(&output_dir.join("findings.toml"))?;

    if summary.is_none() && details.is_none() && findings.is_none() {
        warn!(
            session_id = %session_id,
            "Inline review context requested but summary/details/findings artifacts were missing"
        );
        return Ok(prompt);
    }

    Ok(format_review_context_prompt(
        session_id, &prompt, summary, details, findings,
    ))
}

fn read_optional_review_context_file(path: &Path) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(content) => Ok(Some(content)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err)
            .with_context(|| format!("failed to read review context file '{}'", path.display())),
    }
}

fn format_review_context_prompt(
    session_id: &str,
    prompt: &str,
    summary: Option<String>,
    details: Option<String>,
    findings: Option<String>,
) -> String {
    let mut rendered = format!("<csa-review-context session=\"{session_id}\">\n");
    append_review_context_section(&mut rendered, "summary.md", summary.as_deref());
    append_review_context_section(&mut rendered, "details.md", details.as_deref());
    append_review_context_section(&mut rendered, "findings.toml", findings.as_deref());
    rendered.push_str("</csa-review-context>\n\n<original-prompt>\n");
    rendered.push_str(prompt);
    if !prompt.ends_with('\n') {
        rendered.push('\n');
    }
    rendered.push_str("</original-prompt>\n");
    rendered
}

fn append_review_context_section(rendered: &mut String, file_name: &str, content: Option<&str>) {
    let Some(content) = content else {
        return;
    };

    rendered.push_str(&format!("<!-- {file_name} -->\n"));
    rendered.push_str(content);
    if !content.ends_with('\n') {
        rendered.push('\n');
    }
}
