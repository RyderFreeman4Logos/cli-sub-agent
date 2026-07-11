use super::SessionCreationMode;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ModelAttemptSessionPlan {
    pub(crate) session_arg: Option<String>,
    pub(crate) parent: Option<String>,
    pub(crate) creation_mode: SessionCreationMode,
}

/// Keep the first candidate on the explicitly requested session, but force every
/// later model candidate onto a fresh linked child. Provider sessions and KV
/// caches are model-specific and must never be resumed across model failover.
pub(crate) fn resolve_model_attempt_session(
    attempt_index: usize,
    requested_session: Option<&str>,
    failed_attempt_session: Option<&str>,
    initial_creation_mode: SessionCreationMode,
    initial_parent: Option<&str>,
) -> ModelAttemptSessionPlan {
    if attempt_index == 0 {
        let resumes_requested_session = requested_session.is_some();
        return ModelAttemptSessionPlan {
            session_arg: requested_session.map(str::to_string),
            parent: (!resumes_requested_session
                && initial_creation_mode == SessionCreationMode::FreshChild)
                .then(|| initial_parent.map(str::to_string))
                .flatten(),
            creation_mode: if resumes_requested_session {
                SessionCreationMode::DaemonManaged
            } else {
                initial_creation_mode
            },
        };
    }

    ModelAttemptSessionPlan {
        session_arg: None,
        parent: failed_attempt_session
            .or(requested_session)
            .map(str::to_string),
        creation_mode: SessionCreationMode::FreshChild,
    }
}

pub(crate) fn extract_meta_session_id_from_error(error: &anyhow::Error) -> Option<String> {
    const MARKER: &str = "meta_session_id=";
    error.chain().find_map(|cause| {
        let message = cause.to_string();
        let suffix = message.split_once(MARKER)?.1;
        let session_id: String = suffix
            .chars()
            .take_while(|ch| ch.is_ascii_alphanumeric())
            .collect();
        (!session_id.is_empty()).then_some(session_id)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cross_model_attempt_uses_fresh_child_linked_to_failed_session() {
        let first = resolve_model_attempt_session(
            0,
            Some("requested"),
            None,
            SessionCreationMode::DaemonManaged,
            None,
        );
        assert_eq!(first.session_arg.as_deref(), Some("requested"));
        assert_eq!(first.parent, None);
        assert_eq!(first.creation_mode, SessionCreationMode::DaemonManaged);

        let fallback = resolve_model_attempt_session(
            1,
            Some("requested"),
            Some("failed-attempt"),
            SessionCreationMode::DaemonManaged,
            None,
        );
        assert_eq!(fallback.session_arg, None);
        assert_eq!(fallback.parent.as_deref(), Some("failed-attempt"));
        assert_eq!(fallback.creation_mode, SessionCreationMode::FreshChild);
    }

    #[test]
    fn first_orchestrated_reviewer_uses_fresh_child_linked_to_daemon_parent() {
        let first = resolve_model_attempt_session(
            0,
            None,
            None,
            SessionCreationMode::FreshChild,
            Some("daemon-parent"),
        );

        assert_eq!(first.session_arg, None);
        assert_eq!(first.parent.as_deref(), Some("daemon-parent"));
        assert_eq!(first.creation_mode, SessionCreationMode::FreshChild);
    }

    #[test]
    fn extracts_meta_session_id_for_retry_and_failover_linkage() {
        let error = anyhow::anyhow!("transport failed")
            .context("meta_session_id=01FAILOVERCHILD provider exited");
        assert_eq!(
            extract_meta_session_id_from_error(&error).as_deref(),
            Some("01FAILOVERCHILD")
        );
    }
}
