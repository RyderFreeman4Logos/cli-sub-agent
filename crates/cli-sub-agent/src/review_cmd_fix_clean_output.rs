use std::fs;
use std::path::Path;

use tracing::warn;

const CLEAN_CONVERGENCE_STALE_OUTPUT_FILES: &[&str] = &[
    "suggestion.toml",
    super::super::findings_toml::FINDINGS_TOML_SYNTHETIC_MARKER,
];
const REVIEW_PROSE_SECTION_IDS: &[&str] = &["summary", "details"];

pub(super) fn clear_clean_convergence_fail_signals(
    session_dir: &Path,
    session_id: &str,
    current_fix_output: Option<&str>,
) {
    let output_dir = session_dir.join("output");
    for stale_file in CLEAN_CONVERGENCE_STALE_OUTPUT_FILES {
        let stale_path = output_dir.join(stale_file);
        if let Err(error) = fs::remove_file(&stale_path)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            warn!(
                session_id,
                stale_file,
                error = %error,
                "Failed to remove stale review output artifact after CLEAN convergence"
            );
        }
    }
    retain_current_review_prose_sections(session_dir, session_id, current_fix_output);
}

fn retain_current_review_prose_sections(
    session_dir: &Path,
    session_id: &str,
    current_fix_output: Option<&str>,
) {
    let output_dir = session_dir.join("output");
    let index_path = output_dir.join("index.toml");
    if !index_path.exists() {
        remove_legacy_review_prose_files(&output_dir, session_id);
        return;
    }

    let contents = match fs::read_to_string(&index_path) {
        Ok(contents) => contents,
        Err(error) => {
            warn!(
                session_id,
                error = %error,
                "Failed to read output/index.toml while clearing stale review prose"
            );
            return;
        }
    };
    let mut index: csa_session::OutputIndex = match toml::from_str(&contents) {
        Ok(index) => index,
        Err(error) => {
            warn!(
                session_id,
                error = %error,
                "Failed to parse output/index.toml while clearing stale review prose"
            );
            return;
        }
    };

    let current_review_sections = current_fix_output
        .map(current_output_review_prose_section_ids)
        .unwrap_or_default();
    let keep = review_prose_keep_mask(&index, &current_review_sections);

    if keep.iter().all(|keep_section| *keep_section) {
        return;
    }

    for (idx, section) in index.sections.iter().enumerate() {
        if keep[idx] {
            continue;
        }
        if let Some(file_path) = &section.file_path {
            let stale_path = output_dir.join(file_path);
            if let Err(error) = fs::remove_file(&stale_path)
                && error.kind() != std::io::ErrorKind::NotFound
            {
                warn!(
                    session_id,
                    file_path,
                    error = %error,
                    "Failed to remove stale review prose section after CLEAN convergence"
                );
            }
        }
    }

    index.sections = index
        .sections
        .into_iter()
        .enumerate()
        .filter_map(|(idx, section)| keep[idx].then_some(section))
        .collect();
    index.total_tokens = index
        .sections
        .iter()
        .map(|section| section.token_estimate)
        .sum();

    match toml::to_string_pretty(&index) {
        Ok(rendered) => {
            if let Err(error) = fs::write(&index_path, rendered) {
                warn!(
                    session_id,
                    error = %error,
                    "Failed to rewrite output/index.toml after clearing stale review prose"
                );
            }
        }
        Err(error) => {
            warn!(
                session_id,
                error = %error,
                "Failed to render output/index.toml after clearing stale review prose"
            );
        }
    }
}

fn review_prose_keep_mask(index: &csa_session::OutputIndex, current_ids: &[String]) -> Vec<bool> {
    let mut keep = vec![true; index.sections.len()];
    let mut expected_current = current_ids.iter().rev();
    let mut next_expected = expected_current.next();

    for (idx, section) in index.sections.iter().enumerate().rev() {
        if !is_review_prose_section_id(&section.id) {
            continue;
        }

        if next_expected.is_some_and(|expected| section.id == *expected) {
            next_expected = expected_current.next();
        } else {
            keep[idx] = false;
        }
    }

    keep
}

fn current_output_review_prose_section_ids(output: &str) -> Vec<String> {
    let mut ids = Vec::new();
    let mut open_section_id = None;

    for line in output.lines() {
        match parse_csa_section_marker(line) {
            Some(CsaSectionMarker::Start(id)) => {
                if let Some(previous_id) = open_section_id.take() {
                    push_review_prose_section_id(&mut ids, previous_id);
                }
                open_section_id = Some(id);
            }
            Some(CsaSectionMarker::End(id)) if open_section_id.as_deref() == Some(id.as_str()) => {
                push_review_prose_section_id(&mut ids, id);
                open_section_id = None;
            }
            None => {}
            Some(CsaSectionMarker::End(_)) => {}
        }
    }

    if let Some(id) = open_section_id {
        push_review_prose_section_id(&mut ids, id);
    }

    ids
}

fn push_review_prose_section_id(ids: &mut Vec<String>, section_id: String) {
    if is_review_prose_section_id(&section_id) {
        ids.push(section_id);
    }
}

enum CsaSectionMarker {
    Start(String),
    End(String),
}

fn parse_csa_section_marker(line: &str) -> Option<CsaSectionMarker> {
    let marker = line
        .trim()
        .strip_prefix("<!-- CSA:SECTION:")?
        .strip_suffix("-->")?
        .trim();
    let marker = marker
        .strip_suffix(":END")
        .map(|section_id| CsaSectionMarker::End(section_id.trim().to_string()))
        .unwrap_or_else(|| CsaSectionMarker::Start(marker.to_string()));
    Some(marker)
}

fn is_review_prose_section_id(section_id: &str) -> bool {
    REVIEW_PROSE_SECTION_IDS.contains(&section_id)
}

fn remove_legacy_review_prose_files(output_dir: &Path, session_id: &str) {
    for section_id in REVIEW_PROSE_SECTION_IDS {
        let stale_path = output_dir.join(format!("{section_id}.md"));
        if let Err(error) = fs::remove_file(&stale_path)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            warn!(
                session_id,
                section_id,
                error = %error,
                "Failed to remove legacy review prose file after CLEAN convergence"
            );
        }
    }
}
