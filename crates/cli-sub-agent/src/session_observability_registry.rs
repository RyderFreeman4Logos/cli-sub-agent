use std::fs;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SessionRegistryStateIssue {
    Missing,
    Unreadable,
    Corrupt,
    Invalid,
}

impl SessionRegistryStateIssue {
    fn description(&self) -> &'static str {
        match self {
            Self::Missing => "state.toml is missing",
            Self::Unreadable => "state.toml is unreadable",
            Self::Corrupt => "corrupt state.toml",
            Self::Invalid => "state.toml is not a readable session registration",
        }
    }
}

pub(crate) fn classify_session_registry_state_loss(
    project_root: &Path,
    session_id: &str,
    session_dir: &Path,
) -> Option<SessionRegistryStateIssue> {
    if !session_dir.is_dir() {
        return None;
    }
    if csa_session::load_session(project_root, session_id).is_ok() {
        return None;
    }

    let state_path = session_dir.join("state.toml");
    if !state_path.exists() {
        return Some(SessionRegistryStateIssue::Missing);
    }

    let contents = match fs::read_to_string(&state_path) {
        Ok(contents) => contents,
        Err(_) => return Some(SessionRegistryStateIssue::Unreadable),
    };

    if toml::from_str::<toml::Value>(&contents).is_err() {
        Some(SessionRegistryStateIssue::Corrupt)
    } else {
        Some(SessionRegistryStateIssue::Invalid)
    }
}

pub(crate) fn emit_session_registry_state_loss_diagnostic(
    project_root: &Path,
    session_id: &str,
    session_dir: &Path,
) -> bool {
    let Some(issue) = classify_session_registry_state_loss(project_root, session_id, session_dir)
    else {
        return false;
    };
    eprintln!(
        "{}",
        build_session_registry_state_loss_diagnostic(session_id, &issue, project_root)
    );
    true
}

fn build_session_registry_state_loss_diagnostic(
    session_id: &str,
    issue: &SessionRegistryStateIssue,
    project_root: &Path,
) -> String {
    let cd_arg = crate::daemon_caller_hints::format_cd_arg(project_root);
    format!(
        "session registry lookup failed for session '{session_id}': the session directory exists but {}. \
         This is CSA infrastructure session-registry loss, not a product-code failure. \
         Dirty or staged work may still be in the project worktree; inspect with `git status --short` and `git diff`. \
         Captured output may still be discoverable via `csa session logs --session {session_id}{cd_arg}` \
         or `csa session logs --session {session_id} --events{cd_arg}`.",
        issue.description(),
    )
}

pub(crate) fn build_session_registry_lookup_miss_diagnostic(
    session_id: &str,
    project_root: &Path,
) -> String {
    let cd_arg = crate::daemon_caller_hints::format_cd_arg(project_root);
    format!(
        "session registry lookup failed for session '{session_id}': no session registration was found in the current project, legacy state, or global exact lookup. \
         If this id came from CSA:SESSION_STARTED, this is CSA infrastructure session-registry loss, not a product-code failure. \
         Dirty or staged work may still be in the project worktree; inspect with `git status --short` and `git diff`. \
         If captured output exists, try `csa session logs --session {session_id}{cd_arg}`."
    )
}
