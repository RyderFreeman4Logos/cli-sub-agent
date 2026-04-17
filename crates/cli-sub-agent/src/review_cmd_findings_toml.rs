use std::fs;
use std::path::Path;

use csa_session::state::ReviewSessionMeta;
use csa_session::{FindingsFile, write_findings_toml};
use tracing::{debug, warn};

use crate::review_cmd::output::extract_review_text;

const FINDINGS_TOML_FENCE_LABEL: &str = "findings.toml";

/// Persist `output/findings.toml` extracted from the reviewer message.
///
/// Best-effort: missing/invalid fenced TOML produces a synthetic empty file and
/// logs a warning, but never fails the review command.
pub(super) fn persist_review_findings_toml(project_root: &Path, meta: &ReviewSessionMeta) {
    match csa_session::get_session_dir(project_root, &meta.session_id) {
        Ok(session_dir) => {
            let (artifact, warning_reason) = match derive_findings_toml_artifact(&session_dir) {
                Ok(artifact) => artifact,
                Err(error) => {
                    warn!(
                        session_id = %meta.session_id,
                        error = %error,
                        "Failed to derive review findings.toml; writing synthetic empty artifact"
                    );
                    (FindingsFile::default(), Some("derivation failure"))
                }
            };

            if let Some(reason) = warning_reason {
                warn!(
                    session_id = %meta.session_id,
                    reason,
                    "Reviewer findings.toml block missing or invalid; wrote synthetic empty artifact"
                );
            }

            if let Err(error) = write_findings_toml(&session_dir, &artifact) {
                warn!(
                    session_id = %meta.session_id,
                    error = %error,
                    "Failed to write output/findings.toml"
                );
            } else {
                debug!(session_id = %meta.session_id, "Wrote output/findings.toml");
            }
        }
        Err(error) => {
            warn!(
                session_id = %meta.session_id,
                error = %error,
                "Cannot resolve session dir for review findings.toml"
            );
        }
    }
}

fn derive_findings_toml_artifact(
    session_dir: &Path,
) -> Result<(FindingsFile, Option<&'static str>), anyhow::Error> {
    let Some(review_text) = load_review_text_for_findings(session_dir)? else {
        return Ok((FindingsFile::default(), Some("review text unavailable")));
    };

    match extract_findings_toml_from_text(&review_text) {
        Some(artifact) => Ok((artifact, None)),
        None => Ok((
            FindingsFile::default(),
            Some("findings.toml block missing or invalid"),
        )),
    }
}

fn load_review_text_for_findings(session_dir: &Path) -> Result<Option<String>, anyhow::Error> {
    let details_path = session_dir.join("output").join("details.md");
    if details_path.exists() {
        let details = fs::read_to_string(&details_path)
            .map_err(|error| anyhow::anyhow!("read {}: {error}", details_path.display()))?;
        let full_path = session_dir.join("output").join("full.md");
        if full_path.exists() {
            let raw_output = fs::read_to_string(&full_path)
                .map_err(|error| anyhow::anyhow!("read {}: {error}", full_path.display()))?;
            if let Some(review_text) = extract_review_text(&raw_output) {
                return Ok(Some(review_text));
            }
        }
        return Ok(Some(details));
    }

    let full_output_path = session_dir.join("output").join("full.md");
    if !full_output_path.exists() {
        return Ok(None);
    }

    let raw_output = fs::read_to_string(&full_output_path)
        .map_err(|error| anyhow::anyhow!("read {}: {error}", full_output_path.display()))?;
    Ok(extract_review_text(&raw_output))
}

pub(super) fn extract_findings_toml_from_text(text: &str) -> Option<FindingsFile> {
    let mut in_block = false;
    let mut block_info = String::new();
    let mut block_content = Vec::new();

    for line in text.lines() {
        let trimmed = line.trim();
        if !in_block {
            if let Some(info) = trimmed.strip_prefix("```") {
                in_block = true;
                block_info = info.trim().to_string();
                block_content.clear();
            }
            continue;
        }

        if trimmed.starts_with("```") {
            if is_findings_toml_fence_label(&block_info) {
                let content = block_content.join("\n");
                if let Ok(artifact) = toml::from_str::<FindingsFile>(&content) {
                    return Some(artifact);
                }
            }
            in_block = false;
            block_info.clear();
            block_content.clear();
            continue;
        }

        block_content.push(line.to_string());
    }

    None
}

fn is_findings_toml_fence_label(info: &str) -> bool {
    info.split_ascii_whitespace()
        .any(|token| token.eq_ignore_ascii_case(FINDINGS_TOML_FENCE_LABEL))
}

#[cfg(test)]
#[path = "review_cmd_findings_toml_tests.rs"]
mod tests;
