use csa_config::GlobalConfig;

pub(super) fn run(
    global_config: Option<&GlobalConfig>,
    post_bootstrap_session_id: &str,
    startup_snapshot_session_id: Option<&str>,
    inject_warning: bool,
) -> anyhow::Result<Option<String>> {
    let current_session_id = resolve_preflight_protected_session_id(
        post_bootstrap_session_id,
        startup_snapshot_session_id,
    );
    crate::preflight_state_dir::enforce_state_dir_cap(global_config, current_session_id)?;
    Ok(inject_warning
        .then(|| {
            crate::preflight_state_dir::run_state_dir_preflight(global_config, current_session_id)
        })
        .flatten())
}

/// Resolve the session id that state-dir auto-gc must protect during execution preflight.
///
/// The startup snapshot can be missing or can still name a parent session after daemon
/// bootstrap. The post-bootstrap pipeline session is the local truth for the
/// execution/resume that is about to run.
fn resolve_preflight_protected_session_id<'a>(
    post_bootstrap_session_id: &'a str,
    _startup_snapshot_session_id: Option<&str>,
) -> Option<&'a str> {
    (!post_bootstrap_session_id.is_empty()).then_some(post_bootstrap_session_id)
}

#[cfg(test)]
mod tests {
    use super::resolve_preflight_protected_session_id;

    #[test]
    fn preflight_protection_prefers_post_bootstrap_session_over_startup_capture() {
        assert_eq!(
            resolve_preflight_protected_session_id("01ACTUALDAEMONSESSION000000", None),
            Some("01ACTUALDAEMONSESSION000000")
        );
        assert_eq!(
            resolve_preflight_protected_session_id(
                "01ACTUALDAEMONSESSION000000",
                Some("01PARENTSESSION000000000000")
            ),
            Some("01ACTUALDAEMONSESSION000000")
        );
    }

    #[test]
    fn preflight_protection_is_none_without_active_session() {
        assert_eq!(
            resolve_preflight_protected_session_id("", Some("01PARENTSESSION000000000000")),
            None
        );
    }
}
