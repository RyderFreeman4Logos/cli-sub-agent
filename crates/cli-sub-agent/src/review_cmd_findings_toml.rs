use std::fs;
use std::path::Path;

use csa_session::state::ReviewSessionMeta;
use csa_session::{FindingsFile, write_findings_toml};
use tracing::{debug, warn};

use crate::review_cmd::output::extract_review_text;
use crate::review_cmd::prose_findings::findings_file_from_prose;

const FINDINGS_TOML_FENCE_LABEL: &str = "findings.toml";

/// Sidecar marker file written alongside `output/findings.toml` when the TOML
/// was synthesized (extraction failed or block missing). Downstream verdict
/// derivation checks for this marker to distinguish synthetic-empty from
/// true-empty (#1045 round 3).
pub(super) const FINDINGS_TOML_SYNTHETIC_MARKER: &str = ".findings.toml.synthetic";

/// Persist `output/findings.toml` extracted from the reviewer message.
///
/// Best-effort: missing/invalid fenced TOML produces a synthetic empty file and
/// logs a warning, but never fails the review command.
///
/// When extraction fails, a sidecar marker file
/// `output/.findings.toml.synthetic` is written so downstream readers can
/// distinguish "reviewer said clean" from "extraction failed, we synthesized
/// empty" (#1045 round 3).
pub(super) fn persist_review_findings_toml(project_root: &Path, meta: &ReviewSessionMeta) {
    match csa_session::get_session_dir(project_root, &meta.session_id) {
        Ok(session_dir) => {
            if meta.requires_fail_closed_verdict() {
                let marker_path = session_dir
                    .join("output")
                    .join(FINDINGS_TOML_SYNTHETIC_MARKER);
                let _ = fs::remove_file(&marker_path);
                debug!(
                    session_id = %meta.session_id,
                    "Skipped synthetic findings.toml for failed review execution"
                );
                return;
            }

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

            let is_synthetic = warning_reason.is_some();

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

            // Write or remove sidecar marker depending on whether the TOML is synthetic.
            let marker_path = session_dir
                .join("output")
                .join(FINDINGS_TOML_SYNTHETIC_MARKER);
            if is_synthetic {
                if let Err(error) = fs::write(&marker_path, b"") {
                    warn!(
                        session_id = %meta.session_id,
                        error = %error,
                        "Failed to write synthetic-empty marker"
                    );
                }
            } else {
                // Real extraction succeeded — remove any stale marker from a prior round.
                let _ = fs::remove_file(&marker_path);
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
    let Some(review_text) = load_canonical_review_text(session_dir)? else {
        return Ok((FindingsFile::default(), Some("review text unavailable")));
    };

    match extract_findings_toml_from_text(&review_text) {
        Some(artifact) => Ok((artifact, None)),
        None => {
            if let Some(artifact) = findings_file_from_prose(&review_text) {
                Ok((artifact, None))
            } else {
                Ok((
                    FindingsFile::default(),
                    Some("findings.toml block missing or invalid"),
                ))
            }
        }
    }
}

/// Load the canonical review prose for a session.
///
/// Resolves the authoritative review text via a fixed source precedence:
/// when persisted `details` sections exist, prefer `output/full.md`, then the raw
/// `output.log`, then the `details.md` body; otherwise fall back to `output/full.md`.
/// This is the SINGLE source of review prose shared by both the findings extractor
/// and the fail-closed verdict detector ([`super::output::clean_detection::
/// review_contains_prose_fail_conclusion`]). Sharing one loader keeps their source
/// sets identical so a FAIL verdict can never survive in a place one consults but
/// the other ignores — the root cause of the #1675 review rounds (a verdict in
/// `details`, then `output.log`, that the detector did not scan). Returns `None`
/// when no review text can be located.
pub(in crate::review_cmd) fn load_canonical_review_text(
    session_dir: &Path,
) -> Result<Option<String>, anyhow::Error> {
    let details_path = session_dir.join("output").join("details.md");
    if details_path.exists() {
        for candidate in [
            session_dir.join("output").join("full.md"),
            session_dir.join("output.log"),
        ] {
            if !candidate.exists() {
                continue;
            }
            let raw_output = fs::read_to_string(&candidate)
                .map_err(|error| anyhow::anyhow!("read {}: {error}", candidate.display()))?;
            if let Some(review_text) = extract_review_text(&raw_output) {
                return Ok(Some(review_text));
            }
        }
        let mut latest_summary = None;
        let mut latest_details = None;
        for (section, content) in csa_session::read_all_sections(session_dir)? {
            match section.id.as_str() {
                "summary" => latest_summary = Some(content),
                "details" => latest_details = Some(content),
                _ => {}
            }
        }
        let structured_text = [latest_summary, latest_details]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join("\n");
        if !structured_text.trim().is_empty() {
            return Ok(Some(structured_text));
        }

        let mut file_text = Vec::new();
        for file_name in ["summary.md", "details.md"] {
            let path = session_dir.join("output").join(file_name);
            if !path.exists() {
                continue;
            }
            let content = fs::read_to_string(&path)
                .map_err(|error| anyhow::anyhow!("read {}: {error}", path.display()))?;
            if !content.trim().is_empty() {
                file_text.push(content);
            }
        }
        let file_text = file_text.join("\n");
        return Ok((!file_text.trim().is_empty()).then_some(file_text));
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
