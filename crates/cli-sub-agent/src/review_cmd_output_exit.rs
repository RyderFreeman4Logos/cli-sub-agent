use std::fs;
use std::path::Path;

use csa_session::ReviewVerdictArtifact;
use tracing::warn;

pub(in crate::review_cmd) fn persisted_review_verdict_exit_code(
    project_root: &Path,
    session_id: &str,
) -> i32 {
    let session_dir = match csa_session::get_session_dir(project_root, session_id) {
        Ok(session_dir) => session_dir,
        Err(error) => {
            warn!(
                session_id,
                error = %error,
                "Cannot resolve session dir for persisted review verdict; treating as infrastructure failure"
            );
            return crate::verdict_exit_code::INFRASTRUCTURE_FAILURE_EXIT_CODE;
        }
    };
    let verdict_path = session_dir.join("output").join("review-verdict.json");
    let raw = match fs::read_to_string(&verdict_path) {
        Ok(raw) => raw,
        Err(error) => {
            warn!(
                session_id,
                path = %verdict_path.display(),
                error = %error,
                "Missing or unreadable review verdict artifact; treating as infrastructure failure"
            );
            return crate::verdict_exit_code::INFRASTRUCTURE_FAILURE_EXIT_CODE;
        }
    };
    let artifact = match serde_json::from_str::<ReviewVerdictArtifact>(&raw) {
        Ok(artifact) => artifact,
        Err(error) => {
            warn!(
                session_id,
                path = %verdict_path.display(),
                error = %error,
                "Invalid review verdict artifact; treating as infrastructure failure"
            );
            return crate::verdict_exit_code::INFRASTRUCTURE_FAILURE_EXIT_CODE;
        }
    };

    crate::verdict_exit_code::exit_code_from_review_decision(artifact.decision)
}

pub(in crate::review_cmd) fn persist_review_result_exit_code(
    project_root: &Path,
    session_id: &str,
    exit_code: i32,
    caller_guard_summary: Option<&str>,
) {
    let mut result = match csa_session::load_result(project_root, session_id) {
        Ok(Some(result)) => result,
        Ok(None) => return,
        Err(error) => {
            warn!(
                session_id,
                error = %error,
                "Failed to load review result.toml for verdict exit-code alignment"
            );
            return;
        }
    };
    let summary = caller_guard_summary
        .and_then(|summary| summary.lines().map(str::trim).find(|line| !line.is_empty()))
        .map(|summary| summary.chars().take(200).collect::<String>());
    if result.exit_code == exit_code
        && result.status == csa_session::SessionResult::status_from_exit_code(exit_code)
        && summary.is_none()
    {
        return;
    }

    result.exit_code = exit_code;
    result.status = csa_session::SessionResult::status_from_exit_code(exit_code);
    if let Some(summary) = summary {
        result.summary = summary;
    }
    if let Err(error) = csa_session::save_result(project_root, session_id, &result) {
        warn!(
            session_id,
            error = %error,
            "Failed to persist review result.toml verdict exit-code alignment"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caller_guard_failure_replaces_persisted_guard_summary() {
        let project = tempfile::tempdir().expect("temp project");
        let session = csa_session::create_session(
            project.path(),
            Some("caller guard persistence"),
            None,
            Some("codex"),
        )
        .expect("create session");
        let result = csa_session::SessionResult {
            status: csa_session::SessionResult::status_from_exit_code(1),
            exit_code: 1,
            summary: "</csa-caller-sa-guard>".to_string(),
            tool: "codex".to_string(),
            ..Default::default()
        };
        csa_session::save_result(project.path(), &session.meta_session_id, &result)
            .expect("save guard result");

        persist_review_result_exit_code(
            project.path(),
            &session.meta_session_id,
            1,
            Some("Review unavailable: codex tool failure: executable not found\n"),
        );

        let persisted = csa_session::load_result(project.path(), &session.meta_session_id)
            .expect("load result")
            .expect("persisted result");
        assert_eq!(
            persisted.summary,
            "Review unavailable: codex tool failure: executable not found"
        );
    }
}
