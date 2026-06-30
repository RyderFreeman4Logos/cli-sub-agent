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

    fn reason_code(&self) -> &'static str {
        match self {
            Self::Missing => "state_missing",
            Self::Unreadable => "state_unreadable",
            Self::Corrupt => "state_corrupt",
            Self::Invalid => "state_invalid",
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
        "<!-- CSA:SESSION_REGISTRY_LOSS session={session_id} reason={} -->\n\
         session registry lookup failed for session '{session_id}': the session directory exists but {}. \
         This is CSA infrastructure session-registry loss, not a product-code failure. \
         Dirty or staged work may still be in the project worktree; inspect git metadata with \
         `git status --short`, `git diff`, and `git diff --staged` from the project root. \
         Do not manually read session directories or transcripts; retry `csa session result --session {session_id}{cd_arg}` \
         after preserving any worktree changes.",
        issue.reason_code(),
        issue.description(),
    )
}

pub(crate) fn build_session_registry_lookup_miss_diagnostic(
    session_id: &str,
    project_root: &Path,
) -> String {
    let cd_arg = crate::daemon_caller_hints::format_cd_arg(project_root);
    format!(
        "<!-- CSA:SESSION_REGISTRY_LOSS session={session_id} reason=lookup_miss_or_gc result_location=\"unavailable_if_whole_session_gc_removed_the_session_dir\" -->\n\
         session registry lookup failed for session '{session_id}': no session registration was found in the current project, legacy state, or global exact lookup. \
         If this id came from CSA:SESSION_STARTED, this is CSA infrastructure session-registry loss, not a product-code failure. \
         If `csa gc --max-age-days`, `csa gc --global --max-age-days`, `csa session clean`, or `csa session delete` removed the whole session, \
         its result.toml was inside the removed session directory and was not moved elsewhere; runtime-only GC preserves result.toml and should remain waitable. \
         Dirty or staged work may still be in the project worktree; inspect git metadata with \
         `git status --short`, `git diff`, and `git diff --staged` from the project root. \
         Do not manually read session directories or transcripts; retry `csa session result --session {session_id}{cd_arg}` \
         after preserving any worktree changes."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const SESSION_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";

    #[test]
    fn registry_lookup_miss_diagnostic_is_bounded_and_git_recovery_focused() {
        let diagnostic =
            build_session_registry_lookup_miss_diagnostic(SESSION_ID, Path::new("/repo"));

        assert!(diagnostic.len() < 1100, "{diagnostic}");
        assert!(diagnostic.contains("CSA:SESSION_REGISTRY_LOSS"));
        assert!(diagnostic.contains("reason=lookup_miss_or_gc"));
        assert!(diagnostic.contains("result.toml was inside the removed session directory"));
        assert!(diagnostic.contains("CSA infrastructure session-registry loss"));
        assert!(diagnostic.contains("git status --short"));
        assert!(diagnostic.contains("git diff --staged"));
        assert!(diagnostic.contains("Do not manually read session directories or transcripts"));
        assert!(!diagnostic.contains("session logs"));
        assert!(!diagnostic.contains("acp-events"));
    }

    #[test]
    fn registry_state_loss_diagnostic_is_bounded_and_git_recovery_focused() {
        let diagnostic = build_session_registry_state_loss_diagnostic(
            SESSION_ID,
            &SessionRegistryStateIssue::Missing,
            Path::new("/repo"),
        );

        assert!(diagnostic.len() < 850, "{diagnostic}");
        assert!(diagnostic.contains("CSA:SESSION_REGISTRY_LOSS"));
        assert!(diagnostic.contains("reason=state_missing"));
        assert!(diagnostic.contains("state.toml is missing"));
        assert!(diagnostic.contains("CSA infrastructure session-registry loss"));
        assert!(diagnostic.contains("git status --short"));
        assert!(diagnostic.contains("git diff --staged"));
        assert!(diagnostic.contains("Do not manually read session directories or transcripts"));
        assert!(!diagnostic.contains("session logs"));
        assert!(!diagnostic.contains("acp-events"));
    }
}
