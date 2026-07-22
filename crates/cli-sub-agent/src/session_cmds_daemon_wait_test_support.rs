use anyhow::Result;
use chrono::Utc;
use std::path::Path;

use super::{
    SessionWaitOutputMode, WaitBehavior, WaitCallerIdentity, WaitExecutionOptions,
    WaitReconciliationOutcome, emit_wait_memory_warn_marker,
    handle_session_wait_with_hooks_and_sampler_output_mode,
};

pub(crate) fn handle_session_wait_with_hooks_at<R, E>(
    session: String,
    cd: Option<String>,
    wait_behavior: WaitBehavior,
    current_time: chrono::DateTime<Utc>,
    session_live_for_test: Option<bool>,
    mut reconcile_dead_active_session: R,
    mut emit_completion_signal: E,
) -> Result<i32>
where
    R: for<'a, 'b, 'c> FnMut(&'a Path, &'b str, &'c str) -> Result<WaitReconciliationOutcome>,
    E: for<'a, 'b> FnMut(&'a str, &'b str, i32, bool, bool),
{
    let mut cached_memory_sampler: Option<csa_session::SessionTreeMemorySampler> = None;
    handle_session_wait_with_hooks_and_sampler_output_mode(
        session,
        cd,
        WaitExecutionOptions {
            behavior: wait_behavior,
            output_mode: SessionWaitOutputMode::from_flags(false, false),
            caller_identity: WaitCallerIdentity::capture(),
            model_provider: None,
            current_time_for_test: Some(current_time),
            session_live_for_test,
        },
        &mut reconcile_dead_active_session,
        &mut emit_completion_signal,
        |project_root, session_id| {
            if cached_memory_sampler.is_none() {
                cached_memory_sampler = Some(csa_session::SessionTreeMemorySampler::new(
                    project_root,
                    session_id,
                )?);
            }
            cached_memory_sampler
                .as_ref()
                .expect("cached sampler initialized above")
                .sample_rss_mb()
        },
        emit_wait_memory_warn_marker,
    )
}
